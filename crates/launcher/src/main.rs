#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use shared::{launcher_pipe_path, zenzai_cpu_backend_supported, AppConfig};
use std::collections::VecDeque;
use std::ffi::c_void;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::ptr::addr_of_mut;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};
use std::{env, thread};

use anyhow::Context as _;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use windows::{
    core::{PCWSTR, PWSTR},
    Win32::{
        Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL},
        Security::{
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
                SDDL_REVISION,
            },
            GetTokenInformation, TokenLogonSid, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
            TOKEN_GROUPS, TOKEN_QUERY,
        },
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    },
};

const SERVER_RESTART_DELAY: Duration = Duration::from_secs(1);
const SERVER_RESTART_WINDOW: Duration = Duration::from_secs(60);
const SERVER_RESTART_BURST_LIMIT: usize = 5;
const SERVER_RESTART_COOLDOWN: Duration = Duration::from_secs(30);
const SERVER_WATCH_POLL_INTERVAL: Duration = Duration::from_millis(100);
const LAUNCHER_RESTART_COMMAND: &str = "restart-server";
const LAUNCHER_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const LAUNCHER_CRASH_TRACE_FILE_NAME: &str = "launcher-crash-trace.json";
const LAUNCHER_PREVIOUS_CRASH_TRACE_FILE_NAME: &str = "launcher-crash-trace.previous.json";

fn main() -> anyhow::Result<()> {
    shared::enable_redirection_guard().map_err(anyhow::Error::msg)?;
    let cpu_backend_supported = zenzai_cpu_backend_supported();
    env::set_var(
        "AZOOKEY_ZENZAI_CPU_SUPPORTED",
        if cpu_backend_supported { "1" } else { "0" },
    );

    let exe_path = env::current_exe()?.parent().unwrap().to_path_buf();
    let (command_tx, command_rx) = mpsc::channel();
    // Reserve the session-specific pipe before starting either child. A duplicate
    // launcher (or an ACL failure) must fail without launching more processes.
    start_launcher_command_listener(command_tx, launcher_pipe_path()?)?;

    let server_exe_path = exe_path.clone();
    let server_handle = thread::spawn(move || {
        if let Err(error) =
            watch_server_process(&server_exe_path, cpu_backend_supported, command_rx)
        {
            eprintln!("[launcher] server watchdog stopped: {error:?}");
        }
    });

    let mut ui = start_ui_process(&exe_path)?;
    let ui_status = ui.wait().context("Failed to wait for ui.exe")?;
    eprintln!("[launcher] ui.exe exited: {ui_status}");

    let _ = server_handle.join();

    Ok(())
}

fn watch_server_process(
    install_dir: &Path,
    cpu_backend_supported: bool,
    command_rx: Receiver<LauncherCommand>,
) -> anyhow::Result<()> {
    let mut recent_restarts = VecDeque::new();

    loop {
        let mut server = start_server_process(install_dir, cpu_backend_supported)?;
        let status = wait_for_server_exit_or_restart_request(&mut server, &command_rx)?;
        let restart_delay = match status {
            ServerExit::Exited(status) => {
                eprintln!("[launcher] azookey-server.exe exited: {status}");
                restart_delay_after_server_exit(&mut recent_restarts, false, Instant::now())
            }
            ServerExit::RestartRequested(status) => {
                eprintln!("[launcher] azookey-server.exe restarted by request: {status}");
                restart_delay_after_server_exit(&mut recent_restarts, true, Instant::now())
            }
        };

        if let Some(delay) = restart_delay {
            thread::sleep(delay);
        }
    }
}

fn restart_delay_after_server_exit(
    recent_restarts: &mut VecDeque<Instant>,
    restart_requested: bool,
    now: Instant,
) -> Option<Duration> {
    if restart_requested {
        return None;
    }

    recent_restarts.push_back(now);
    while recent_restarts
        .front()
        .is_some_and(|started| now.duration_since(*started) > SERVER_RESTART_WINDOW)
    {
        recent_restarts.pop_front();
    }

    if recent_restarts.len() >= SERVER_RESTART_BURST_LIMIT {
        eprintln!(
            "[launcher] azookey-server.exe restarted too often; cooling down for {} seconds",
            SERVER_RESTART_COOLDOWN.as_secs()
        );
        recent_restarts.clear();
        Some(SERVER_RESTART_COOLDOWN)
    } else {
        Some(SERVER_RESTART_DELAY)
    }
}

