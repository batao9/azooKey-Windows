use anyhow::{Context as _, Result};
use shared::AppConfig;
use std::{
    ffi::OsString,
    os::windows::ffi::OsStringExt as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::windows::named_pipe::{ClientOptions, NamedPipeClient},
    time,
};
use windows::{
    core::PWSTR,
    Win32::{
        Foundation::{
            CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_PIPE_BUSY,
            WAIT_OBJECT_0, WAIT_TIMEOUT,
        },
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
                TH32CS_SNAPPROCESS,
            },
            Threading::{
                OpenProcess, QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject,
                PROCESS_ACCESS_RIGHTS, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
                PROCESS_TERMINATE,
            },
        },
    },
};

const SERVER_EXE_NAME: &str = "azookey-server.exe";
const LAUNCHER_PIPE_PATH: &str = r"\\.\pipe\azookey_launcher";
const LAUNCHER_RESTART_COMMAND: &[u8] = b"restart-server\n";
const LAUNCHER_CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
const LAUNCHER_RETRY_INTERVAL: Duration = Duration::from_millis(50);
const LAUNCHER_RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);
const PROCESS_SYNCHRONIZE: PROCESS_ACCESS_RIGHTS = PROCESS_ACCESS_RIGHTS(0x0010_0000);
const SERVER_EXIT_TIMEOUT_MS: u32 = 5_000;
const WATCHDOG_RESTART_WAIT: Duration = Duration::from_millis(2_500);

pub fn restart_server(config: &AppConfig) -> Result<()> {
    if request_launcher_restart()? {
        return Ok(());
    }

    restart_server_direct(config)
}

fn restart_server_direct(config: &AppConfig) -> Result<()> {
    let server_path = resolve_server_path()?;
    let target = normalize_path(&server_path);

    let process_ids = matching_server_process_ids(&target)?;
    for process_id in process_ids {
        terminate_process(process_id)
            .with_context(|| format!("Failed to terminate {SERVER_EXE_NAME} pid={process_id}"))?;
    }

    // Give launcher watchdog a chance to restart its own child first. If the
    // settings app is running without launcher, fall back to starting the server.
    thread::sleep(WATCHDOG_RESTART_WAIT);

    if matching_server_process_ids(&target)?.is_empty() {
        start_server(&server_path, config)?;
    }

    Ok(())
}

fn request_launcher_restart() -> Result<bool> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let Some(mut client) = open_launcher_pipe().await? else {
            return Ok(false);
        };

        client
            .write_all(LAUNCHER_RESTART_COMMAND)
            .await
            .context("Failed to write launcher restart request")?;
        client
            .flush()
            .await
            .context("Failed to flush launcher restart request")?;

        let mut response = [0u8; 512];
        let size = time::timeout(LAUNCHER_RESPONSE_TIMEOUT, client.read(&mut response))
            .await
            .context("Timed out waiting for launcher restart response")?
            .context("Failed to read launcher restart response")?;
        parse_launcher_response(&response[..size])?;

        Ok(true)
    })
}

async fn open_launcher_pipe() -> Result<Option<NamedPipeClient>> {
    let started_at = Instant::now();

    loop {
        match ClientOptions::new().open(LAUNCHER_PIPE_PATH) {
            Ok(client) => return Ok(Some(client)),
            Err(error) if launcher_pipe_missing(error.raw_os_error()) => return Ok(None),
            Err(error)
                if error.raw_os_error() == Some(ERROR_PIPE_BUSY.0 as i32)
                    && started_at.elapsed() < LAUNCHER_CONNECT_TIMEOUT =>
            {
                time::sleep(LAUNCHER_RETRY_INTERVAL).await;
            }
            Err(error) => {
                return Err(error).context("Failed to connect launcher restart pipe");
            }
        }
    }
}

fn launcher_pipe_missing(raw_os_error: Option<i32>) -> bool {
    raw_os_error == Some(ERROR_FILE_NOT_FOUND.0 as i32)
        || raw_os_error == Some(ERROR_PATH_NOT_FOUND.0 as i32)
}

fn parse_launcher_response(bytes: &[u8]) -> Result<()> {
    let response = std::str::from_utf8(bytes)
        .context("Launcher restart response is not UTF-8")?
        .trim();

    if response == "ok" {
        return Ok(());
    }

    if let Some(message) = response.strip_prefix("error:") {
        anyhow::bail!("Launcher failed to restart server: {}", message.trim());
    }

    anyhow::bail!("Unexpected launcher restart response: {response}");
}

fn resolve_server_path() -> Result<PathBuf> {
    let current_exe = std::env::current_exe()?;
    let install_dir = current_exe
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .context("Failed to resolve settings app directory")?;
    Ok(install_dir.join(SERVER_EXE_NAME))
}

