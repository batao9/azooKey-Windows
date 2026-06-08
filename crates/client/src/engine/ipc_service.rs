use anyhow::Result;
use hyper_util::rt::TokioIo;
use shared::{
    proto::{
        azookey_service_client::AzookeyServiceClient, window_service_client::WindowServiceClient,
        PerformanceLogRequest,
    },
    AppConfig,
};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};
use tokio::{net::windows::named_pipe::ClientOptions, time};
use tonic::transport::Endpoint;
use tower::service_fn;
use windows::Win32::Foundation::ERROR_PIPE_BUSY;

const INPUT_STYLE_ROMAN2KANA: i32 = 0;
const INPUT_STYLE_DIRECT: i32 = 1;
const CLIENT_LOG_CONFIG_REFRESH_INTERVAL: Duration = Duration::from_secs(1);

static CLIENT_REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static CLIENT_LOG_CONFIG_CACHE: OnceLock<Mutex<ClientLogConfigCache>> = OnceLock::new();

#[derive(Debug, Default)]
struct ClientLogConfigCache {
    last_checked: Option<Instant>,
    enabled: bool,
}

// connect to kkc server
#[derive(Debug, Clone)]
pub struct IPCService {
    // kkc server client
    azookey_client: AzookeyServiceClient<tonic::transport::channel::Channel>,
    // candidate window server client
    window_client: WindowServiceClient<tonic::transport::channel::Channel>,
    runtime: Arc<tokio::runtime::Runtime>,
    performance_log_tx: tokio::sync::mpsc::UnboundedSender<PerformanceLogRequest>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Candidates {
    pub texts: Vec<String>,
    pub sub_texts: Vec<String>,
    pub hiragana: String,
    pub corresponding_count: Vec<i32>,
}

fn next_request_id() -> u64 {
    let counter = CLIENT_REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    (u64::from(std::process::id()) << 32) | (counter & 0xffff_ffff)
}

fn client_log_config_cache() -> &'static Mutex<ClientLogConfigCache> {
    CLIENT_LOG_CONFIG_CACHE.get_or_init(|| Mutex::new(ClientLogConfigCache::default()))
}

fn client_performance_log_enabled() -> bool {
    let Ok(mut cache) = client_log_config_cache().lock() else {
        return false;
    };

    let should_refresh = cache
        .last_checked
        .map(|last_checked| last_checked.elapsed() >= CLIENT_LOG_CONFIG_REFRESH_INTERVAL)
        .unwrap_or(true);
    if should_refresh {
        cache.enabled = AppConfig::read()
            .map(|config| config.debug.server_log_enabled)
            .unwrap_or(false);
        cache.last_checked = Some(Instant::now());
    }

    cache.enabled
}

fn duration_millis_u64(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

impl IPCService {
    pub fn new() -> Result<Self> {
        let runtime = Arc::new(tokio::runtime::Runtime::new()?);

        let server_channel = runtime.block_on(
            Endpoint::try_from("http://[::]:50051")?.connect_with_connector(service_fn(
                |_| async {
                    let client = loop {
                        match ClientOptions::new().open(r"\\.\pipe\azookey_server") {
                            Ok(client) => break client,
                            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY.0 as i32) => (),
                            Err(e) => return Err(e),
                        }

                        time::sleep(Duration::from_millis(50)).await;
                    };

                    Ok::<_, std::io::Error>(TokioIo::new(client))
                },
            )),
        )?;

        let ui_channel = runtime.block_on(
            Endpoint::try_from("http://[::]:50052")?.connect_with_connector(service_fn(
                |_| async {
                    let client = loop {
                        match ClientOptions::new().open(r"\\.\pipe\azookey_ui") {
                            Ok(client) => break client,
                            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY.0 as i32) => (),
                            Err(e) => return Err(e),
                        }

                        time::sleep(Duration::from_millis(50)).await;
                    };

                    Ok::<_, std::io::Error>(TokioIo::new(client))
                },
            )),
        )?;

        let azookey_client = AzookeyServiceClient::new(server_channel);
        let window_client = WindowServiceClient::new(ui_channel);
        let (performance_log_tx, mut performance_log_rx) =
            tokio::sync::mpsc::unbounded_channel::<PerformanceLogRequest>();
        let mut performance_log_client = azookey_client.clone();
        runtime.spawn(async move {
            while let Some(request) = performance_log_rx.recv().await {
                if let Err(error) = performance_log_client
                    .log_performance(tonic::Request::new(request))
                    .await
                {
                    tracing::debug!("failed to write client performance log: {error:?}");
                }
            }
        });
        tracing::debug!("Connected to server: {:?}", azookey_client);

        Ok(Self {
            azookey_client,
            window_client,
            runtime,
            performance_log_tx,
        })
    }
}

