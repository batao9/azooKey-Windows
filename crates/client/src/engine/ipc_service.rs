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
    cell::Cell,
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

thread_local! {
    static CLIENT_INPUT_TRACE_REQUEST_ID: Cell<Option<u64>> = const { Cell::new(None) };
}

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
    server_session_id: Option<u64>,
    server_reset_recovered: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Candidates {
    pub texts: Vec<String>,
    pub sub_texts: Vec<String>,
    pub hiragana: String,
    pub corresponding_count: Vec<i32>,
}

impl Candidates {
    pub(crate) fn is_empty_composition(&self) -> bool {
        self.texts.is_empty()
            && self.sub_texts.is_empty()
            && self.hiragana.is_empty()
            && self.corresponding_count.is_empty()
    }
}

#[derive(Debug)]
enum NonIdempotentEditAttempt<T> {
    Completed(T),
    ReconnectAndRefresh(anyhow::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NonIdempotentEditRecovery {
    None,
    RetriedAfterUnchangedRefresh,
    RefreshedAfterReconnect,
}

impl NonIdempotentEditRecovery {
    fn log_value(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::RetriedAfterUnchangedRefresh => "retry_after_unchanged_refresh",
            Self::RefreshedAfterReconnect => "refresh_after_reconnect",
        }
    }
}

fn next_request_id() -> u64 {
    let counter = CLIENT_REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    (u64::from(std::process::id()) << 32) | (counter & 0xffff_ffff)
}

fn current_or_next_request_id() -> u64 {
    CLIENT_INPUT_TRACE_REQUEST_ID
        .with(|current| current.get())
        .unwrap_or_else(next_request_id)
}

pub(crate) fn current_input_trace_request_id() -> Option<u64> {
    CLIENT_INPUT_TRACE_REQUEST_ID.with(|current| current.get())
}

fn client_log_config_cache() -> &'static Mutex<ClientLogConfigCache> {
    CLIENT_LOG_CONFIG_CACHE.get_or_init(|| Mutex::new(ClientLogConfigCache::default()))
}

pub(crate) fn client_performance_log_enabled() -> bool {
    let Ok(mut cache) = client_log_config_cache().lock() else {
        return false;
    };

    let should_refresh = cache
        .last_checked
        .map(|last_checked| last_checked.elapsed() >= CLIENT_LOG_CONFIG_REFRESH_INTERVAL)
        .unwrap_or(true);
    if should_refresh {
        cache.enabled = AppConfig::read()
            .map(|config| {
                config.debug.server_log_enabled
                    && config.debug.server_log_level.eq_ignore_ascii_case("debug")
            })
            .unwrap_or(false);
        cache.last_checked = Some(Instant::now());
    }

    cache.enabled
}

