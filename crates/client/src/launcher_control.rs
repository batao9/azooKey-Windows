use anyhow::{Context as _, Result};
use std::time::{Duration, Instant};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::windows::named_pipe::{ClientOptions, NamedPipeClient},
    time,
};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_PIPE_BUSY};

const LAUNCHER_PIPE_PATH: &str = r"\\.\pipe\azookey_launcher";
const LAUNCHER_RESTART_COMMAND: &[u8] = b"restart-server\n";
const LAUNCHER_CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
const LAUNCHER_RETRY_INTERVAL: Duration = Duration::from_millis(50);
const LAUNCHER_RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) fn request_restart() -> Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let mut client = open_launcher_pipe()
            .await?
            .context("Launcher restart pipe is not available")?;

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
        parse_launcher_response(&response[..size])
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
            Err(error) => return Err(error).context("Failed to connect launcher restart pipe"),
        }
    }
}

fn launcher_pipe_missing(raw_os_error: Option<i32>) -> bool {
    raw_os_error == Some(ERROR_FILE_NOT_FOUND.0 as i32)
        || raw_os_error == Some(ERROR_PATH_NOT_FOUND.0 as i32)
}

pub(crate) fn parse_launcher_response(bytes: &[u8]) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::parse_launcher_response;

    #[test]
    fn launcher_restart_response_is_strictly_validated() {
        assert!(parse_launcher_response(b"ok\n").is_ok());
        assert!(parse_launcher_response(b"error: restart throttled\n").is_err());
        assert!(parse_launcher_response(b"unexpected\n").is_err());
    }
}