fn start_server_process(install_dir: &Path, cpu_backend_supported: bool) -> anyhow::Result<Child> {
    let config = load_config();

    if config.zenzai.enable && config.zenzai.backend == "cpu" && !cpu_backend_supported {
        eprintln!("[launcher] CPU backend requires AVX support. Zenzai will fall back to standard conversion.");
    }

    let mut command = process_command_with_backend(install_dir, "azookey-server.exe", &config)?;
    command.env(
        "AZOOKEY_ZENZAI_CPU_SUPPORTED",
        if cpu_backend_supported { "1" } else { "0" },
    );

    let startup_details = format!(
        "backend={};backend_dir={};zenzai_enable={};cpu_backend_supported={}",
        config.zenzai.backend,
        backend_dir(&config),
        config.zenzai.enable,
        cpu_backend_supported
    );
    write_launcher_crash_trace(
        &config,
        "server_startup",
        "spawning",
        "begin",
        &startup_details,
    );
    match spawn_process(command, "azookey-server.exe", "[server]") {
        Ok(child) => {
            write_launcher_crash_trace(
                &config,
                "server_startup",
                "spawned",
                "begin",
                &format!("child_pid={};{startup_details}", child.id()),
            );
            Ok(child)
        }
        Err(error) => {
            write_launcher_crash_trace(
                &config,
                "server_startup",
                "spawn",
                "error",
                &format!("{startup_details};error={error:?}"),
            );
            Err(error)
        }
    }
}

fn start_ui_process(install_dir: &Path) -> anyhow::Result<Child> {
    let config = load_config();
    let command = process_command_with_backend(install_dir, "ui.exe", &config)?;
    spawn_process(command, "ui.exe", "[ui]")
}

fn process_command_with_backend(
    install_dir: &Path,
    exe: &str,
    config: &AppConfig,
) -> anyhow::Result<Command> {
    let backend_path = install_dir.join(backend_dir(config));
    let mut command = process_command(install_dir, exe)?;
    command.env("PATH", prepend_to_path(&backend_path));
    Ok(command)
}

fn process_command(install_dir: &Path, exe: &str) -> anyhow::Result<Command> {
    let exe_path = install_dir.join(exe);
    if !exe_path.is_file() {
        anyhow::bail!(
            "Refusing to launch {exe} outside the protected install directory; expected file is missing: {}",
            exe_path.display()
        );
    }

    let mut command = Command::new(&exe_path);
    command.current_dir(install_dir);
    Ok(command)
}

fn resolve_log_path(file_name: &str) -> Option<std::path::PathBuf> {
    env::var("APPDATA").ok().map(|appdata| {
        std::path::PathBuf::from(appdata)
            .join("Azookey")
            .join("logs")
            .join(file_name)
    })
}

fn json_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            ch if ch.is_control() => escaped.push(' '),
            ch => escaped.push(ch),
        }
    }
    escaped
}

fn crash_trace_is_in_progress(trace: &str) -> bool {
    trace.contains("\"state\":\"begin\"") || trace.contains("\"state\": \"begin\"")
}

fn crash_trace_is_completed(trace: &str) -> bool {
    trace.contains("\"state\":\"completed\"") || trace.contains("\"state\": \"completed\"")
}

fn preserve_launcher_crash_trace_if_incomplete(config: &AppConfig) {
    if !config.debug.server_crash_trace_enabled {
        return;
    }

    let Some(source_path) = resolve_log_path(LAUNCHER_CRASH_TRACE_FILE_NAME) else {
        return;
    };
    let Ok(trace) = fs::read_to_string(source_path) else {
        return;
    };
    if !crash_trace_is_in_progress(&trace) {
        return;
    }

    let Some(previous_path) = resolve_log_path(LAUNCHER_PREVIOUS_CRASH_TRACE_FILE_NAME) else {
        return;
    };
    if let Some(parent) = previous_path.parent() {
        if let Err(error) = fs::create_dir_all(parent) {
            eprintln!("[launcher] failed to create previous crash trace directory: {error}");
            return;
        }
    }
    if let Err(error) = fs::write(previous_path, trace.as_bytes()) {
        eprintln!("[launcher] failed to preserve previous crash trace: {error}");
    }
}