fn duration_millis_u64(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn client_performance_start() -> Option<Instant> {
    client_performance_log_enabled().then(Instant::now)
}

#[derive(Debug)]
pub(crate) struct ClientInputTraceGuard {
    request_id: u64,
    previous_request_id: Option<u64>,
}

impl ClientInputTraceGuard {
    pub(crate) fn begin() -> Self {
        let request_id = next_request_id();
        let previous_request_id =
            CLIENT_INPUT_TRACE_REQUEST_ID.with(|current| current.replace(Some(request_id)));
        Self {
            request_id,
            previous_request_id,
        }
    }

    pub(crate) fn request_id(&self) -> u64 {
        self.request_id
    }
}

impl Drop for ClientInputTraceGuard {
    fn drop(&mut self) {
        CLIENT_INPUT_TRACE_REQUEST_ID.with(|current| current.set(self.previous_request_id));
    }
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
            server_session_id: None,
            server_reset_recovered: false,
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

    fn observe_server_session(&mut self, operation: &str, server_session_id: u64) {
        if server_session_id == 0 {
            return;
        }

        if Self::server_session_changed(self.server_session_id, server_session_id) {
            if let Some(previous_session_id) = self.server_session_id {
                self.server_reset_recovered = true;
                tracing::warn!(
                    operation = operation,
                    previous_session_id = previous_session_id,
                    server_session_id = server_session_id,
                    "Detected azookey server session change"
                );
            }
        }

        self.server_session_id = Some(server_session_id);
    }

    #[inline]
    fn server_session_changed(previous_session_id: Option<u64>, server_session_id: u64) -> bool {
        server_session_id != 0
            && previous_session_id.is_some_and(|previous| previous != server_session_id)
    }

    pub(crate) fn take_server_reset_recovered(&mut self) -> bool {
        let recovered = self.server_reset_recovered;
        self.server_reset_recovered = false;
        recovered
    }

    fn run_rpc_with_reconnect<T>(
        &mut self,
        operation: &str,
        mut send: impl FnMut(&mut Self) -> anyhow::Result<T>,
    ) -> anyhow::Result<(T, bool)> {
        match send(self) {
            Ok(value) => Ok((value, false)),
            Err(first_error) => {
                if !Self::should_reconnect_rpc_error(&first_error) {
                    tracing::warn!(
                        "{operation} failed with non-reconnectable error: {first_error:?}"
                    );
                    return Err(first_error);
                }

                tracing::warn!(
                    "{operation} first attempt failed, reconnecting IPC once: {first_error:?}"
                );

                match self.reconnect() {
                    Ok(()) => {
                        tracing::info!("{operation} IPC reconnect succeeded, retrying request");
                    }
                    Err(reconnect_error) => {
                        tracing::error!("{operation} IPC reconnect failed: {reconnect_error:?}");
                        return Err(reconnect_error);
                    }
                }

                match send(self) {
                    Ok(value) => Ok((value, true)),
                    Err(retry_error) => {
                        tracing::error!(
                            "{operation} retry failed after IPC reconnect: {retry_error:?}"
                        );
                        Err(retry_error)
                    }
                }
            }
        }
    }

    fn classify_non_idempotent_edit_attempt<T>(
        operation: &str,
        first_result: anyhow::Result<T>,
    ) -> anyhow::Result<NonIdempotentEditAttempt<T>> {
        match first_result {
            Ok(value) => Ok(NonIdempotentEditAttempt::Completed(value)),
            Err(first_error) => {
                if !Self::should_reconnect_rpc_error(&first_error) {
                    tracing::warn!(
                        "{operation} failed with non-reconnectable error: {first_error:?}"
                    );
                    return Err(first_error);
                }

                tracing::warn!(
                    "{operation} first attempt failed, reconnecting IPC once without replaying edit RPC: {first_error:?}"
                );
                Ok(NonIdempotentEditAttempt::ReconnectAndRefresh(first_error))
            }
        }
    }

    #[inline]
    fn should_retry_non_idempotent_edit_after_refresh(
        previous_candidates: Option<&Candidates>,
        refreshed_candidates: &Candidates,
    ) -> bool {
        previous_candidates.is_some_and(|previous| {
            previous == refreshed_candidates && !refreshed_candidates.is_empty_composition()
        })
    }

    fn run_non_idempotent_edit_with_reconnect(
        &mut self,
        operation: &str,
        request_id: u64,
        previous_candidates: Option<&Candidates>,
        mut send: impl FnMut(&mut Self) -> anyhow::Result<Candidates>,
    ) -> anyhow::Result<(Candidates, NonIdempotentEditRecovery)> {
        match Self::classify_non_idempotent_edit_attempt(operation, send(self))? {
            NonIdempotentEditAttempt::Completed(candidates) => {
                Ok((candidates, NonIdempotentEditRecovery::None))
            }
            NonIdempotentEditAttempt::ReconnectAndRefresh(first_error) => {
                match self.reconnect() {
                    Ok(()) => {
                        tracing::info!(
                            "{operation} IPC reconnect succeeded, refreshing server composition without replaying edit RPC"
                        );
                    }
                    Err(reconnect_error) => {
                        tracing::error!(
                            "{operation} IPC reconnect failed after first error {first_error:?}: {reconnect_error:?}"
                        );
                        return Err(reconnect_error);
                    }
                }

                // remove_text, shrink_text, and non-zero move_cursor may have already
                // changed server state before the transport broke. Refresh first, and
                // only replay the edit if the server state is still the previous one.
                match self.send_move_cursor(0, request_id) {
                    Ok(refreshed_candidates) => {
                        if Self::should_retry_non_idempotent_edit_after_refresh(
                            previous_candidates,
                            &refreshed_candidates,
                        ) {
                            tracing::warn!(
                                "{operation} refreshed unchanged composition after reconnect, retrying edit RPC once"
                            );
                            let candidates = send(self)?;
                            return Ok((
                                candidates,
                                NonIdempotentEditRecovery::RetriedAfterUnchangedRefresh,
                            ));
                        }

                        Ok((
                            refreshed_candidates,
                            NonIdempotentEditRecovery::RefreshedAfterReconnect,
                        ))
                    }
                    Err(refresh_error) => {
                        tracing::error!(
                            "{operation} refresh failed after IPC reconnect: {refresh_error:?}"
                        );
                        Err(refresh_error)
                    }
                }
            }
        }
    }

    fn should_reconnect_rpc_error(error: &anyhow::Error) -> bool {
        let Some(status) = error.downcast_ref::<tonic::Status>() else {
            return true;
        };

        matches!(
            status.code(),
            tonic::Code::Aborted
                | tonic::Code::Cancelled
                | tonic::Code::DataLoss
                | tonic::Code::DeadlineExceeded
                | tonic::Code::Internal
                | tonic::Code::Unavailable
                | tonic::Code::Unknown
        )
    }

    fn send_append_text(
        &mut self,
        text: &str,
        input_style: i32,
        request_id: u64,
    ) -> anyhow::Result<shared::proto::AppendTextResponse> {
        let request = tonic::Request::new(shared::proto::AppendTextRequest {
            text_to_append: text.to_string(),
            input_style,
            request_id,
        });

        let response = self
            .runtime
            .clone()
            .block_on(self.azookey_client.append_text(request))?;
        let response = response.into_inner();
        self.observe_server_session("append_text", response.server_session_id);
        Ok(response)
    }

    fn send_remove_text(&mut self, request_id: u64) -> anyhow::Result<Candidates> {
        let request = tonic::Request::new(shared::proto::RemoveTextRequest { request_id });
        let response = self
            .runtime
            .clone()
            .block_on(self.azookey_client.remove_text(request))?;
        let response = response.into_inner();
        self.observe_server_session("remove_text", response.server_session_id);
        Self::candidates_from_composing_text(response.composing_text)
    }

    fn send_clear_text(&mut self, request_id: u64) -> anyhow::Result<()> {
        let request = tonic::Request::new(shared::proto::ClearTextRequest { request_id });
        let response = self
            .runtime
            .clone()
            .block_on(self.azookey_client.clear_text(request))?;
        let response = response.into_inner();
        self.observe_server_session("clear_text", response.server_session_id);
        Ok(())
    }

    fn send_shrink_text(&mut self, offset: i32, request_id: u64) -> anyhow::Result<Candidates> {
        let request = tonic::Request::new(shared::proto::ShrinkTextRequest { offset, request_id });
        let response = self
            .runtime
            .clone()
            .block_on(self.azookey_client.shrink_text(request))?;
        let response = response.into_inner();
        self.observe_server_session("shrink_text", response.server_session_id);
        Self::candidates_from_composing_text(response.composing_text)
    }

    fn send_move_cursor(&mut self, offset: i32, request_id: u64) -> anyhow::Result<Candidates> {
        let request = tonic::Request::new(shared::proto::MoveCursorRequest { offset, request_id });
        let response = self
            .runtime
            .clone()
            .block_on(self.azookey_client.move_cursor(request))?;
        let response = response.into_inner();
        self.observe_server_session("move_cursor", response.server_session_id);
        Self::candidates_from_composing_text(response.composing_text)
    }

    fn send_set_context(&mut self, context: &str, request_id: u64) -> anyhow::Result<()> {
        let request = tonic::Request::new(shared::proto::SetContextRequest {
            context: context.to_string(),
            request_id,
        });
        let response = self
            .runtime
            .clone()
            .block_on(self.azookey_client.set_context(request))?;
        let response = response.into_inner();
        self.observe_server_session("set_context", response.server_session_id);
        Ok(())
    }

    fn enqueue_client_performance(
        &self,
        request_id: u64,
        operation: &str,
        stage: &str,
        elapsed: Duration,
        details: String,
    ) {
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

    pub(crate) fn log_client_performance(
        &self,
        request_id: u64,
        operation: &str,
        stage: &str,
        elapsed: Duration,
        details: String,
    ) {
        if !client_performance_log_enabled() {
            return;
        }

        self.enqueue_client_performance(request_id, operation, stage, elapsed, details);
    }

    fn log_client_performance_from_start(
        &self,
        start: Option<Instant>,
        request_id: u64,
        operation: &str,
        stage: &str,
        details: impl FnOnce() -> String,
    ) {
        if let Some(start) = start {
            self.enqueue_client_performance(
                request_id,
                operation,
                stage,
                start.elapsed(),
                details(),
            );
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
        previous_candidates.is_some_and(|previous| {
            previous == refreshed_candidates || refreshed_candidates.is_empty_composition()
        })
    }

    #[inline]
    fn should_reset_client_composition_after_append_refresh(
        previous_candidates: Option<&Candidates>,
        refreshed_candidates: &Candidates,
    ) -> bool {
        previous_candidates.is_some() && refreshed_candidates.is_empty_composition()
    }

    #[tracing::instrument]
    fn append_text_with_style_and_context(
        &mut self,
        text: String,
        input_style: i32,
        previous_candidates: Option<&Candidates>,
    ) -> anyhow::Result<Candidates> {
        let request_id = current_or_next_request_id();
        let performance_start = client_performance_start();
        let input_len = performance_start.map(|_| text.chars().count());
        let send = |this: &mut Self| this.send_append_text(&text, input_style, request_id);

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
                        self.log_client_performance_from_start(
                            performance_start,
                            request_id,
                            "append_text",
                            "rpc_total",
                            || {
                                let input_len = input_len.unwrap_or_default();
                                format!(
                                    "status=error;phase=reconnect;input_len={input_len};input_style={input_style}"
                                )
                            },
                        );
                        return Err(reconnect_error);
                    }
                }

                match self.send_move_cursor(0, request_id) {
                    Ok(candidates) => {
                        if Self::should_retry_append_after_refresh(previous_candidates, &candidates)
                        {
                            if Self::should_reset_client_composition_after_append_refresh(
                                previous_candidates,
                                &candidates,
                            ) {
                                self.server_reset_recovered = true;
                                tracing::warn!(
                                    "append_text recovered empty composition after reconnect (style={input_style}); client composition reset required"
                                );
                            }
                            tracing::warn!(
                                "append_text recovered unchanged composition after reconnect (style={input_style}), retrying original input"
                            );
                            let retry_response = send(self)?;
                            let candidates = Self::candidates_from_composing_text(
                                retry_response.composing_text,
                            )?;
                            self.log_client_performance_from_start(
                                performance_start,
                                request_id,
                                "append_text",
                                "rpc_total",
                                || {
                                    let input_len = input_len.unwrap_or_default();
                                    format!(
                                        "status=success;retry=true;input_len={input_len};input_style={input_style}"
                                    )
                                },
                            );
                            return Ok(candidates);
                        }

                        tracing::info!(
                            "append_text recovered changed composition after reconnect (style={input_style}), reusing server state"
                        );
                        self.log_client_performance_from_start(
                            performance_start,
                            request_id,
                            "append_text",
                            "rpc_total",
                            || {
                                let input_len = input_len.unwrap_or_default();
                                format!(
                                    "status=recovered_changed;input_len={input_len};input_style={input_style}"
                                )
                            },
                        );
                        return Ok(candidates);
                    }
                    Err(refresh_error) => {
                        tracing::error!(
                            "append_text refresh failed after reconnect (style={input_style}): {refresh_error:?}"
                        );
                        self.log_client_performance_from_start(
                            performance_start,
                            request_id,
                            "append_text",
                            "rpc_total",
                            || {
                                let input_len = input_len.unwrap_or_default();
                                format!(
                                    "status=error;phase=refresh;input_len={input_len};input_style={input_style}"
                                )
                            },
                        );
                        return Err(refresh_error);
                    }
                }
            }
        };
        let candidates = Self::candidates_from_composing_text(response.composing_text)?;
        self.log_client_performance_from_start(
            performance_start,
            request_id,
            "append_text",
            "rpc_total",
            || {
                let input_len = input_len.unwrap_or_default();
                format!("status=success;input_len={input_len};input_style={input_style}")
            },
        );
        Ok(candidates)
    }

    #[tracing::instrument]
    pub fn remove_text(&mut self) -> anyhow::Result<Candidates> {
        self.remove_text_inner(None)
    }

    #[tracing::instrument(skip(self, previous_candidates))]
    pub fn remove_text_with_context(
        &mut self,
        previous_candidates: &Candidates,
    ) -> anyhow::Result<Candidates> {
        self.remove_text_inner(Some(previous_candidates))
    }

    fn remove_text_inner(
        &mut self,
        previous_candidates: Option<&Candidates>,
    ) -> anyhow::Result<Candidates> {
        let request_id = current_or_next_request_id();
        let performance_start = client_performance_start();
        let result = self.run_non_idempotent_edit_with_reconnect(
            "remove_text",
            request_id,
            previous_candidates,
            |this| this.send_remove_text(request_id),
        );
        self.log_client_performance_from_start(
            performance_start,
            request_id,
            "remove_text",
            "rpc_total",
            || match &result {
                Ok((_, recovery)) => format!("status=success;recovery={}", recovery.log_value()),
                Err(error) => format!("status=error;error={error:?}"),
            },
        );
        let (candidates, _) = result?;
        Ok(candidates)
    }

    #[tracing::instrument]
    pub fn clear_text(&mut self) -> anyhow::Result<()> {
        let request_id = current_or_next_request_id();
        let performance_start = client_performance_start();
        let result =
            self.run_rpc_with_reconnect("clear_text", |this| this.send_clear_text(request_id));
        self.log_client_performance_from_start(
            performance_start,
            request_id,
            "clear_text",
            "rpc_total",
            || match &result {
                Ok(((), retried)) => format!("status=success;retry={retried}"),
                Err(error) => format!("status=error;error={error:?}"),
            },
        );
        result.map(|((), _)| ())
    }

    #[tracing::instrument]
    pub fn shrink_text(&mut self, offset: i32) -> anyhow::Result<Candidates> {
        self.shrink_text_inner(offset, None)
    }

    #[tracing::instrument(skip(self, previous_candidates))]
    pub fn shrink_text_with_context(
        &mut self,
        offset: i32,
        previous_candidates: &Candidates,
    ) -> anyhow::Result<Candidates> {
        self.shrink_text_inner(offset, Some(previous_candidates))
    }

    fn shrink_text_inner(
        &mut self,
        offset: i32,
        previous_candidates: Option<&Candidates>,
    ) -> anyhow::Result<Candidates> {
        let request_id = current_or_next_request_id();
        let performance_start = client_performance_start();
        let result = self.run_non_idempotent_edit_with_reconnect(
            "shrink_text",
            request_id,
            previous_candidates,
            |this| this.send_shrink_text(offset, request_id),
        );
        self.log_client_performance_from_start(
            performance_start,
            request_id,
            "shrink_text",
            "rpc_total",
            || match &result {
                Ok((_, recovery)) => format!(
                    "status=success;recovery={};offset={offset}",
                    recovery.log_value()
                ),
                Err(error) => format!("status=error;offset={offset};error={error:?}"),
            },
        );
        let (candidates, _) = result?;
        Ok(candidates)
    }

    #[tracing::instrument]
    pub fn move_cursor(&mut self, offset: i32) -> anyhow::Result<Candidates> {
        self.move_cursor_inner(offset, None)
    }

    #[tracing::instrument(skip(self, previous_candidates))]
    pub fn move_cursor_with_context(
        &mut self,
        offset: i32,
        previous_candidates: &Candidates,
    ) -> anyhow::Result<Candidates> {
        self.move_cursor_inner(offset, Some(previous_candidates))
    }

    fn move_cursor_inner(
        &mut self,
        offset: i32,
        previous_candidates: Option<&Candidates>,
    ) -> anyhow::Result<Candidates> {
        let request_id = current_or_next_request_id();
        let performance_start = client_performance_start();
        let result = self.run_non_idempotent_edit_with_reconnect(
            "move_cursor",
            request_id,
            previous_candidates,
            |this| this.send_move_cursor(offset, request_id),
        );
        self.log_client_performance_from_start(
            performance_start,
            request_id,
            "move_cursor",
            "rpc_total",
            || match &result {
                Ok((_, recovery)) => format!(
                    "status=success;recovery={};offset={offset}",
                    recovery.log_value()
                ),
                Err(error) => format!("status=error;offset={offset};error={error:?}"),
            },
        );
        let (candidates, _) = result?;
        Ok(candidates)
    }

    pub fn set_context(&mut self, context: String) -> anyhow::Result<()> {
        let request_id = current_or_next_request_id();
        let performance_start = client_performance_start();
        let context_len = performance_start.map(|_| context.chars().count());
        let result = self.run_rpc_with_reconnect("set_context", |this| {
            this.send_set_context(&context, request_id)
        });
        self.log_client_performance_from_start(
            performance_start,
            request_id,
            "set_context",
            "rpc_total",
            || {
                let context_len = context_len.unwrap_or_default();
                match &result {
                    Ok(((), retried)) => {
                        format!("status=success;retry={retried};context_len={context_len}")
                    }
                    Err(error) => {
                        format!("status=error;context_len={context_len};error={error:?}")
                    }
                }
            },
        );

        result.map(|((), _)| ())
    }
}