// implement methods to interact with kkc server
impl IPCService {
    fn candidates_from_composing_text(
        composing_text: Option<shared::proto::ComposingText>,
    ) -> anyhow::Result<Candidates> {
        if let Some(composing_text) = composing_text {
            Ok(Candidates {
                texts: composing_text
                    .suggestions
                    .iter()
                    .map(|s| s.text.clone())
                    .collect(),
                sub_texts: composing_text
                    .suggestions
                    .iter()
                    .map(|s| s.subtext.clone())
                    .collect(),
                hiragana: composing_text.hiragana,
                corresponding_count: composing_text
                    .suggestions
                    .iter()
                    .map(|s| s.corresponding_count)
                    .collect(),
            })
        } else {
            anyhow::bail!("composing_text is None");
        }
    }

    fn reconnect(&mut self) -> anyhow::Result<()> {
        let refreshed = Self::new()?;
        self.azookey_client = refreshed.azookey_client;
        self.window_client = refreshed.window_client;
        self.runtime = refreshed.runtime;
        self.performance_log_tx = refreshed.performance_log_tx;
        Ok(())
    }

    fn log_client_performance(
        &mut self,
        request_id: u64,
        operation: &str,
        stage: &str,
        elapsed: Duration,
        details: String,
    ) {
        if !client_performance_log_enabled() {
            return;
        }

        let request = PerformanceLogRequest {
            request_id,
            component: "ime".to_string(),
            operation: operation.to_string(),
            stage: stage.to_string(),
            elapsed_ms: duration_millis_u64(elapsed),
            details,
        };

        if let Err(error) = self.performance_log_tx.send(request) {
            tracing::debug!("failed to enqueue client performance log: {error:?}");
        }
    }

    #[tracing::instrument]
    pub fn append_text(&mut self, text: String) -> anyhow::Result<Candidates> {
        self.append_text_with_style(text, INPUT_STYLE_ROMAN2KANA)
    }

    #[tracing::instrument]
    pub fn append_text_with_context(
        &mut self,
        text: String,
        previous_candidates: &Candidates,
    ) -> anyhow::Result<Candidates> {
        self.append_text_with_style_and_context(
            text,
            INPUT_STYLE_ROMAN2KANA,
            Some(previous_candidates),
        )
    }

    #[tracing::instrument]
    pub fn append_text_direct(&mut self, text: String) -> anyhow::Result<Candidates> {
        self.append_text_with_style(text, INPUT_STYLE_DIRECT)
    }

    #[tracing::instrument]
    pub fn append_text_direct_with_context(
        &mut self,
        text: String,
        previous_candidates: &Candidates,
    ) -> anyhow::Result<Candidates> {
        self.append_text_with_style_and_context(text, INPUT_STYLE_DIRECT, Some(previous_candidates))
    }

    #[tracing::instrument]
    fn append_text_with_style(
        &mut self,
        text: String,
        input_style: i32,
    ) -> anyhow::Result<Candidates> {
        self.append_text_with_style_and_context(text, input_style, None)
    }

    #[inline]
    fn should_retry_append_after_refresh(
        previous_candidates: Option<&Candidates>,
        refreshed_candidates: &Candidates,
    ) -> bool {
        previous_candidates.is_some_and(|previous| previous == refreshed_candidates)
    }