fn write_launcher_crash_trace(
    config: &AppConfig,
    operation: &str,
    stage: &str,
    state: &str,
    details: &str,
) {
    if !config.debug.server_crash_trace_enabled {
        return;
    }

    if stage == "spawning" && state == "begin" {
        preserve_launcher_crash_trace_if_incomplete(config);
    }

    let Some(path) = resolve_log_path(LAUNCHER_CRASH_TRACE_FILE_NAME) else {
        return;
    };
    if stage == "spawned"
        && state == "begin"
        && fs::read_to_string(&path)
            .map(|trace| crash_trace_is_completed(&trace))
            .unwrap_or(false)
    {
        return;
    }
    if let Some(parent) = path.parent() {
        if let Err(error) = fs::create_dir_all(parent) {
            eprintln!("[launcher] failed to create crash trace directory: {error}");
            return;
        }
    }

    let trace = format!(
        concat!(
            "{{\n",
            "  \"timestamp_ms\": {},\n",
            "  \"process_id\": {},\n",
            "  \"component\": \"launcher\",\n",
            "  \"operation\": \"{}\",\n",
            "  \"stage\": \"{}\",\n",
            "  \"state\": \"{}\",\n",
            "  \"details\": \"{}\"\n",
            "}}\n"
        ),
        now_timestamp_millis(),
        std::process::id(),
        json_escape(operation),
        json_escape(stage),
        json_escape(state),
        json_escape(details),
    );

    match File::create(path).and_then(|mut file| {
        file.write_all(trace.as_bytes())?;
        file.flush()
    }) {
        Ok(()) => {}
        Err(error) => eprintln!("[launcher] failed to write crash trace: {error}"),
    }
}

fn now_timestamp_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

fn spawn_process(mut command: Command, exe: &str, prefix: &str) -> anyhow::Result<Child> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("Failed to start {exe}"))?;

    if let Some(stdout) = child.stdout.take() {
        let stdout_reader = BufReader::new(stdout);
        let prefix_stdout = prefix.to_string();
        thread::spawn(move || {
            for line in stdout_reader.lines().map_while(Result::ok) {
                println!("{}: {}", prefix_stdout, line);
            }
        });
    }

    if let Some(stderr) = child.stderr.take() {
        let stderr_reader = BufReader::new(stderr);
        let prefix_stderr = prefix.to_string();
        thread::spawn(move || {
            for line in stderr_reader.lines().map_while(Result::ok) {
                eprintln!("{}: {}", prefix_stderr, line);
            }
        });
    }

    Ok(child)
}

fn wait_for_server_exit_or_restart_request(
    server: &mut Child,
    command_rx: &Receiver<LauncherCommand>,
) -> anyhow::Result<ServerExit> {
    loop {
        if let Some(status) = server
            .try_wait()
            .context("Failed to check azookey-server.exe status")?
        {
            return Ok(ServerExit::Exited(status));
        }

        match command_rx.recv_timeout(SERVER_WATCH_POLL_INTERVAL) {
            Ok(LauncherCommand::RestartServer { reply }) => {
                let result = terminate_server_child(server);
                let reply_result = result
                    .as_ref()
                    .map(|_| ())
                    .map_err(|error| error.to_string());
                let _ = reply.send(reply_result);
                return result.map(ServerExit::RestartRequested);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                let status = server
                    .wait()
                    .context("Failed to wait for azookey-server.exe")?;
                return Ok(ServerExit::Exited(status));
            }
        }
    }
}

fn terminate_server_child(server: &mut Child) -> anyhow::Result<ExitStatus> {
    if let Some(status) = server
        .try_wait()
        .context("Failed to check azookey-server.exe status")?
    {
        return Ok(status);
    }

    server
        .kill()
        .context("Failed to terminate azookey-server.exe")?;
    server
        .wait()
        .context("Failed to wait for azookey-server.exe after restart request")
}

fn start_launcher_command_listener(
    command_tx: Sender<LauncherCommand>,
    pipe_path: &str,
) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    let pipe = {
        let _runtime_guard = runtime.enter();
        create_launcher_command_pipe(pipe_path, true)?
    };

    let pipe_path = pipe_path.to_owned();
    thread::spawn(move || {
        if let Err(error) =
            runtime.block_on(run_launcher_command_listener(pipe, command_tx, &pipe_path))
        {
            eprintln!("[launcher] command listener stopped: {error:?}");
        }
    });
    Ok(())
}