// implement methods to interact with candidate window server
impl IPCService {
    #[tracing::instrument]
    pub fn show_window(&mut self) -> anyhow::Result<()> {
        let request_id = current_or_next_request_id();
        let performance_start = client_performance_start();
        let result: anyhow::Result<()> = (|| {
            let request = tonic::Request::new(shared::proto::EmptyResponse {});
            self.runtime
                .clone()
                .block_on(self.window_client.show_window(request))?;

            Ok(())
        })();
        self.log_client_performance_from_start(
            performance_start,
            request_id,
            "ui_show_window",
            "rpc_total",
            || match &result {
                Ok(()) => "status=success".to_string(),
                Err(error) => format!("status=error;error={error:?}"),
            },
        );
        result
    }

    #[tracing::instrument]
    pub fn hide_window(&mut self) -> anyhow::Result<()> {
        let request_id = current_or_next_request_id();
        let performance_start = client_performance_start();
        let result: anyhow::Result<()> = (|| {
            let request = tonic::Request::new(shared::proto::EmptyResponse {});
            self.runtime
                .clone()
                .block_on(self.window_client.hide_window(request))?;

            Ok(())
        })();
        self.log_client_performance_from_start(
            performance_start,
            request_id,
            "ui_hide_window",
            "rpc_total",
            || match &result {
                Ok(()) => "status=success".to_string(),
                Err(error) => format!("status=error;error={error:?}"),
            },
        );
        result
    }