    #[tracing::instrument]
    fn append_text_with_style_and_context(
        &mut self,
        text: String,
        input_style: i32,
        previous_candidates: Option<&Candidates>,
    ) -> anyhow::Result<Candidates> {
        let request_id = next_request_id();
        let operation_start = Instant::now();
        let input_len = text.chars().count();
        let send = |this: &mut Self| -> anyhow::Result<
            tonic::Response<shared::proto::AppendTextResponse>,
        > {
            let request = tonic::Request::new(shared::proto::AppendTextRequest {
                text_to_append: text.clone(),
                input_style,
                request_id,
            });

            let response = this
                .runtime
                .clone()
                .block_on(this.azookey_client.append_text(request))?;
            Ok(response)
        };

        let response = match send(self) {
            Ok(response) => response,
            Err(first_error) => {
                tracing::warn!(
                    "append_text first attempt failed (style={input_style}, text_len={}), reconnecting IPC: {first_error:?}",
                    text.chars().count()
                );

                match self.reconnect() {
                    Ok(()) => {
                        tracing::info!(
                            "append_text IPC reconnect succeeded (style={input_style}), refreshing current composition"
                        );
                    }
                    Err(reconnect_error) => {
                        tracing::error!(
                            "append_text IPC reconnect failed (style={input_style}): {reconnect_error:?}"
                        );
                        self.log_client_performance(
                            request_id,
                            "append_text",
                            "rpc_total",
                            operation_start.elapsed(),
                            format!(
                                "status=error;phase=reconnect;input_len={input_len};input_style={input_style}"
                            ),
                        );
                        return Err(reconnect_error);
                    }
                }

                match self.move_cursor(0) {
                    Ok(candidates) => {
                        if Self::should_retry_append_after_refresh(previous_candidates, &candidates)
                        {
                            tracing::warn!(
                                "append_text recovered unchanged composition after reconnect (style={input_style}), retrying original input"
                            );
                            let retry_response = send(self)?;
                            let candidates = Self::candidates_from_composing_text(
                                retry_response.into_inner().composing_text,
                            )?;
                            self.log_client_performance(
                                request_id,
                                "append_text",
                                "rpc_total",
                                operation_start.elapsed(),
                                format!(
                                    "status=success;retry=true;input_len={input_len};input_style={input_style}"
                                ),
                            );
                            return Ok(candidates);
                        }

                        tracing::info!(
                            "append_text recovered changed composition after reconnect (style={input_style}), reusing server state"
                        );
                        self.log_client_performance(
                            request_id,
                            "append_text",
                            "rpc_total",
                            operation_start.elapsed(),
                            format!(
                                "status=recovered_changed;input_len={input_len};input_style={input_style}"
                            ),
                        );
                        return Ok(candidates);
                    }
                    Err(refresh_error) => {
                        tracing::error!(
                            "append_text refresh failed after reconnect (style={input_style}): {refresh_error:?}"
                        );
                        self.log_client_performance(
                            request_id,
                            "append_text",
                            "rpc_total",
                            operation_start.elapsed(),
                            format!(
                                "status=error;phase=refresh;input_len={input_len};input_style={input_style}"
                            ),
                        );
                        return Err(refresh_error);
                    }
                }
            }
        };
        let candidates =
            Self::candidates_from_composing_text(response.into_inner().composing_text)?;
        self.log_client_performance(
            request_id,
            "append_text",
            "rpc_total",
            operation_start.elapsed(),
            format!("status=success;input_len={input_len};input_style={input_style}"),
        );
        Ok(candidates)
    }

    #[tracing::instrument]
    pub fn remove_text(&mut self) -> anyhow::Result<Candidates> {
        let request_id = next_request_id();
        let operation_start = Instant::now();
        let request = tonic::Request::new(shared::proto::RemoveTextRequest { request_id });
        let response = self
            .runtime
            .clone()
            .block_on(self.azookey_client.remove_text(request))?;
        let candidates =
            Self::candidates_from_composing_text(response.into_inner().composing_text)?;
        self.log_client_performance(
            request_id,
            "remove_text",
            "rpc_total",
            operation_start.elapsed(),
            "status=success".to_string(),
        );
        Ok(candidates)
    }

    #[tracing::instrument]
    pub fn clear_text(&mut self) -> anyhow::Result<()> {
        let request_id = next_request_id();
        let operation_start = Instant::now();
        let request = tonic::Request::new(shared::proto::ClearTextRequest { request_id });
        let _response = self
            .runtime
            .clone()
            .block_on(self.azookey_client.clear_text(request))?;
        self.log_client_performance(
            request_id,
            "clear_text",
            "rpc_total",
            operation_start.elapsed(),
            "status=success".to_string(),
        );

        Ok(())
    }

    #[tracing::instrument]
    pub fn shrink_text(&mut self, offset: i32) -> anyhow::Result<Candidates> {
        let request_id = next_request_id();
        let operation_start = Instant::now();
        let request = tonic::Request::new(shared::proto::ShrinkTextRequest { offset, request_id });
        let response = self
            .runtime
            .clone()
            .block_on(self.azookey_client.shrink_text(request))?;
        let candidates =
            Self::candidates_from_composing_text(response.into_inner().composing_text)?;
        self.log_client_performance(
            request_id,
            "shrink_text",
            "rpc_total",
            operation_start.elapsed(),
            format!("status=success;offset={offset}"),
        );
        Ok(candidates)
    }

    #[tracing::instrument]
    pub fn move_cursor(&mut self, offset: i32) -> anyhow::Result<Candidates> {
        let request_id = next_request_id();
        let operation_start = Instant::now();
        let request = tonic::Request::new(shared::proto::MoveCursorRequest { offset, request_id });
        let response = self
            .runtime
            .clone()
            .block_on(self.azookey_client.move_cursor(request))?;
        let candidates =
            Self::candidates_from_composing_text(response.into_inner().composing_text)?;
        self.log_client_performance(
            request_id,
            "move_cursor",
            "rpc_total",
            operation_start.elapsed(),
            format!("status=success;offset={offset}"),
        );
        Ok(candidates)
    }