async fn run_launcher_command_listener(
    mut pipe: NamedPipeServer,
    command_tx: Sender<LauncherCommand>,
    pipe_path: &str,
) -> anyhow::Result<()> {
    loop {
        let connection = pipe.connect().await;

        // Reserve the next instance before dropping the connected handle. This
        // keeps the name owned without discarding the client's unread response
        // (DisconnectNamedPipe would discard it; Tokio flush is a no-op).
        let next_pipe = create_launcher_command_pipe(pipe_path, false)?;
        match connection {
            Ok(()) => {
                if let Err(error) = handle_launcher_command(&mut pipe, &command_tx).await {
                    eprintln!("[launcher] command failed: {error:?}");
                }
            }
            Err(error) => eprintln!("[launcher] command connection failed: {error:?}"),
        }
        pipe = next_pipe;
    }
}

fn create_launcher_pipe_security_descriptor() -> anyhow::Result<PSECURITY_DESCRIPTOR> {
    let logon_sid = current_logon_sid_string()?;
    let sddl = launcher_pipe_sddl(&logon_sid);
    let sddl_wide = sddl.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let mut security_descriptor = PSECURITY_DESCRIPTOR::default();

    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl_wide.as_ptr()),
            SDDL_REVISION,
            &mut security_descriptor,
            None,
        )
        .context("Failed to create launcher pipe security descriptor")?;
    }

    Ok(security_descriptor)
}

fn launcher_pipe_sddl(logon_sid: &str) -> String {
    format!("D:(D;;GA;;;NU)(A;;GA;;;SY)(A;;GRGW;;;{logon_sid})S:(ML;;NW;;;ME)")
}

fn current_logon_sid_string() -> anyhow::Result<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)
            .context("Failed to open current process token")?;

        let result = logon_sid_string_from_token(token);
        let _ = CloseHandle(token);
        result
    }
}

fn logon_sid_string_from_token(token: HANDLE) -> anyhow::Result<String> {
    unsafe {
        let mut token_info_length = 0;
        let _ = GetTokenInformation(token, TokenLogonSid, None, 0, &mut token_info_length);
        anyhow::ensure!(
            token_info_length >= size_of::<TOKEN_GROUPS>() as u32,
            "Failed to get current logon SID buffer size"
        );

        // TOKEN_GROUPS contains pointers; use the same aligned buffer and
        // fail-closed single-logon-SID check as the server.
        let word_count = (token_info_length as usize).div_ceil(size_of::<usize>());
        let mut token_info = vec![0usize; word_count];
        GetTokenInformation(
            token,
            TokenLogonSid,
            Some(token_info.as_mut_ptr().cast()),
            token_info_length,
            &mut token_info_length,
        )
        .context("Failed to get current logon SID")?;

        let token_groups = &*(token_info.as_ptr() as *const TOKEN_GROUPS);
        anyhow::ensure!(
            token_groups.GroupCount == 1,
            "Expected one logon SID, got {}",
            token_groups.GroupCount
        );
        let mut sid_string = PWSTR::null();
        ConvertSidToStringSidW(token_groups.Groups[0].Sid, &mut sid_string)
            .context("Failed to convert current logon SID to string")?;

        let result = sid_string
            .to_string()
            .context("Failed to decode current logon SID string");
        let _ = LocalFree(HLOCAL(sid_string.as_ptr().cast()));
        result
    }
}

fn create_launcher_command_pipe(
    path: &str,
    first_pipe_instance: bool,
) -> anyhow::Result<NamedPipeServer> {
    let descriptor = create_launcher_pipe_security_descriptor()?;
    let mut attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: false.into(),
    };
    let result = unsafe {
        ServerOptions::new()
            .first_pipe_instance(first_pipe_instance)
            .create_with_security_attributes_raw(path, addr_of_mut!(attributes) as *mut c_void)
    };
    unsafe {
        let _ = LocalFree(HLOCAL(descriptor.0));
    }
    result.context("Failed to create launcher command pipe")
}