    #[tracing::instrument]
    pub fn set_window_position(
        &mut self,
        top: i32,
        left: i32,
        bottom: i32,
        right: i32,
    ) -> anyhow::Result<()> {
        let request_id = current_or_next_request_id();
        let performance_start = client_performance_start();
        let result: anyhow::Result<()> = (|| {
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
        })();
        self.log_client_performance_from_start(
            performance_start,
            request_id,
            "ui_set_window_position",
            "rpc_total",
            || match &result {
                Ok(()) => {
                    format!("status=success;top={top};left={left};bottom={bottom};right={right}")
                }
                Err(error) => format!(
                    "status=error;top={top};left={left};bottom={bottom};right={right};error={error:?}"
                ),
            },
        );
        result
    }

    #[tracing::instrument]
    pub fn set_candidates(&mut self, candidates: Vec<String>) -> anyhow::Result<()> {
        let request_id = current_or_next_request_id();
        let performance_start = client_performance_start();
        let candidate_count = performance_start.map(|_| candidates.len());
        let result: anyhow::Result<()> = (|| {
            let request = tonic::Request::new(shared::proto::SetCandidateRequest { candidates });
            self.runtime
                .clone()
                .block_on(self.window_client.set_candidate(request))?;

            Ok(())
        })();
        self.log_client_performance_from_start(
            performance_start,
            request_id,
            "ui_set_candidates",
            "rpc_total",
            || {
                let candidate_count = candidate_count.unwrap_or_default();
                match &result {
                    Ok(()) => format!("status=success;candidate_count={candidate_count}"),
                    Err(error) => {
                        format!("status=error;candidate_count={candidate_count};error={error:?}")
                    }
                }
            },
        );
        result
    }