    pub fn set_context(&mut self, context: String) -> anyhow::Result<()> {
        let request_id = next_request_id();
        let operation_start = Instant::now();
        let context_len = context.chars().count();
        let request = tonic::Request::new(shared::proto::SetContextRequest {
            context,
            request_id,
        });
        let _response = self
            .runtime
            .clone()
            .block_on(self.azookey_client.set_context(request))?;
        self.log_client_performance(
            request_id,
            "set_context",
            "rpc_total",
            operation_start.elapsed(),
            format!("status=success;context_len={context_len}"),
        );

        Ok(())
    }
}

// implement methods to interact with candidate window server
impl IPCService {
    #[tracing::instrument]
    pub fn show_window(&mut self) -> anyhow::Result<()> {
        let request = tonic::Request::new(shared::proto::EmptyResponse {});
        self.runtime
            .clone()
            .block_on(self.window_client.show_window(request))?;

        Ok(())
    }

    #[tracing::instrument]
    pub fn hide_window(&mut self) -> anyhow::Result<()> {
        let request = tonic::Request::new(shared::proto::EmptyResponse {});
        self.runtime
            .clone()
            .block_on(self.window_client.hide_window(request))?;

        Ok(())
    }

    #[tracing::instrument]
    pub fn set_window_position(
        &mut self,
        top: i32,
        left: i32,
        bottom: i32,
        right: i32,
    ) -> anyhow::Result<()> {
        let request = tonic::Request::new(shared::proto::SetPositionRequest {
            position: Some(shared::proto::WindowPosition {
                top,
                left,
                bottom,
                right,
            }),
        });
        self.runtime
            .clone()
            .block_on(self.window_client.set_window_position(request))?;

        Ok(())
    }

    #[tracing::instrument]
    pub fn set_candidates(&mut self, candidates: Vec<String>) -> anyhow::Result<()> {
        let request = tonic::Request::new(shared::proto::SetCandidateRequest { candidates });
        self.runtime
            .clone()
            .block_on(self.window_client.set_candidate(request))?;

        Ok(())
    }

    #[tracing::instrument]
    pub fn set_selection(&mut self, index: i32) -> anyhow::Result<()> {
        let request = tonic::Request::new(shared::proto::SetSelectionRequest { index });
        self.runtime
            .clone()
            .block_on(self.window_client.set_selection(request))?;

        Ok(())
    }

    #[tracing::instrument]
    pub fn set_input_mode(&mut self, mode: &str) -> anyhow::Result<()> {
        let request = tonic::Request::new(shared::proto::SetInputModeRequest {
            mode: mode.to_string(),
        });
        self.runtime
            .clone()
            .block_on(self.window_client.set_input_mode(request))?;

        Ok(())
    }

    #[tracing::instrument(skip(candidates))]
    pub fn update_candidate_window(
        &mut self,
        visible: Option<bool>,
        position: Option<shared::proto::WindowPosition>,
        candidates: Option<Vec<String>>,
        selected_index: Option<i32>,
        input_mode: Option<&str>,
    ) -> anyhow::Result<()> {
        let request = tonic::Request::new(shared::proto::UpdateCandidateWindowRequest {
            visible,
            position,
            candidates: candidates.map(|candidates| shared::proto::CandidateList { candidates }),
            selected_index,
            input_mode: input_mode.map(ToString::to_string),
        });
        self.runtime
            .clone()
            .block_on(self.window_client.update_candidate_window(request))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Candidates, IPCService};

    #[test]
    fn append_retry_is_enabled_when_server_state_is_unchanged() {
        let previous = Candidates {
            texts: vec!["か".to_string()],
            sub_texts: vec![String::new()],
            hiragana: "か".to_string(),
            corresponding_count: vec![1],
        };

        assert!(IPCService::should_retry_append_after_refresh(
            Some(&previous),
            &previous
        ));
    }

    #[test]
    fn append_retry_is_disabled_when_server_state_has_changed() {
        let previous = Candidates::default();
        let refreshed = Candidates {
            texts: vec!["か".to_string()],
            sub_texts: vec![String::new()],
            hiragana: "か".to_string(),
            corresponding_count: vec![1],
        };

        assert!(!IPCService::should_retry_append_after_refresh(
            Some(&previous),
            &refreshed
        ));
    }
}