async fn handle_launcher_command(
    pipe: &mut NamedPipeServer,
    command_tx: &Sender<LauncherCommand>,
) -> anyhow::Result<()> {
    let mut buffer = [0u8; 256];
    let size = pipe
        .read(&mut buffer)
        .await
        .context("Failed to read launcher command")?;

    let result = match parse_launcher_command(&buffer[..size]) {
        Ok(LauncherCommandKind::RestartServer) => request_server_restart(command_tx),
        Err(error) => Err(error),
    };
    let response = launcher_response(result);

    pipe.write_all(response.as_bytes())
        .await
        .context("Failed to write launcher command response")?;
    pipe.flush()
        .await
        .context("Failed to flush launcher command response")?;

    Ok(())
}

fn parse_launcher_command(bytes: &[u8]) -> anyhow::Result<LauncherCommandKind> {
    match std::str::from_utf8(bytes)
        .context("Launcher command is not UTF-8")?
        .trim()
    {
        LAUNCHER_RESTART_COMMAND => Ok(LauncherCommandKind::RestartServer),
        command => anyhow::bail!("Unknown launcher command: {command}"),
    }
}

fn request_server_restart(command_tx: &Sender<LauncherCommand>) -> anyhow::Result<()> {
    let (reply_tx, reply_rx) = mpsc::channel();
    command_tx
        .send(LauncherCommand::RestartServer { reply: reply_tx })
        .context("Server watchdog is not running")?;

    match reply_rx.recv_timeout(LAUNCHER_COMMAND_TIMEOUT) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(message)) => anyhow::bail!(message),
        Err(RecvTimeoutError::Timeout) => anyhow::bail!("Timed out waiting for server restart"),
        Err(RecvTimeoutError::Disconnected) => {
            anyhow::bail!("Server watchdog stopped before restart completed")
        }
    }
}

fn launcher_response(result: anyhow::Result<()>) -> String {
    match result {
        Ok(()) => "ok\n".to_string(),
        Err(error) => format!("error:{error}\n"),
    }
}

fn load_config() -> AppConfig {
    AppConfig::new().unwrap_or_else(|error| {
        eprintln!("[launcher] Failed to load settings; using defaults: {error}");
        AppConfig::default()
    })
}

fn backend_dir(config: &AppConfig) -> &'static str {
    match config.zenzai.backend.as_str() {
        "cuda" => "llama_cuda",
        "vulkan" => "llama_vulkan",
        _ => "llama_cpu",
    }
}

fn prepend_to_path(path: &Path) -> String {
    let existing = env::var("PATH").unwrap_or_default();
    format!("{};{}", path.to_string_lossy(), existing)
}

#[derive(Debug)]
enum ServerExit {
    Exited(ExitStatus),
    RestartRequested(ExitStatus),
}

enum LauncherCommand {
    RestartServer {
        reply: Sender<std::result::Result<(), String>>,
    },
}

enum LauncherCommandKind {
    RestartServer,
}

#[cfg(test)]
mod tests {
    use super::{
        launcher_pipe_sddl, launcher_response, parse_launcher_command, process_command,
        restart_delay_after_server_exit, LauncherCommandKind, SERVER_RESTART_BURST_LIMIT,
        SERVER_RESTART_COOLDOWN, SERVER_RESTART_DELAY,
    };
    use std::collections::VecDeque;
    use std::time::{Duration, Instant};

    #[test]
    fn parse_launcher_command_accepts_restart_server() {
        assert!(matches!(
            parse_launcher_command(b"restart-server\n").unwrap(),
            LauncherCommandKind::RestartServer
        ));
    }

    #[test]
    fn parse_launcher_command_rejects_unknown_command() {
        assert!(parse_launcher_command(b"stop-server\n").is_err());
    }

    #[test]
    fn launcher_response_encodes_success_and_error() {
        assert_eq!(launcher_response(Ok(())), "ok\n");
        assert_eq!(
            launcher_response(Err(anyhow::anyhow!("denied"))),
            "error:denied\n"
        );
    }

    #[test]
    fn launcher_pipe_sddl_denies_network_and_grants_only_system_and_current_logon() {
        assert_eq!(
            launcher_pipe_sddl("S-1-5-5-1-2"),
            "D:(D;;GA;;;NU)(A;;GA;;;SY)(A;;GRGW;;;S-1-5-5-1-2)S:(ML;;NW;;;ME)"
        );
    }