    #[tracing::instrument]
    pub fn set_selection(&mut self, index: i32) -> anyhow::Result<()> {
        let request_id = current_or_next_request_id();
        let performance_start = client_performance_start();
        let result: anyhow::Result<()> = (|| {
            let request = tonic::Request::new(shared::proto::SetSelectionRequest { index });
            self.runtime
                .clone()
                .block_on(self.window_client.set_selection(request))?;

            Ok(())
        })();
        self.log_client_performance_from_start(
            performance_start,
            request_id,
            "ui_set_selection",
            "rpc_total",
            || match &result {
                Ok(()) => format!("status=success;index={index}"),
                Err(error) => format!("status=error;index={index};error={error:?}"),
            },
        );
        result
    }

    #[tracing::instrument]
    pub fn set_input_mode(&mut self, mode: &str) -> anyhow::Result<()> {
        let request_id = current_or_next_request_id();
        let performance_start = client_performance_start();
        let result: anyhow::Result<()> = (|| {
            let request = tonic::Request::new(shared::proto::SetInputModeRequest {
                mode: mode.to_string(),
            });
            self.runtime
                .clone()
                .block_on(self.window_client.set_input_mode(request))?;

            Ok(())
        })();
        self.log_client_performance_from_start(
            performance_start,
            request_id,
            "ui_set_input_mode",
            "rpc_total",
            || match &result {
                Ok(()) => format!("status=success;mode={mode}"),
                Err(error) => format!("status=error;mode={mode};error={error:?}"),
            },
        );
        result
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
        let request_id = current_or_next_request_id();
        let performance_start = client_performance_start();
        let position_present = performance_start.map(|_| position.is_some());
        let candidate_count = performance_start.map(|_| candidates.as_ref().map(Vec::len));
        let input_mode_present = performance_start.map(|_| input_mode.is_some());
        let result: anyhow::Result<()> = (|| {
            let request = tonic::Request::new(shared::proto::UpdateCandidateWindowRequest {
                visible,
                position,
                candidates: candidates
                    .map(|candidates| shared::proto::CandidateList { candidates }),
                selected_index,
                input_mode: input_mode.map(ToString::to_string),
            });
            self.runtime
                .clone()
                .block_on(self.window_client.update_candidate_window(request))?;

            Ok(())
        })();
        self.log_client_performance_from_start(
            performance_start,
            request_id,
            "ui_update_candidate_window",
            "rpc_total",
            || {
                let position_present = position_present.unwrap_or_default();
                let candidate_count = candidate_count.unwrap_or_default();
                let input_mode_present = input_mode_present.unwrap_or_default();
                match &result {
                    Ok(()) => format!(
                        "status=success;visible={visible:?};position_present={position_present};candidate_count={candidate_count:?};selected_index={selected_index:?};input_mode_present={input_mode_present}"
                    ),
                    Err(error) => format!(
                        "status=error;visible={visible:?};position_present={position_present};candidate_count={candidate_count:?};selected_index={selected_index:?};input_mode_present={input_mode_present};error={error:?}"
                    ),
                }
            },
        );
        result
    }
}

