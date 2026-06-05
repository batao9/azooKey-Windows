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
        Self::new_with_timeout(Duration::ZERO)
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
        let runtime = tokio::runtime::Runtime::new()?;

        let server_channel = runtime.block_on(
            Endpoint::try_from("http://[::]:50051")?.connect_with_connector(service_fn(
                move |_| async move {
                    let started_at = Instant::now();
                    let client = loop {
                        match ClientOptions::new().open(SERVER_PIPE_PATH) {
                            Ok(client) => break client,
                            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY.0 as i32) => (),
                            Err(e)
                                if should_retry_missing_pipe(
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

fn should_retry_missing_pipe(
    raw_os_error: Option<i32>,
    started_at: Instant,
    timeout: Duration,
) -> bool {
    if started_at.elapsed() >= timeout {
        return false;
    }

    raw_os_error == Some(ERROR_FILE_NOT_FOUND.0 as i32)
        || raw_os_error == Some(ERROR_PATH_NOT_FOUND.0 as i32)
}

// implement methods to interact with kkc server
impl IPCService {
    pub fn update_config(&mut self) -> anyhow::Result<()> {
        let request = tonic::Request::new(shared::proto::UpdateConfigRequest {});
        self.runtime
            .clone()
            .block_on(self.azookey_client.update_config(request))?;

        Ok(())
    }
}