    #[test]
    fn launcher_pipe_reserves_name_across_connections_and_duplicate_startup_fails() {
        use super::{
            create_launcher_command_pipe, run_launcher_command_listener,
            start_launcher_command_listener,
        };
        use std::sync::mpsc;
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::windows::named_pipe::ClientOptions,
        };
        use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_PIPE_BUSY};

        fn open_client(path: &str) -> tokio::net::windows::named_pipe::NamedPipeClient {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match ClientOptions::new().open(path) {
                    Ok(client) => return client,
                    Err(error)
                        if error.raw_os_error() == Some(ERROR_PIPE_BUSY.0 as i32)
                            && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("Failed to open test launcher pipe: {error}"),
                }
            }
        }

        let runtime = tokio::runtime::Runtime::new().unwrap();
        let path = format!(
            r"\\.\pipe\LOCAL\azookey_launcher_test_{}_{}",
            std::process::id(),
            super::now_timestamp_millis()
        );
        let pipe = {
            let _runtime_guard = runtime.enter();
            create_launcher_command_pipe(&path, true).unwrap()
        };
        let (command_tx, command_rx) = mpsc::channel();

        // Initialization failure must reach main's gate before children spawn.
        assert!(start_launcher_command_listener(command_tx.clone(), &path).is_err());
        let watchdog = std::thread::spawn(move || {
            while let Ok(super::LauncherCommand::RestartServer { reply }) = command_rx.recv() {
                reply.send(Ok(())).unwrap();
            }
        });

        runtime.block_on(async {
            let listener_path = path.clone();
            let listener = tokio::spawn(async move {
                run_launcher_command_listener(pipe, command_tx, &listener_path).await
            });
            for _ in 0..3 {
                // An abandoned connection must not release the launcher name
                // or stop subsequent restart requests.
                drop(open_client(&path));
                let mut client = open_client(&path);
                client.write_all(b"restart-server\n").await.unwrap();
                let mut response = [0u8; 256];
                let size = client.read(&mut response).await.unwrap();
                assert_eq!(&response[..size], b"ok\n");

                let error = create_launcher_command_pipe(&path, true).unwrap_err();
                assert_eq!(
                    error
                        .downcast_ref::<std::io::Error>()
                        .unwrap()
                        .raw_os_error(),
                    Some(ERROR_ACCESS_DENIED.0 as i32)
                );
            }

            listener.abort();
            assert!(listener.await.unwrap_err().is_cancelled());
            // Releasing the resident handle permits a new launcher instance.
            assert!(create_launcher_command_pipe(&path, true).is_ok());
        });
        watchdog.join().unwrap();
    }

    #[test]
    fn missing_protected_executable_never_falls_back_to_path() {
        let install_dir = std::env::temp_dir().join("azookey-missing-protected-executable-test");

        let error = process_command(&install_dir, "ui.exe").unwrap_err();

        assert!(error.to_string().contains("protected install directory"));
        assert!(error.to_string().contains("ui.exe"));
    }

    #[test]
    fn requested_restarts_do_not_count_toward_crash_cooldown() {
        let mut recent_restarts = VecDeque::new();
        let start = Instant::now();

        for offset in 0..SERVER_RESTART_BURST_LIMIT {
            assert_eq!(
                restart_delay_after_server_exit(
                    &mut recent_restarts,
                    true,
                    start + Duration::from_secs(offset as u64)
                ),
                None
            );
        }

        assert!(recent_restarts.is_empty());
        assert_eq!(
            restart_delay_after_server_exit(&mut recent_restarts, false, start),
            Some(SERVER_RESTART_DELAY)
        );
    }

    #[test]
    fn unexpected_restarts_trigger_crash_cooldown() {
        let mut recent_restarts = VecDeque::new();
        let start = Instant::now();

        for offset in 0..SERVER_RESTART_BURST_LIMIT - 1 {
            assert_eq!(
                restart_delay_after_server_exit(
                    &mut recent_restarts,
                    false,
                    start + Duration::from_secs(offset as u64)
                ),
                Some(SERVER_RESTART_DELAY)
            );
        }

        assert_eq!(
            restart_delay_after_server_exit(
                &mut recent_restarts,
                false,
                start + Duration::from_secs(SERVER_RESTART_BURST_LIMIT as u64)
            ),
            Some(SERVER_RESTART_COOLDOWN)
        );
        assert!(recent_restarts.is_empty());
    }
}