#[cfg(test)]
mod tests {
    use super::{Candidates, IPCService, NonIdempotentEditAttempt};

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

    #[test]
    fn append_retry_is_enabled_when_server_state_was_reset() {
        let previous = Candidates {
            texts: vec!["感じ".to_string()],
            sub_texts: vec![String::new()],
            hiragana: "かんじ".to_string(),
            corresponding_count: vec![5],
        };

        assert!(IPCService::should_retry_append_after_refresh(
            Some(&previous),
            &Candidates::default()
        ));
    }

    #[test]
    fn append_recovery_requires_client_reset_when_server_state_was_reset() {
        let previous = Candidates {
            texts: vec!["漢字".to_string()],
            sub_texts: vec![String::new()],
            hiragana: "かんじ".to_string(),
            corresponding_count: vec![5],
        };

        assert!(
            IPCService::should_reset_client_composition_after_append_refresh(
                Some(&previous),
                &Candidates::default()
            )
        );
    }

    #[test]
    fn append_recovery_does_not_reset_client_when_server_state_is_unchanged() {
        let previous = Candidates {
            texts: vec!["か".to_string()],
            sub_texts: vec![String::new()],
            hiragana: "か".to_string(),
            corresponding_count: vec![1],
        };

        assert!(
            !IPCService::should_reset_client_composition_after_append_refresh(
                Some(&previous),
                &previous
            )
        );
    }

