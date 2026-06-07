use shared::{zenzai_cpu_backend_supported, AppConfig};
use std::collections::VecDeque;
use std::ffi::c_void;
use std::io::{BufRead, BufReader};
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
            GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
            TOKEN_USER,
        },
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    },
};

const SERVER_RESTART_DELAY: Duration = Duration::from_secs(1);
const SERVER_RESTART_WINDOW: Duration = Duration::from_secs(60);
const SERVER_RESTART_BURST_LIMIT: usize = 5;
const SERVER_RESTART_COOLDOWN: Duration = Duration::from_secs(30);
const SERVER_WATCH_POLL_INTERVAL: Duration = Duration::from_millis(100);
const LAUNCHER_PIPE_PATH: &str = r"\\.\pipe\azookey_launcher";
const LAUNCHER_RESTART_COMMAND: &str = "restart-server";
const LAUNCHER_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

fn main() -> anyhow::Result<()> {
    let cpu_backend_supported = zenzai_cpu_backend_supported();
    env::set_var(
        "AZOOKEY_ZENZAI_CPU_SUPPORTED",
        if cpu_backend_supported { "1" } else { "0" },
    );

    let exe_path = env::current_exe()?.parent().unwrap().to_path_buf();
    let (command_tx, command_rx) = mpsc::channel();
    start_launcher_command_listener(command_tx);

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

    let mut command = process_command_with_backend(install_dir, "azookey-server.exe", &config);
    command.env(
        "AZOOKEY_ZENZAI_CPU_SUPPORTED",
        if cpu_backend_supported { "1" } else { "0" },
    );

    spawn_process(command, "azookey-server.exe", "[server]")
}

fn start_ui_process(install_dir: &Path) -> anyhow::Result<Child> {
    let config = load_config();
    let command = process_command_with_backend(install_dir, "ui.exe", &config);
    spawn_process(command, "ui.exe", "[ui]")
}

fn process_command_with_backend(install_dir: &Path, exe: &str, config: &AppConfig) -> Command {
    let backend_path = install_dir.join(backend_dir(config));
    let mut command = process_command(install_dir, exe);
    command.env("PATH", prepend_to_path(&backend_path));
    command
}

fn process_command(install_dir: &Path, exe: &str) -> Command {
    let exe_path = install_dir.join(exe);
    let mut command = if exe_path.is_file() {
        Command::new(&exe_path)
    } else {
        Command::new(exe)
    };
    command.current_dir(install_dir);
    command
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
            for line in stdout_reader.lines() {
                if let Ok(line) = line {
                    println!("{}: {}", prefix_stdout, line);
                }
            }
        });
    }

    if let Some(stderr) = child.stderr.take() {
        let stderr_reader = BufReader::new(stderr);
        let prefix_stderr = prefix.to_string();
        thread::spawn(move || {
            for line in stderr_reader.lines() {
                if let Ok(line) = line {
                    eprintln!("{}: {}", prefix_stderr, line);
                }
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

fn start_launcher_command_listener(command_tx: Sender<LauncherCommand>) {
    thread::spawn(move || {
        if let Err(error) = run_launcher_command_listener(command_tx) {
            eprintln!("[launcher] command listener stopped: {error:?}");
        }
    });
}

fn run_launcher_command_listener(command_tx: Sender<LauncherCommand>) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let security_descriptor = create_launcher_pipe_security_descriptor()?;

        let mut security_attributes = UnsafeSecurityAttributes(SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: security_descriptor.0,
            bInheritHandle: false.into(),
        });

        let mut first_pipe_instance = true;
        loop {
            let mut pipe =
                create_launcher_command_pipe(&mut security_attributes, first_pipe_instance)?;
            first_pipe_instance = false;
            pipe.connect()
                .await
                .context("Failed to connect launcher command pipe")?;

            if let Err(error) = handle_launcher_command(&mut pipe, &command_tx).await {
                eprintln!("[launcher] command failed: {error:?}");
            }
        }
    })
}

fn create_launcher_pipe_security_descriptor() -> anyhow::Result<PSECURITY_DESCRIPTOR> {
    let user_sid = current_user_sid_string()?;
    let sddl = launcher_pipe_sddl(&user_sid);
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

fn launcher_pipe_sddl(user_sid: &str) -> String {
    format!("D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;{user_sid})S:(ML;;NW;;;ME)")
}

fn current_user_sid_string() -> anyhow::Result<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)
            .context("Failed to open current process token")?;

        let result = user_sid_string_from_token(token);
        let _ = CloseHandle(token);
        result
    }
}

fn user_sid_string_from_token(token: HANDLE) -> anyhow::Result<String> {
    unsafe {
        let mut token_info_length = 0;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut token_info_length);
        anyhow::ensure!(
            token_info_length > 0,
            "Failed to get current user token SID size"
        );

        let mut token_info = vec![0u8; token_info_length as usize];
        GetTokenInformation(
            token,
            TokenUser,
            Some(token_info.as_mut_ptr().cast()),
            token_info_length,
            &mut token_info_length,
        )
        .context("Failed to get current user token SID")?;

        let token_user = &*(token_info.as_ptr() as *const TOKEN_USER);
        let mut sid_string = PWSTR::null();
        ConvertSidToStringSidW(token_user.User.Sid, &mut sid_string)
            .context("Failed to convert current user SID to string")?;

        let result = sid_string
            .to_string()
            .context("Failed to decode current user SID string");
        let _ = LocalFree(HLOCAL(sid_string.as_ptr().cast()));
        result
    }
}

fn create_launcher_command_pipe(
    security_attributes: &mut UnsafeSecurityAttributes,
    first_pipe_instance: bool,
) -> anyhow::Result<NamedPipeServer> {
    let mut options = ServerOptions::new();
    if first_pipe_instance {
        options.first_pipe_instance(true);
    }

    unsafe {
        options
            .create_with_security_attributes_raw(
                LAUNCHER_PIPE_PATH,
                addr_of_mut!(security_attributes.0) as *mut c_void,
            )
            .context("Failed to create launcher command pipe")
    }
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

struct UnsafeSecurityAttributes(SECURITY_ATTRIBUTES);

unsafe impl Send for UnsafeSecurityAttributes {}
unsafe impl Sync for UnsafeSecurityAttributes {}

#[cfg(test)]
mod tests {
    use super::{
        launcher_pipe_sddl, launcher_response, parse_launcher_command,
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
    fn launcher_pipe_sddl_is_limited_to_current_user_and_medium_integrity() {
        let sddl = launcher_pipe_sddl("S-1-5-21-1-2-3-1001");

        assert!(sddl.contains("(A;;GA;;;SY)"));
        assert!(sddl.contains("(A;;GA;;;BA)"));
        assert!(sddl.contains("(A;;GA;;;S-1-5-21-1-2-3-1001)"));
        assert!(sddl.contains("S:(ML;;NW;;;ME)"));
        assert!(!sddl.contains(";;;BU)"));
        assert!(!sddl.contains(";;;AC)"));
        assert!(!sddl.contains(";;;RC)"));
        assert!(!sddl.contains(";;;LW)"));
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
