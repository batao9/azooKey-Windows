use anyhow::Result;
use hyper_util::rt::TokioIo;
use shared::proto::azookey_service_client::AzookeyServiceClient;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{net::windows::named_pipe::ClientOptions, time};
use tonic::transport::Endpoint;
use tower::service_fn;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_PIPE_BUSY};

const SERVER_PIPE_PATH: &str = r"\\.\pipe\azookey_server";
const IPC_RETRY_INTERVAL: Duration = Duration::from_millis(50);

// connect to kkc server
#[derive(Debug, Clone)]
pub struct IPCService {
    // kkc server client
    azookey_client: AzookeyServiceClient<tonic::transport::channel::Channel>,
    runtime: Arc<tokio::runtime::Runtime>,
}

impl IPCService {
    pub fn new() -> Result<Self> {
        Self::new_inner(None)
    }

    #[cfg(test)]
    pub(crate) fn new_for_test() -> Self {
        let runtime = tokio::runtime::Runtime::new().expect("test runtime should be created");
        let server_channel = {
            let _runtime_guard = runtime.enter();
            Endpoint::from_static("http://[::]:50051").connect_lazy()
        };
        let azookey_client = AzookeyServiceClient::new(server_channel);

        Self {
            azookey_client,
            runtime: Arc::new(runtime),
        }
    }

    pub fn new_with_timeout(timeout: Duration) -> Result<Self> {
        Self::new_inner(Some(timeout))
    }

    fn new_inner(timeout: Option<Duration>) -> Result<Self> {
        let runtime = tokio::runtime::Runtime::new()?;

        let server_channel = runtime.block_on(
            Endpoint::try_from("http://[::]:50051")?.connect_with_connector(service_fn(
                move |_| async move {
                    let started_at = Instant::now();
                    let client = loop {
                        match ClientOptions::new().open(SERVER_PIPE_PATH) {
                            Ok(client) => break client,
                            Err(e)
                                if should_retry_pipe_connect_error(
                                    e.raw_os_error(),
                                    started_at,
                                    timeout,
                                ) => {}
                            Err(e) => return Err(e),
                        }

                        time::sleep(IPC_RETRY_INTERVAL).await;
                    };

                    Ok::<_, std::io::Error>(TokioIo::new(client))
                },
            )),
        )?;

        let azookey_client = AzookeyServiceClient::new(server_channel);

        Ok(Self {
            azookey_client,
            runtime: Arc::new(runtime),
        })
    }
}

fn should_retry_pipe_connect_error(
    raw_os_error: Option<i32>,
    started_at: Instant,
    timeout: Option<Duration>,
) -> bool {
    match timeout {
        Some(timeout) if started_at.elapsed() >= timeout => return false,
        Some(_) => {}
        None => return raw_os_error == Some(ERROR_PIPE_BUSY.0 as i32),
    }

    raw_os_error == Some(ERROR_FILE_NOT_FOUND.0 as i32)
        || raw_os_error == Some(ERROR_PATH_NOT_FOUND.0 as i32)
        || raw_os_error == Some(ERROR_PIPE_BUSY.0 as i32)
}

// implement methods to interact with kkc server
impl IPCService {
    pub fn update_config(&mut self) -> anyhow::Result<()> {
        let request = tonic::Request::new(shared::proto::UpdateConfigRequest { request_id: 0 });
        self.runtime
            .clone()
            .block_on(self.azookey_client.update_config(request))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        should_retry_pipe_connect_error, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND,
        ERROR_PIPE_BUSY,
    };
    use std::time::{Duration, Instant};

    #[test]
    fn retries_missing_and_busy_pipe_errors_before_timeout() {
        let started_at = Instant::now();
        let timeout = Some(Duration::from_secs(10));

        assert!(should_retry_pipe_connect_error(
            Some(ERROR_FILE_NOT_FOUND.0 as i32),
            started_at,
            timeout
        ));
        assert!(should_retry_pipe_connect_error(
            Some(ERROR_PATH_NOT_FOUND.0 as i32),
            started_at,
            timeout
        ));
        assert!(should_retry_pipe_connect_error(
            Some(ERROR_PIPE_BUSY.0 as i32),
            started_at,
            timeout
        ));
    }

    #[test]
    fn stops_retrying_busy_pipe_after_timeout() {
        let timeout = Duration::from_secs(10);
        let started_at = Instant::now() - timeout;

        assert!(!should_retry_pipe_connect_error(
            Some(ERROR_PIPE_BUSY.0 as i32),
            started_at,
            Some(timeout)
        ));
    }

    #[test]
    fn retries_busy_pipe_without_timeout() {
        let started_at = Instant::now();

        assert!(should_retry_pipe_connect_error(
            Some(ERROR_PIPE_BUSY.0 as i32),
            started_at,
            None
        ));
    }

    #[test]
    fn does_not_retry_missing_pipe_without_timeout() {
        let started_at = Instant::now();

        assert!(!should_retry_pipe_connect_error(
            Some(ERROR_FILE_NOT_FOUND.0 as i32),
            started_at,
            None
        ));
        assert!(!should_retry_pipe_connect_error(
            Some(ERROR_PATH_NOT_FOUND.0 as i32),
            started_at,
            None
        ));
    }

    #[test]
    fn does_not_retry_other_pipe_errors() {
        let started_at = Instant::now();
        let timeout = Some(Duration::from_secs(10));

        assert!(!should_retry_pipe_connect_error(
            Some(5),
            started_at,
            timeout
        ));
        assert!(!should_retry_pipe_connect_error(None, started_at, timeout));
    }
}