    #[test]
    fn non_idempotent_edit_retry_is_enabled_when_refreshed_state_is_unchanged() {
        let previous = Candidates {
            texts: vec!["か".to_string()],
            sub_texts: vec![String::new()],
            hiragana: "か".to_string(),
            corresponding_count: vec![1],
        };

        assert!(IPCService::should_retry_non_idempotent_edit_after_refresh(
            Some(&previous),
            &previous
        ));
    }

    #[test]
    fn non_idempotent_edit_retry_is_disabled_when_refreshed_state_changed() {
        let previous = Candidates {
            texts: vec!["か".to_string()],
            sub_texts: vec![String::new()],
            hiragana: "か".to_string(),
            corresponding_count: vec![1],
        };
        let refreshed = Candidates {
            texts: vec!["".to_string()],
            sub_texts: vec![String::new()],
            hiragana: String::new(),
            corresponding_count: vec![0],
        };

        assert!(!IPCService::should_retry_non_idempotent_edit_after_refresh(
            Some(&previous),
            &refreshed
        ));
    }

    #[test]
    fn non_idempotent_edit_retry_is_disabled_for_empty_refreshed_state() {
        let previous = Candidates {
            texts: vec!["か".to_string()],
            sub_texts: vec![String::new()],
            hiragana: "か".to_string(),
            corresponding_count: vec![1],
        };

        assert!(!IPCService::should_retry_non_idempotent_edit_after_refresh(
            Some(&previous),
            &Candidates::default()
        ));
    }