fn matching_server_process_ids(target: &str) -> Result<Vec<u32>> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)? };
    let _snapshot_guard = HandleGuard(snapshot);

    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };

    let mut process_ids = Vec::new();
    let mut has_entry = unsafe { Process32FirstW(snapshot, &mut entry).is_ok() };

    while has_entry {
        if wide_null_terminated_to_string(&entry.szExeFile).eq_ignore_ascii_case(SERVER_EXE_NAME) {
            let process_id = entry.th32ProcessID;
            if process_image_path(process_id)
                .map(|path| normalize_path(&path) == target)
                .unwrap_or(false)
            {
                process_ids.push(process_id);
            }
        }

        has_entry = unsafe { Process32NextW(snapshot, &mut entry).is_ok() };
    }

    Ok(process_ids)
}

fn terminate_process(process_id: u32) -> Result<()> {
    let access = PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE;
    let process = unsafe { OpenProcess(access, false, process_id)? };
    let _process_guard = HandleGuard(process);

    unsafe {
        TerminateProcess(process, 0)?;
        match WaitForSingleObject(process, SERVER_EXIT_TIMEOUT_MS) {
            WAIT_OBJECT_0 => Ok(()),
            WAIT_TIMEOUT => anyhow::bail!("Timed out waiting for process exit"),
            event => anyhow::bail!("Unexpected wait result: {:?}", event),
        }
    }
}

fn process_image_path(process_id: u32) -> Option<PathBuf> {
    let process =
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id).ok()? };
    let _process_guard = HandleGuard(process);

    let mut buffer = vec![0u16; 32_768];
    let mut size = buffer.len() as u32;
    unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut size,
        )
        .ok()?;
    }

    let size = usize::try_from(size).ok()?;
    Some(PathBuf::from(OsString::from_wide(&buffer[..size])))
}

fn start_server(server_path: &Path, config: &AppConfig) -> Result<()> {
    if !server_path.is_file() {
        anyhow::bail!("Server executable not found: {}", server_path.display());
    }

    let install_dir = server_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .context("Server executable directory not found")?;
    let backend_path = install_dir.join(backend_dir(config));
    let path = prepend_to_path(&backend_path);

    Command::new(server_path)
        .current_dir(install_dir)
        .env("AZOOKEY_ZENZAI_CPU_SUPPORTED", zenzai_cpu_supported_env())
        .env("PATH", path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("Failed to start {}", server_path.display()))?;

    Ok(())
}

fn backend_dir(config: &AppConfig) -> &'static str {
    match config.zenzai.backend.as_str() {
        "cuda" => "llama_cuda",
        "vulkan" => "llama_vulkan",
        _ => "llama_cpu",
    }
}

fn zenzai_cpu_supported_env() -> &'static str {
    if shared::zenzai_cpu_backend_supported() {
        "1"
    } else {
        "0"
    }
}

fn prepend_to_path(path: &Path) -> String {
    let existing = std::env::var("PATH").unwrap_or_default();
    format!("{};{}", path.to_string_lossy(), existing)
}

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('/', r"\")
        .to_ascii_lowercase()
}

fn wide_null_terminated_to_string(value: &[u16]) -> String {
    let len = value
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(value.len());
    OsString::from_wide(&value[..len])
        .to_string_lossy()
        .into_owned()
}

struct HandleGuard(windows::Win32::Foundation::HANDLE);

impl Drop for HandleGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize_path, parse_launcher_response, wide_null_terminated_to_string};
    use std::{ffi::OsStr, os::windows::ffi::OsStrExt as _, path::Path};

    #[test]
    fn normalize_path_ignores_case_and_extended_prefix() {
        assert_eq!(
            normalize_path(Path::new(r"\\?\C:\Azookey/azookey-server.exe")),
            r"c:\azookey\azookey-server.exe"
        );
    }

    #[test]
    fn wide_null_terminated_to_string_stops_at_null() {
        let mut wide: Vec<u16> = OsStr::new("azookey-server.exe").encode_wide().collect();
        wide.push(0);
        wide.extend(OsStr::new("ignored").encode_wide());

        assert_eq!(wide_null_terminated_to_string(&wide), "azookey-server.exe");
    }

    #[test]
    fn parse_launcher_response_accepts_ok() {
        parse_launcher_response(b"ok\n").unwrap();
    }

    #[test]
    fn parse_launcher_response_rejects_launcher_error() {
        let error = parse_launcher_response(b"error:denied\n").unwrap_err();
        assert!(error.to_string().contains("denied"));
    }
}