    #[test]
    fn server_session_change_ignores_initial_observation() {
        assert!(!IPCService::server_session_changed(None, 42));
    }

    #[test]
    fn server_session_change_detects_known_session_change() {
        assert!(IPCService::server_session_changed(Some(42), 43));
    }

    #[test]
    fn server_session_change_ignores_zero_session_id() {
        assert!(!IPCService::server_session_changed(Some(42), 0));
    }

    #[test]
    fn server_session_change_ignores_same_session() {
        assert!(!IPCService::server_session_changed(Some(42), 42));
    }

    #[test]
    fn reconnect_retry_is_enabled_for_transport_like_status() {
        let error = anyhow::Error::new(tonic::Status::unavailable("pipe closed"));

        assert!(IPCService::should_reconnect_rpc_error(&error));
    }

    #[test]
    fn reconnect_retry_is_disabled_for_invalid_request_status() {
        let error = anyhow::Error::new(tonic::Status::invalid_argument("offset out of range"));

        assert!(!IPCService::should_reconnect_rpc_error(&error));
    }

    #[test]
    fn reconnect_retry_is_enabled_for_non_status_error() {
        let error = anyhow::anyhow!("named pipe disconnected");

        assert!(IPCService::should_reconnect_rpc_error(&error));
    }

    #[test]
    fn non_idempotent_edit_attempt_completes_without_recovery_on_success() {
        let candidates = Candidates {
            texts: vec!["か".to_string()],
            sub_texts: vec![String::new()],
            hiragana: "か".to_string(),
            corresponding_count: vec![1],
        };

        let attempt =
            IPCService::classify_non_idempotent_edit_attempt("remove_text", Ok(candidates.clone()))
                .expect("successful edit should not require recovery");

        match attempt {
            NonIdempotentEditAttempt::Completed(value) => assert_eq!(value, candidates),
            NonIdempotentEditAttempt::ReconnectAndRefresh(_) => {
                panic!("successful edit must not be classified as reconnect recovery")
            }
        }
    }

    #[test]
    fn non_idempotent_edit_attempt_refreshes_after_reconnectable_error() {
        let error = anyhow::Error::new(tonic::Status::unavailable("pipe closed"));

        let attempt = IPCService::classify_non_idempotent_edit_attempt::<Candidates>(
            "remove_text",
            Err(error),
        )
        .expect("reconnectable edit error should recover by refreshing");

        assert!(matches!(
            attempt,
            NonIdempotentEditAttempt::ReconnectAndRefresh(_)
        ));
    }

    #[test]
    fn non_idempotent_edit_attempt_returns_non_reconnectable_error() {
        let error = anyhow::Error::new(tonic::Status::invalid_argument("offset out of range"));

        let attempt = IPCService::classify_non_idempotent_edit_attempt::<Candidates>(
            "move_cursor",
            Err(error),
        );

        assert!(attempt.is_err());
    }
}
