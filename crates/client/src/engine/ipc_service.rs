use anyhow::Result;
use hyper_util::rt::TokioIo;
use shared::{
    proto::{
        azookey_service_client::AzookeyServiceClient, window_service_client::WindowServiceClient,
        PerformanceLogRequest, StartReconversionRequest,
    },
    AppConfig,
};
use std::{
    cell::Cell,
    error::Error as StdError,
    fmt,
    future::Future,
    io,
    os::windows::io::IntoRawHandle,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
    task::{Context, Poll},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::windows::named_pipe::NamedPipeClient,
    time,
};
use tonic::transport::{channel::Channel, Endpoint};
use tower::service_fn;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_PIPE_BUSY};

const INPUT_STYLE_ROMAN2KANA: i32 = 0;
const INPUT_STYLE_DIRECT: i32 = 1;
const CLIENT_LOG_CONFIG_REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const PIPE_BUSY_RETRY_INTERVAL: Duration = Duration::from_millis(50);
const SERVER_PIPE_BUSY_TIMEOUT: Duration = Duration::from_millis(750);
const UI_PIPE_BUSY_TIMEOUT: Duration = Duration::ZERO;
const IPC_CONNECT_DEADLINE: Duration = Duration::from_secs(1);
const INPUT_RPC_DEADLINE: Duration = Duration::from_secs(2);
const STATE_RPC_DEADLINE: Duration = Duration::from_secs(1);
const LEARNING_RPC_DEADLINE: Duration = Duration::from_secs(1);
const UI_RPC_DEADLINE: Duration = Duration::from_millis(250);
const PERFORMANCE_RPC_DEADLINE: Duration = Duration::from_millis(100);

static CLIENT_REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static IPC_CONNECTION_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static CLIENT_LOG_CONFIG_CACHE: OnceLock<Mutex<ClientLogConfigCache>> = OnceLock::new();

thread_local! {
    static CLIENT_INPUT_TRACE_REQUEST_ID: Cell<Option<u64>> = const { Cell::new(None) };
}

fn open_named_pipe_client(pipe_name: &str) -> std::io::Result<NamedPipeClient> {
    let handle = shared::open_named_pipe_client_handle(pipe_name)?;
    unsafe { NamedPipeClient::from_raw_handle(handle.into_raw_handle()) }
}

#[derive(Debug, Default)]
struct ClientLogConfigCache {
    last_checked: Option<Instant>,
    enabled: bool,
}

// connect to kkc server
#[derive(Debug, Clone)]
pub struct IPCService {
    connection_id: u64,
    // kkc server client
    azookey_client: AzookeyServiceClient<Channel>,
    // candidate window server client
    window_client: Option<WindowServiceClient<Channel>>,
    runtime: Arc<tokio::runtime::Runtime>,
    performance_log_tx: tokio::sync::mpsc::Sender<PerformanceLogRequest>,
    server_session_id: Option<u64>,
    server_reset_recovered: bool,
    recovery: Arc<ServerRecoveryState>,
    transport: Arc<TransportLifecycle>,
    #[cfg(test)]
    recovery_error_for_test: bool,
    #[cfg(test)]
    reconnect_channel_for_test: Option<Channel>,
}

#[derive(Debug, Default)]
struct TransportLifecycle {
    opened: AtomicBool,
    retired: AtomicBool,
    wake: Arc<tokio::sync::Notify>,
}

impl TransportLifecycle {
    fn retire(&self) {
        self.retired.store(true, Ordering::Release);
        self.wake.notify_waiters();
    }
}

// Channel clones (including the logging worker) must not keep an old owner
// alive. Retirement wakes the transport's pending read and closes the real pipe.
struct RetirableIo<T> {
    inner: Option<T>,
    lifecycle: Arc<TransportLifecycle>,
    retired: Pin<Box<dyn Future<Output = ()> + Send>>,
}

impl<T: Unpin> RetirableIo<T> {
    fn new(inner: T, lifecycle: Arc<TransportLifecycle>) -> Self {
        let wake = lifecycle.wake.clone();
        let retired = Box::pin(async move { wake.notified().await });
        Self {
            inner: Some(inner),
            lifecycle,
            retired,
        }
    }

    fn io(&mut self, cx: &mut Context<'_>) -> io::Result<Pin<&mut T>> {
        if self.retired.as_mut().poll(cx).is_ready()
            || self.lifecycle.retired.load(Ordering::Acquire)
        {
            self.inner.take();
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "retired IPC transport",
            ));
        }
        Ok(Pin::new(self.inner.as_mut().expect("live transport")))
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for RetirableIo<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.get_mut().io(cx)?.poll_read(cx, buf)
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for RetirableIo<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.get_mut().io(cx)?.poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.get_mut().io(cx)?.poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.get_mut().io(cx)?.poll_shutdown(cx)
    }
}

#[derive(Debug)]
struct ServerRecoveryState {
    pending: AtomicBool,
    generation: AtomicU64,
    restart_completed_generation: AtomicU64,
    restart_request_in_flight: AtomicBool,
    input_ledger: Mutex<InputLedger>,
    context_epoch: AtomicU64,
}

impl Default for ServerRecoveryState {
    fn default() -> Self {
        Self {
            pending: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            restart_completed_generation: AtomicU64::new(0),
            restart_request_in_flight: AtomicBool::new(false),
            context_epoch: AtomicU64::new(0),
            input_ledger: Mutex::new(InputLedger {
                operations: Vec::new(),
                complete: true,
            }),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct InputLedger {
    operations: Vec<CompositionOperation>,
    complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CompositionOperation {
    Append { text: String, input_style: i32 },
    Remove,
    MoveCursor(i32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecoveredComposition {
    pub(crate) candidates: Candidates,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextRemoval {
    pub(crate) candidates: Candidates,
    pub(crate) raw_input: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReconversionResult {
    pub(crate) candidates: Candidates,
    pub(crate) selection_index: i32,
}

#[derive(Debug, Clone)]
pub(crate) struct InputLedgerSnapshot(InputLedger);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IpcDeadlineExceeded {
    operation: &'static str,
    deadline: Duration,
}

impl fmt::Display for IpcDeadlineExceeded {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} exceeded IPC deadline of {:?}",
            self.operation, self.deadline
        )
    }
}

impl StdError for IpcDeadlineExceeded {}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IpcRecoveryPending {
    details: String,
}

impl fmt::Display for IpcRecoveryPending {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "IPC recovery is still pending: {}", self.details)
    }
}

impl StdError for IpcRecoveryPending {}

pub(crate) fn is_ipc_deadline(error: &anyhow::Error) -> bool {
    error.downcast_ref::<IpcDeadlineExceeded>().is_some()
        || error
            .downcast_ref::<tonic::Status>()
            .is_some_and(|status| status.code() == tonic::Code::DeadlineExceeded)
}

pub(crate) fn is_non_destructive_ipc_error(error: &anyhow::Error) -> bool {
    requires_ipc_recovery(error)
        || error.downcast_ref::<tonic::Status>().is_some_and(|status| {
            matches!(
                status.code(),
                tonic::Code::InvalidArgument
                    | tonic::Code::PermissionDenied
                    | tonic::Code::OutOfRange
                    | tonic::Code::FailedPrecondition
            )
        })
}

pub(crate) fn is_ipc_permission_denied(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<tonic::Status>()
        .is_some_and(|status| status.code() == tonic::Code::PermissionDenied)
}

pub(crate) fn requires_ipc_recovery(error: &anyhow::Error) -> bool {
    is_ipc_deadline(error) || error.downcast_ref::<IpcRecoveryPending>().is_some()
}

fn preserve_recovery_error(error: anyhow::Error) -> anyhow::Error {
    if is_non_destructive_ipc_error(&error) {
        error
    } else {
        IpcRecoveryPending {
            details: format!("{error:#}"),
        }
        .into()
    }
}

async fn await_rpc_with_deadline<T, F>(
    operation: &'static str,
    deadline: Duration,
    future: F,
) -> anyhow::Result<T>
where
    F: Future<Output = Result<T, tonic::Status>>,
{
    match time::timeout(deadline, future).await {
        Ok(result) => result.map_err(Into::into),
        Err(_) => Err(IpcDeadlineExceeded {
            operation,
            deadline,
        }
        .into()),
    }
}

fn recovery_generation_is_current(expected: u64, current: u64) -> bool {
    expected == current
}

fn restart_generation_ready(required: u64, completed: u64) -> bool {
    required != 0 && completed >= required
}

fn restart_request_needed(pending: bool, ready: bool, in_flight: bool) -> bool {
    pending && !ready && !in_flight
}

fn append_input_segment(ledger: &mut InputLedger, text: &str, input_style: i32) {
    if !ledger.complete || text.is_empty() {
        return;
    }
    ledger.operations.push(CompositionOperation::Append {
        text: text.to_string(),
        input_style,
    });
}

fn pop_input_segment_character(ledger: &mut InputLedger) {
    if ledger.complete {
        ledger.operations.push(CompositionOperation::Remove);
    }
}

fn move_input_cursor(ledger: &mut InputLedger, offset: i32) {
    if ledger.complete && offset != 0 {
        ledger
            .operations
            .push(CompositionOperation::MoveCursor(offset));
    }
}

fn mark_input_ledger_incomplete(ledger: &mut InputLedger) {
    ledger.operations.clear();
    ledger.complete = false;
}

fn fallback_input_ledger(raw_input: &str, raw_hiragana: &str) -> InputLedger {
    let (text, input_style) = if raw_input.is_empty() {
        (raw_hiragana, INPUT_STYLE_DIRECT)
    } else {
        // raw_hiragana may still contain an incomplete roman2kana sequence such as
        // `k`. Replaying raw_input preserves that converter buffer, while kana in
        // raw_input is also accepted by the roman2kana input path.
        (raw_input, INPUT_STYLE_ROMAN2KANA)
    };

    InputLedger {
        operations: if text.is_empty() {
            Vec::new()
        } else {
            vec![CompositionOperation::Append {
                text: text.to_string(),
                input_style,
            }]
        },
        complete: true,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Candidates {
    pub texts: Vec<String>,
    pub sub_texts: Vec<String>,
    pub hiragana: String,
    pub corresponding_count: Vec<i32>,
    pub candidate_ids: Vec<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClauseSnapshotOperation {
    Clear,
    Push,
    Pop,
}

impl ClauseSnapshotOperation {
    fn proto_value(self) -> i32 {
        use shared::proto::CompositionSnapshotOperation;

        match self {
            Self::Clear => CompositionSnapshotOperation::Clear as i32,
            Self::Push => CompositionSnapshotOperation::Push as i32,
            Self::Pop => CompositionSnapshotOperation::Pop as i32,
        }
    }
}

impl Candidates {
    pub(crate) fn is_empty_composition(&self) -> bool {
        self.texts.is_empty()
            && self.sub_texts.is_empty()
            && self.hiragana.is_empty()
            && self.corresponding_count.is_empty()
            && self.candidate_ids.is_empty()
    }

    #[inline]
    fn has_same_composition(&self, other: &Self) -> bool {
        self.texts == other.texts
            && self.sub_texts == other.sub_texts
            && self.hiragana == other.hiragana
            && self.corresponding_count == other.corresponding_count
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
    RetriedAfterReconstruction,
}

impl NonIdempotentEditRecovery {
    fn log_value(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::RetriedAfterReconstruction => "retry_after_reconstruction",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WindowRpcDelivery {
    Sent,
    SkippedUnavailable,
}

impl WindowRpcDelivery {
    pub(crate) fn was_sent(self) -> bool {
        matches!(self, Self::Sent)
    }

    fn log_status(self) -> &'static str {
        match self {
            Self::Sent => "success",
            Self::SkippedUnavailable => "skipped_unavailable",
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
    /// Cloud ranking is stateless and must never occupy the TSF input thread or recovery ledger.
    pub(crate) fn rerank_candidates_background(
        &self,
        request: shared::proto::RerankCandidatesRequest,
        result: std::sync::mpsc::Sender<Option<usize>>,
    ) -> tokio::task::JoinHandle<()> {
        let mut client = self.azookey_client.clone();
        self.runtime.spawn(async move {
            let mut request = tonic::Request::new(request);
            request.set_timeout(Duration::from_millis(1800));
            let selected = tokio::time::timeout(
                Duration::from_millis(1800),
                client.rerank_candidates(request),
            )
            .await
            .ok()
            .and_then(Result::ok)
            .and_then(|response| response.into_inner().selected_index)
            .map(|index| index as usize);
            let _ = result.send(selected);
        })
    }

    #[cfg(test)]
    pub(crate) fn recovery_for_test(pending: bool) -> Self {
        let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
        let channel = {
            let _entered = runtime.enter();
            Endpoint::from_static("http://127.0.0.1:1").connect_lazy()
        };
        let (performance_log_tx, _) = tokio::sync::mpsc::channel(1);
        Self {
            connection_id: 0,
            azookey_client: AzookeyServiceClient::new(channel),
            window_client: None,
            runtime,
            performance_log_tx,
            server_session_id: None,
            server_reset_recovered: false,
            recovery: Arc::new(ServerRecoveryState {
                pending: AtomicBool::new(pending),
                generation: AtomicU64::new(u64::from(pending)),
                // Model a stalled worker without starting a launcher or server.
                restart_request_in_flight: AtomicBool::new(true),
                ..ServerRecoveryState::default()
            }),
            recovery_error_for_test: true,
            reconnect_channel_for_test: None,
            transport: Arc::new(TransportLifecycle::default()),
        }
    }

    #[cfg(test)]
    pub(crate) fn complete_restart_for_test(&self) {
        self.recovery.restart_completed_generation.store(
            self.recovery.generation.load(Ordering::Acquire),
            Ordering::Release,
        );
    }

    pub fn new() -> Result<Self> {
        let runtime = Arc::new(tokio::runtime::Runtime::new()?);
        let connection_id = IPC_CONNECTION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let transport = Arc::new(TransportLifecycle::default());

        let server_channel = Self::connect_named_pipe_channel(
            &runtime,
            "http://[::]:50051",
            shared::server_pipe_path()?,
            SERVER_PIPE_BUSY_TIMEOUT,
            Some(transport.clone()),
        )?;
        let window_client = match Self::connect_named_pipe_channel(
            &runtime,
            "http://[::]:50052",
            shared::ui_pipe_path()?,
            UI_PIPE_BUSY_TIMEOUT,
            None,
        ) {
            Ok(ui_channel) => Some(WindowServiceClient::new(ui_channel)),
            Err(error) => {
                tracing::warn!(
                    ?error,
                    "Candidate window IPC is unavailable; continuing without UI connection"
                );
                None
            }
        };

        let azookey_client = AzookeyServiceClient::new(server_channel);
        let (performance_log_tx, mut performance_log_rx) =
            tokio::sync::mpsc::channel::<PerformanceLogRequest>(64);
        let mut performance_log_client = azookey_client.clone();
        runtime.spawn(async move {
            while let Some(request) = performance_log_rx.recv().await {
                let mut request = tonic::Request::new(request);
                request.set_timeout(PERFORMANCE_RPC_DEADLINE);
                if let Err(error) = await_rpc_with_deadline(
                    "log_performance",
                    PERFORMANCE_RPC_DEADLINE,
                    performance_log_client.log_performance(request),
                )
                .await
                {
                    tracing::debug!("failed to write client performance log: {error:?}");
                }
            }
        });
        tracing::debug!("Connected to server: {:?}", azookey_client);

        Ok(Self {
            connection_id,
            azookey_client,
            window_client,
            runtime,
            performance_log_tx,
            server_session_id: None,
            server_reset_recovered: false,
            recovery: Arc::new(ServerRecoveryState::default()),
            transport,
            #[cfg(test)]
            recovery_error_for_test: false,
            #[cfg(test)]
            reconnect_channel_for_test: None,
        })
    }

    fn connect_named_pipe_channel(
        runtime: &tokio::runtime::Runtime,
        endpoint: &'static str,
        pipe_name: &'static str,
        busy_timeout: Duration,
        lifecycle: Option<Arc<TransportLifecycle>>,
    ) -> Result<Channel> {
        let endpoint = Endpoint::try_from(endpoint)?;
        let connect = endpoint.connect_with_connector(service_fn(move |_| {
            let lifecycle = lifecycle.clone();
            async move {
                if lifecycle.as_ref().is_some_and(|state| {
                    state.retired.load(Ordering::Acquire)
                        || state.opened.swap(true, Ordering::AcqRel)
                }) {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "IPC transport requires reconstruction",
                    ));
                }
                let busy_started_at = Instant::now();
                let client = loop {
                    match open_named_pipe_client(pipe_name) {
                        Ok(client) => break client,
                        Err(e)
                            if matches!(
                                e.raw_os_error(),
                                Some(code)
                                    if code == ERROR_PIPE_BUSY.0 as i32
                                        || code == ERROR_FILE_NOT_FOUND.0 as i32
                                        || code == ERROR_PATH_NOT_FOUND.0 as i32
                            ) =>
                        {
                            if busy_started_at.elapsed() >= busy_timeout {
                                return Err(std::io::Error::new(
                                    std::io::ErrorKind::TimedOut,
                                    format!(
                                    "{pipe_name} remained unavailable for at least {busy_timeout:?}"
                                ),
                                ));
                            }
                        }
                        Err(e) => return Err(e),
                    }

                    time::sleep(PIPE_BUSY_RETRY_INTERVAL).await;
                };

                let lifecycle = lifecycle.unwrap_or_default();
                lifecycle.opened.store(true, Ordering::Release);
                Ok::<_, std::io::Error>(TokioIo::new(RetirableIo::new(client, lifecycle)))
            }
        }));
        let channel = runtime.block_on(async {
            time::timeout(IPC_CONNECT_DEADLINE, connect)
                .await
                .map_err(|_| IpcDeadlineExceeded {
                    operation: "connect_named_pipe",
                    deadline: IPC_CONNECT_DEADLINE,
                })?
                .map_err(anyhow::Error::from)
        })?;

        Ok(channel)
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
                candidate_ids: composing_text
                    .suggestions
                    .iter()
                    .map(|s| s.candidate_id)
                    .collect(),
            })
        } else {
            anyhow::bail!("composing_text is None");
        }
    }

    fn reconnect(&mut self) -> anyhow::Result<()> {
        // A new server-assigned transport identity cannot refresh the old
        // composition. Only a complete edit ledger is trustworthy for replay.
        let ledger = self.input_ledger_snapshot().0;
        if !ledger.complete {
            // Successful clause edits can invalidate the relative ledger. The
            // existing recovery path rebuilds from the client's current raw
            // input and reconciles clause caches before replaying deferred input.
            self.require_server_recovery("reconnect_incomplete_ledger");
            return Err(self.recovery_pending_error());
        }
        self.reconnect_transport()?;
        self.send_replace_composition(&ledger, current_or_next_request_id())?;
        self.server_reset_recovered = false;
        Ok(())
    }

    fn reconnect_transport(&mut self) -> anyhow::Result<()> {
        self.transport.retire();
        #[cfg(test)]
        if let Some(channel) = self.reconnect_channel_for_test.as_ref() {
            self.azookey_client = AzookeyServiceClient::new(channel.clone());
            self.connection_id = IPC_CONNECTION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            self.transport = Arc::new(TransportLifecycle::default());
            return Ok(());
        }
        #[cfg(test)]
        if self.recovery_error_for_test {
            return Err(self.recovery_pending_error());
        }
        let refreshed = Self::new()?;
        self.connection_id = refreshed.connection_id;
        self.azookey_client = refreshed.azookey_client;
        self.window_client = refreshed.window_client;
        self.runtime = refreshed.runtime;
        self.performance_log_tx = refreshed.performance_log_tx;
        self.transport = refreshed.transport;
        Ok(())
    }

    fn mark_server_recovery_required(recovery: &Arc<ServerRecoveryState>, operation: &'static str) {
        Self::mark_recovery_pending(recovery);
        Self::request_server_restart(recovery, operation);
    }

    fn mark_recovery_pending(recovery: &ServerRecoveryState) {
        recovery.generation.fetch_add(1, Ordering::AcqRel);
        recovery.pending.store(true, Ordering::Release);
    }

    fn request_server_restart(recovery: &Arc<ServerRecoveryState>, operation: &'static str) {
        if recovery
            .restart_request_in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let generation = recovery.generation.load(Ordering::Acquire);
        let recovery = recovery.clone();
        std::thread::spawn(move || {
            match crate::launcher_control::request_restart() {
                Ok(()) => {
                    recovery
                        .restart_completed_generation
                        .fetch_max(generation, Ordering::AcqRel);
                    tracing::warn!(
                        operation,
                        generation,
                        "Launcher completed azookey server restart"
                    );
                }
                Err(error) => {
                    tracing::warn!(
                        ?error,
                        operation,
                        generation,
                        "Failed to request azookey server restart; next input will retry"
                    );
                }
            }
            recovery
                .restart_request_in_flight
                .store(false, Ordering::Release);
        });
    }

    pub(crate) fn recovery_pending(&self) -> bool {
        self.recovery.pending.load(Ordering::Acquire)
    }

    pub(crate) fn ensure_server_restart_requested(&self) {
        if restart_request_needed(
            self.recovery_pending(),
            self.recovery_restart_ready(),
            self.recovery
                .restart_request_in_flight
                .load(Ordering::Acquire),
        ) {
            Self::request_server_restart(&self.recovery, "recovery_retry");
        }
    }

    pub(crate) fn require_server_recovery(&self, operation: &'static str) {
        Self::mark_server_recovery_required(&self.recovery, operation);
    }

    pub(crate) fn recovery_restart_ready(&self) -> bool {
        let generation = self.recovery.generation.load(Ordering::Acquire);
        restart_generation_ready(
            generation,
            self.recovery
                .restart_completed_generation
                .load(Ordering::Acquire),
        )
    }

    pub(crate) fn recovery_pending_error(&self) -> anyhow::Error {
        IpcRecoveryPending {
            details: "waiting for launcher to complete server restart".to_string(),
        }
        .into()
    }

    fn record_successful_append(&self, text: &str, input_style: i32) {
        let Ok(mut ledger) = self.recovery.input_ledger.lock() else {
            return;
        };
        append_input_segment(&mut ledger, text, input_style);
    }

    fn record_successful_remove(&self) {
        let Ok(mut ledger) = self.recovery.input_ledger.lock() else {
            return;
        };
        pop_input_segment_character(&mut ledger);
    }

    fn record_successful_move(&self, offset: i32) {
        if offset == 0 {
            return;
        }
        if let Ok(mut ledger) = self.recovery.input_ledger.lock() {
            move_input_cursor(&mut ledger, offset);
        }
    }

    fn clear_input_ledger(&self) {
        if let Ok(mut ledger) = self.recovery.input_ledger.lock() {
            ledger.operations.clear();
            ledger.complete = true;
        }
    }

    pub(crate) fn discard_input_ledger(&self) {
        self.clear_input_ledger();
    }

    fn invalidate_input_ledger(&self) {
        if let Ok(mut ledger) = self.recovery.input_ledger.lock() {
            mark_input_ledger_incomplete(&mut ledger);
        }
    }

    pub(crate) fn input_ledger_snapshot(&self) -> InputLedgerSnapshot {
        let ledger = self
            .recovery
            .input_ledger
            .lock()
            .map(|ledger| ledger.clone())
            .unwrap_or_default();
        InputLedgerSnapshot(ledger)
    }

    pub(crate) fn restore_input_ledger(&self, snapshot: InputLedgerSnapshot) {
        if let Ok(mut ledger) = self.recovery.input_ledger.lock() {
            *ledger = snapshot.0;
        }
    }

    pub(crate) fn replace_input_ledger_direct(&self, text: &str) {
        if let Ok(mut ledger) = self.recovery.input_ledger.lock() {
            ledger.operations = if text.is_empty() {
                Vec::new()
            } else {
                vec![CompositionOperation::Append {
                    text: text.to_string(),
                    input_style: INPUT_STYLE_DIRECT,
                }]
            };
            ledger.complete = true;
        }
    }

    fn block_on_server_rpc<T, F>(
        runtime: &tokio::runtime::Runtime,
        recovery: &Arc<ServerRecoveryState>,
        operation: &'static str,
        deadline: Duration,
        future: F,
    ) -> anyhow::Result<T>
    where
        F: Future<Output = Result<T, tonic::Status>>,
    {
        let result = runtime.block_on(await_rpc_with_deadline(operation, deadline, future));
        if result.as_ref().is_err_and(is_ipc_deadline) {
            Self::mark_server_recovery_required(recovery, operation);
        }
        result
    }

    fn block_on_window_rpc<T, F>(
        runtime: &tokio::runtime::Runtime,
        operation: &'static str,
        future: F,
    ) -> anyhow::Result<T>
    where
        F: Future<Output = Result<T, tonic::Status>>,
    {
        runtime.block_on(await_rpc_with_deadline(operation, UI_RPC_DEADLINE, future))
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

                let reconnect = if matches!(operation, "clear_text" | "start_reconversion") {
                    self.reconnect_transport()
                } else {
                    self.reconnect()
                };
                match reconnect {
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
                    "{operation} first attempt failed, reconstructing composition before retry: {first_error:?}"
                );
                Ok(NonIdempotentEditAttempt::ReconnectAndRefresh(first_error))
            }
        }
    }

    #[inline]
    fn prepare_future_clauses_reconnect_state_is_valid(
        previous_candidates: &Candidates,
        refreshed_candidates: &Candidates,
    ) -> bool {
        !refreshed_candidates.is_empty_composition()
            && previous_candidates.has_same_composition(refreshed_candidates)
    }

    fn run_non_idempotent_edit_with_reconnect<T>(
        &mut self,
        operation: &str,
        mut send: impl FnMut(&mut Self) -> anyhow::Result<T>,
    ) -> anyhow::Result<(T, NonIdempotentEditRecovery)> {
        match Self::classify_non_idempotent_edit_attempt(operation, send(self))? {
            NonIdempotentEditAttempt::Completed(value) => {
                Ok((value, NonIdempotentEditRecovery::None))
            }
            NonIdempotentEditAttempt::ReconnectAndRefresh(first_error) => {
                match self.reconnect() {
                    Ok(()) => {
                        tracing::info!(
                            "{operation} IPC reconnect reconstructed the acknowledged composition"
                        );
                    }
                    Err(reconnect_error) => {
                        tracing::error!(
                            "{operation} IPC reconnect failed after first error {first_error:?}: {reconnect_error:?}"
                        );
                        return Err(reconnect_error);
                    }
                }

                // ReplaceComposition rebuilt the acknowledged pre-edit ledger,
                // so replay exactly once even if the old transport applied it.
                Ok((
                    send(self)?,
                    NonIdempotentEditRecovery::RetriedAfterReconstruction,
                ))
            }
        }
    }

    fn should_reconnect_rpc_error(error: &anyhow::Error) -> bool {
        if requires_ipc_recovery(error) {
            // A timeout may still apply the operation, while a typed pending
            // error means launcher-driven reconstruction is already required.
            // Neither is safe to replace with an immediate reconnect result.
            return false;
        }
        let Some(status) = error.downcast_ref::<tonic::Status>() else {
            return true;
        };

        matches!(
            status.code(),
            tonic::Code::Aborted
                | tonic::Code::Cancelled
                | tonic::Code::DataLoss
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
        let mut request = tonic::Request::new(shared::proto::AppendTextRequest {
            text_to_append: text.to_string(),
            input_style,
            request_id,
        });
        request.set_timeout(INPUT_RPC_DEADLINE);

        let response = Self::block_on_server_rpc(
            self.runtime.as_ref(),
            &self.recovery,
            "append_text",
            INPUT_RPC_DEADLINE,
            self.azookey_client.append_text(request),
        );
        let response = response?;
        let response = response.into_inner();
        self.observe_server_session("append_text", response.server_session_id);
        self.record_successful_append(text, input_style);
        Ok(response)
    }

    fn send_remove_text(&mut self, request_id: u64) -> anyhow::Result<TextRemoval> {
        let mut request = tonic::Request::new(shared::proto::RemoveTextRequest { request_id });
        request.set_timeout(INPUT_RPC_DEADLINE);
        let response = Self::block_on_server_rpc(
            self.runtime.as_ref(),
            &self.recovery,
            "remove_text",
            INPUT_RPC_DEADLINE,
            self.azookey_client.remove_text(request),
        )?;
        let response = response.into_inner();
        self.observe_server_session("remove_text", response.server_session_id);
        let Some(raw_input) = response.raw_input else {
            // A server built before raw-input synchronization cannot provide
            // enough information to keep mapped romaji deletion coherent.
            // Reconstruct on the current server instead of treating protobuf's
            // absent scalar as an empty composition.
            Self::mark_server_recovery_required(&self.recovery, "remove_text_missing_raw_input");
            return Err(preserve_recovery_error(anyhow::anyhow!(
                "remove_text response did not include canonical raw input"
            )));
        };
        self.record_successful_remove();
        Ok(TextRemoval {
            candidates: Self::candidates_from_composing_text(response.composing_text)?,
            raw_input,
        })
    }

    fn send_clear_text(&mut self, request_id: u64) -> anyhow::Result<()> {
        let mut request = tonic::Request::new(shared::proto::ClearTextRequest { request_id });
        request.set_timeout(STATE_RPC_DEADLINE);
        let response = Self::block_on_server_rpc(
            self.runtime.as_ref(),
            &self.recovery,
            "clear_text",
            STATE_RPC_DEADLINE,
            self.azookey_client.clear_text(request),
        )?;
        let response = response.into_inner();
        self.observe_server_session("clear_text", response.server_session_id);
        self.clear_input_ledger();
        Ok(())
    }

    fn send_start_reconversion(
        &mut self,
        surface: &str,
        request_id: u64,
    ) -> anyhow::Result<Option<ReconversionResult>> {
        let mut request = tonic::Request::new(StartReconversionRequest {
            surface: surface.to_string(),
            request_id,
        });
        request.set_timeout(INPUT_RPC_DEADLINE);
        let response = Self::block_on_server_rpc(
            self.runtime.as_ref(),
            &self.recovery,
            "start_reconversion",
            INPUT_RPC_DEADLINE,
            self.azookey_client.start_reconversion(request),
        )?
        .into_inner();
        self.observe_server_session("start_reconversion", response.server_session_id);
        if !response.applied {
            return Ok(None);
        }
        let candidates = Self::candidates_from_composing_text(response.composing_text)?;
        self.replace_input_ledger_direct(&candidates.hiragana);
        // This absolute response fully describes the composition on the observed
        // server session, so an older idle-session change must not cancel the new
        // reconversion when its first backend-dependent action is processed.
        self.server_reset_recovered = false;
        Ok(Some(ReconversionResult {
            candidates,
            selection_index: response.selection_index,
        }))
    }

    fn send_commit_learning_candidate(
        &mut self,
        candidate_id: u64,
        commit_kind: i32,
        request_id: u64,
    ) -> anyhow::Result<()> {
        let mut request = tonic::Request::new(shared::proto::CommitLearningCandidateRequest {
            candidate_id,
            commit_kind,
            request_id,
        });
        request.set_timeout(LEARNING_RPC_DEADLINE);
        let response = Self::block_on_server_rpc(
            self.runtime.as_ref(),
            &self.recovery,
            "commit_learning_candidate",
            LEARNING_RPC_DEADLINE,
            self.azookey_client.commit_learning_candidate(request),
        )?;
        let response = response.into_inner();
        self.observe_server_session("commit_learning_candidate", response.server_session_id);
        Ok(())
    }

    fn send_commit_learning_candidates(
        &mut self,
        commits: &[(u64, i32)],
        request_id: u64,
    ) -> anyhow::Result<u32> {
        let mut request = tonic::Request::new(shared::proto::CommitLearningCandidatesRequest {
            commits: commits
                .iter()
                .map(
                    |(candidate_id, commit_kind)| shared::proto::LearningCandidateCommit {
                        candidate_id: *candidate_id,
                        commit_kind: *commit_kind,
                    },
                )
                .collect(),
            request_id,
        });
        request.set_timeout(LEARNING_RPC_DEADLINE);
        let response = Self::block_on_server_rpc(
            self.runtime.as_ref(),
            &self.recovery,
            "commit_learning_candidates",
            LEARNING_RPC_DEADLINE,
            self.azookey_client.commit_learning_candidates(request),
        )?
        .into_inner();
        self.observe_server_session("commit_learning_candidates", response.server_session_id);
        Ok(response.committed_count)
    }

    fn send_shrink_text(&mut self, offset: i32, request_id: u64) -> anyhow::Result<Candidates> {
        let mut request =
            tonic::Request::new(shared::proto::ShrinkTextRequest { offset, request_id });
        request.set_timeout(INPUT_RPC_DEADLINE);
        let response = Self::block_on_server_rpc(
            self.runtime.as_ref(),
            &self.recovery,
            "shrink_text",
            INPUT_RPC_DEADLINE,
            self.azookey_client.shrink_text(request),
        )?;
        let response = response.into_inner();
        self.observe_server_session("shrink_text", response.server_session_id);
        self.invalidate_input_ledger();
        Self::candidates_from_composing_text(response.composing_text)
    }

    fn send_advance_clause(
        &mut self,
        offset: i32,
        selected_candidate_id: u64,
        request_id: u64,
    ) -> anyhow::Result<super::composition::ClauseAdvance> {
        let mut request = tonic::Request::new(shared::proto::AdvanceClauseRequest {
            offset,
            request_id,
            selected_candidate_id,
        });
        request.set_timeout(INPUT_RPC_DEADLINE);
        let response = Self::block_on_server_rpc(
            self.runtime.as_ref(),
            &self.recovery,
            "advance_clause",
            INPUT_RPC_DEADLINE,
            self.azookey_client.advance_clause(request),
        )?;
        let response = response.into_inner();
        self.observe_server_session("advance_clause", response.server_session_id);
        self.invalidate_input_ledger();
        let shrunk = Self::candidates_from_composing_text(response.shrunk_text)?;
        let navigation = Self::candidates_from_composing_text(response.navigation_text)?;
        let raw_input = if response.raw_input.is_empty() && !navigation.hiragana.is_empty() {
            super::composition::ClauseAdvanceRawInput::Unavailable
        } else {
            super::composition::ClauseAdvanceRawInput::Verified(response.raw_input)
        };
        Ok(super::composition::ClauseAdvance {
            shrunk,
            navigation,
            raw_input,
        })
    }

    fn send_prepare_future_clauses(
        &mut self,
        initial_offset: i32,
        initial_selected_candidate_id: u64,
        request_id: u64,
        leave_at_last: bool,
    ) -> anyhow::Result<(Vec<super::composition::ClauseAdvance>, bool)> {
        let mut request = tonic::Request::new(shared::proto::PrepareFutureClausesRequest {
            initial_offset,
            request_id,
            leave_at_last,
            initial_selected_candidate_id,
        });
        request.set_timeout(INPUT_RPC_DEADLINE);
        let response = Self::block_on_server_rpc(
            self.runtime.as_ref(),
            &self.recovery,
            "prepare_future_clauses",
            INPUT_RPC_DEADLINE,
            self.azookey_client.prepare_future_clauses(request),
        )?;
        let response = response.into_inner();
        self.observe_server_session("prepare_future_clauses", response.server_session_id);
        if leave_at_last && response.completed {
            self.invalidate_input_ledger();
        }
        let advances = response
            .advances
            .into_iter()
            .map(|advance| {
                Ok(super::composition::ClauseAdvance {
                    shrunk: Self::candidates_from_composing_text(advance.shrunk_text)?,
                    navigation: Self::candidates_from_composing_text(advance.navigation_text)?,
                    raw_input: super::composition::ClauseAdvanceRawInput::Unverified,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok((advances, response.completed))
    }

    fn send_move_cursor(&mut self, offset: i32, request_id: u64) -> anyhow::Result<Candidates> {
        let mut request =
            tonic::Request::new(shared::proto::MoveCursorRequest { offset, request_id });
        request.set_timeout(INPUT_RPC_DEADLINE);
        let response = Self::block_on_server_rpc(
            self.runtime.as_ref(),
            &self.recovery,
            "move_cursor",
            INPUT_RPC_DEADLINE,
            self.azookey_client.move_cursor(request),
        )?;
        let response = response.into_inner();
        self.observe_server_session("move_cursor", response.server_session_id);
        self.record_successful_move(offset);
        Self::candidates_from_composing_text(response.composing_text)
    }

    fn send_adjust_clause_boundary(
        &mut self,
        current_input_count: i32,
        direction: i32,
        expected_raw_input: &str,
        request_id: u64,
    ) -> anyhow::Result<super::composition::ClauseBoundaryAdjustment> {
        let mut request = tonic::Request::new(shared::proto::AdjustClauseBoundaryRequest {
            current_input_count,
            direction,
            request_id,
            expected_raw_input: expected_raw_input.to_string(),
        });
        request.set_timeout(INPUT_RPC_DEADLINE);
        let response = Self::block_on_server_rpc(
            self.runtime.as_ref(),
            &self.recovery,
            "adjust_clause_boundary",
            INPUT_RPC_DEADLINE,
            self.azookey_client.adjust_clause_boundary(request),
        )?
        .into_inner();
        self.observe_server_session("adjust_clause_boundary", response.server_session_id);
        if !response.applied {
            return Ok(super::composition::ClauseBoundaryAdjustment::skipped());
        }

        let candidates = Self::candidates_from_composing_text(response.composing_text)?;
        if candidates.texts.is_empty()
            || !candidates
                .corresponding_count
                .contains(&response.adjusted_input_count)
        {
            anyhow::bail!(
                "adjust_clause_boundary returned no candidate for input boundary {}",
                response.adjusted_input_count
            );
        }

        // Cursor materialization is absolute and may include a large jump from
        // the server's previous position. The recovery ledger stores only
        // relative edits, so do not replay an ambiguous pre-adjustment cursor.
        self.invalidate_input_ledger();
        Ok(super::composition::ClauseBoundaryAdjustment::applied(
            candidates,
            response.adjusted_input_count,
        ))
    }

    fn send_update_composition_snapshot(
        &mut self,
        operation: ClauseSnapshotOperation,
        selected_candidate_id: u64,
        request_id: u64,
    ) -> anyhow::Result<()> {
        let mut request = tonic::Request::new(shared::proto::UpdateCompositionSnapshotRequest {
            operation: operation.proto_value(),
            request_id,
            selected_candidate_id,
        });
        request.set_timeout(INPUT_RPC_DEADLINE);
        let response = Self::block_on_server_rpc(
            self.runtime.as_ref(),
            &self.recovery,
            "update_composition_snapshot",
            INPUT_RPC_DEADLINE,
            self.azookey_client.update_composition_snapshot(request),
        )?;
        let response = response.into_inner();
        self.observe_server_session("update_composition_snapshot", response.server_session_id);
        Ok(())
    }

    fn send_set_context(&mut self, context: &str, request_id: u64) -> anyhow::Result<()> {
        let mut request = tonic::Request::new(shared::proto::SetContextRequest {
            context: context.to_string(),
            request_id,
        });
        request.set_timeout(STATE_RPC_DEADLINE);
        let response = Self::block_on_server_rpc(
            self.runtime.as_ref(),
            &self.recovery,
            "set_context",
            STATE_RPC_DEADLINE,
            self.azookey_client.set_context(request),
        )?;
        let response = response.into_inner();
        self.observe_server_session("set_context", response.server_session_id);
        Ok(())
    }

    fn send_replace_composition(
        &mut self,
        input_ledger: &InputLedger,
        request_id: u64,
    ) -> anyhow::Result<Candidates> {
        let operations = input_ledger
            .operations
            .iter()
            .map(|operation| match operation {
                CompositionOperation::Append { text, input_style } => {
                    shared::proto::CompositionOperation {
                        kind: shared::proto::CompositionOperationKind::Append as i32,
                        text: text.clone(),
                        input_style: *input_style,
                        cursor_offset: 0,
                    }
                }
                CompositionOperation::Remove => shared::proto::CompositionOperation {
                    kind: shared::proto::CompositionOperationKind::Remove as i32,
                    text: String::new(),
                    input_style: INPUT_STYLE_ROMAN2KANA,
                    cursor_offset: 0,
                },
                CompositionOperation::MoveCursor(cursor_offset) => {
                    shared::proto::CompositionOperation {
                        kind: shared::proto::CompositionOperationKind::MoveCursor as i32,
                        text: String::new(),
                        input_style: INPUT_STYLE_ROMAN2KANA,
                        cursor_offset: *cursor_offset,
                    }
                }
            })
            .collect();
        let mut request = tonic::Request::new(shared::proto::ReplaceCompositionRequest {
            operations,
            request_id,
        });
        request.set_timeout(INPUT_RPC_DEADLINE);
        let response = Self::block_on_server_rpc(
            self.runtime.as_ref(),
            &self.recovery,
            "replace_composition",
            INPUT_RPC_DEADLINE,
            self.azookey_client.replace_composition(request),
        )?
        .into_inner();
        self.observe_server_session("replace_composition", response.server_session_id);
        self.recovery.context_epoch.fetch_add(1, Ordering::AcqRel);
        if let Ok(mut ledger) = self.recovery.input_ledger.lock() {
            *ledger = input_ledger.clone();
        }
        Self::candidates_from_composing_text(response.composing_text)
    }

    pub(crate) fn recover_composition_if_needed(
        &mut self,
        raw_input: &str,
        raw_hiragana: &str,
    ) -> anyhow::Result<Option<RecoveredComposition>> {
        if !self.recovery.pending.load(Ordering::Acquire) {
            return Ok(None);
        }
        if !self.recovery_restart_ready() {
            return Err(self.recovery_pending_error());
        }

        #[cfg(test)]
        if self.recovery_error_for_test && self.reconnect_channel_for_test.is_none() {
            return Err(self.recovery_pending_error());
        }

        let recovery_generation = self.recovery.generation.load(Ordering::Acquire);
        let input_ledger = self
            .recovery
            .input_ledger
            .lock()
            .ok()
            .filter(|ledger| ledger.complete)
            .map(|ledger| ledger.clone())
            .unwrap_or_else(|| fallback_input_ledger(raw_input, raw_hiragana));

        self.reconnect_transport()
            .map_err(preserve_recovery_error)?;
        let candidates = self
            .send_replace_composition(&input_ledger, current_or_next_request_id())
            .map_err(preserve_recovery_error)?;

        if !self.recovery_restart_ready() {
            return Err(self.recovery_pending_error());
        }

        // A second timeout may have started while this reconstruction was in
        // progress. Only the generation that we rebuilt is allowed to clear the
        // recovery flag; late results from an older generation are ignored.
        if recovery_generation_is_current(
            recovery_generation,
            self.recovery.generation.load(Ordering::Acquire),
        ) {
            self.recovery.pending.store(false, Ordering::Release);
        }
        self.server_reset_recovered = false;
        Ok(Some(RecoveredComposition { candidates }))
    }

    pub(crate) fn connection_id(&self) -> u64 {
        self.connection_id
    }

    pub(crate) fn context_cache_key(&self) -> (u64, u64) {
        (
            self.connection_id,
            self.recovery.context_epoch.load(Ordering::Acquire),
        )
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

        if let Err(error) = self.performance_log_tx.try_send(request) {
            tracing::debug!("dropped client performance log without blocking input: {error:?}");
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

    #[tracing::instrument]
    fn append_text_with_style_and_context(
        &mut self,
        text: String,
        input_style: i32,
        previous_candidates: Option<&Candidates>,
    ) -> anyhow::Result<Candidates> {
        if text.is_empty() {
            if let Some(candidates) = previous_candidates {
                // Empty AppendText is a claim-free handshake, not a candidate refresh.
                // A partial commit already received the remaining candidates from ShrinkText.
                return Ok(candidates.clone());
            }
        }

        let request_id = current_or_next_request_id();
        let performance_start = client_performance_start();
        let input_len = performance_start.map(|_| text.chars().count());
        let send = |this: &mut Self| this.send_append_text(&text, input_style, request_id);

        let response = match send(self) {
            Ok(response) => response,
            Err(first_error) => {
                if !Self::should_reconnect_rpc_error(&first_error) {
                    tracing::warn!(
                        "append_text failed without immediate replay (style={input_style}, text_len={}): {first_error:?}",
                        text.chars().count()
                    );
                    return Err(first_error);
                }
                tracing::warn!(
                    "append_text first attempt failed (style={input_style}, text_len={}), reconnecting IPC: {first_error:?}",
                    text.chars().count()
                );

                let reconnect = if text.is_empty() {
                    // The initialization handshake must stay claim-free even
                    // after a disconnect while another transport owns input.
                    self.reconnect_transport()
                } else {
                    self.reconnect()
                };
                match reconnect {
                    Ok(()) => {
                        tracing::info!("append_text IPC reconnect succeeded (style={input_style})");
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

                // The old input ledger survived the ambiguous failure and has
                // now been replaced absolutely. Append the new input once.
                send(self)?
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
    pub fn remove_text(&mut self) -> anyhow::Result<TextRemoval> {
        let request_id = current_or_next_request_id();
        let performance_start = client_performance_start();
        let result = self.run_non_idempotent_edit_with_reconnect("remove_text", |this| {
            this.send_remove_text(request_id)
        });
        self.log_client_performance_from_start(
            performance_start,
            request_id,
            "remove_text",
            "rpc_total",
            || match &result {
                Ok((_, recovery)) => {
                    format!("status=success;recovery={}", recovery.log_value())
                }
                Err(error) => format!("status=error;error={error:?}"),
            },
        );
        result.map(|(removal, _)| removal)
    }

    #[tracing::instrument]
    pub fn clear_text(&mut self) -> anyhow::Result<()> {
        // Shared by all IPC clones: ClearText also clears the server context.
        self.recovery.context_epoch.fetch_add(1, Ordering::AcqRel);
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
        match result {
            Ok(((), _)) => Ok(()),
            Err(error) if is_ipc_deadline(&error) => {
                // Clear is an absolute, idempotent desired state. The timeout
                // already requested a server restart, whose fresh process is
                // empty, so let the client finish clearing its own preedit.
                tracing::warn!(
                    ?error,
                    "Treating timed-out clear_text as best-effort success"
                );
                self.clear_input_ledger();
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    #[tracing::instrument(skip(self, surface))]
    pub(crate) fn start_reconversion(
        &mut self,
        surface: &str,
    ) -> anyhow::Result<Option<ReconversionResult>> {
        let request_id = current_or_next_request_id();
        let performance_start = client_performance_start();
        let result = self.run_rpc_with_reconnect("start_reconversion", |this| {
            this.send_start_reconversion(surface, request_id)
        });
        self.log_client_performance_from_start(
            performance_start,
            request_id,
            "start_reconversion",
            "rpc_total",
            || match &result {
                Ok((Some(reconversion), retried)) => format!(
                    "status=success;retry={retried};surface_len={};suggestions={}",
                    surface.chars().count(),
                    reconversion.candidates.texts.len()
                ),
                Ok((None, retried)) => format!(
                    "status=unsupported;retry={retried};surface_len={}",
                    surface.chars().count()
                ),
                Err(error) => format!(
                    "status=error;surface_len={};error={error:?}",
                    surface.chars().count()
                ),
            },
        );
        result.map(|(value, _)| value)
    }

    #[tracing::instrument]
    pub fn commit_learning_candidate(
        &mut self,
        candidate_id: u64,
        commit_kind: i32,
    ) -> anyhow::Result<()> {
        let request_id = current_or_next_request_id();
        let performance_start = client_performance_start();
        // Learning is an external side effect and the server has no dedupe
        // ledger. Never replay it after an ambiguous failure.
        let result = self.send_commit_learning_candidate(candidate_id, commit_kind, request_id);
        self.log_client_performance_from_start(
            performance_start,
            request_id,
            "commit_learning_candidate",
            "rpc_total",
            || match &result {
                Ok(()) => {
                    format!(
                        "status=success;retry=false;candidate_id={candidate_id};commit_kind={commit_kind}"
                    )
                }
                Err(error) => {
                    format!(
                        "status=error;candidate_id={candidate_id};commit_kind={commit_kind};error={error:?}"
                    )
                }
            },
        );
        result
    }

    #[tracing::instrument(skip(self, commits))]
    pub fn commit_learning_candidates(&mut self, commits: &[(u64, i32)]) -> anyhow::Result<()> {
        if commits.is_empty() {
            return Ok(());
        }

        let request_id = current_or_next_request_id();
        let performance_start = client_performance_start();
        // Learning is an external side effect and the server has no dedupe
        // ledger. Never replay a batch after an ambiguous failure.
        let result = self.send_commit_learning_candidates(commits, request_id);
        self.log_client_performance_from_start(
            performance_start,
            request_id,
            "commit_learning_candidates",
            "rpc_total",
            || match &result {
                Ok(committed_count) => format!(
                    "status=success;retry=false;requested_count={};committed_count={committed_count}",
                    commits.len()
                ),
                Err(error) => format!(
                    "status=error;requested_count={};error={error:?}",
                    commits.len()
                ),
            },
        );
        match result {
            Ok(committed_count) => {
                if usize::try_from(committed_count).unwrap_or_default() != commits.len() {
                    tracing::warn!(
                        requested_count = commits.len(),
                        committed_count,
                        "Some conversion learning candidates were not committed"
                    );
                }
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    #[tracing::instrument]
    pub fn shrink_text(&mut self, offset: i32) -> anyhow::Result<Candidates> {
        let request_id = current_or_next_request_id();
        let performance_start = client_performance_start();
        let result = self.run_non_idempotent_edit_with_reconnect("shrink_text", |this| {
            this.send_shrink_text(offset, request_id)
        });
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
    pub(crate) fn advance_clause(
        &mut self,
        offset: i32,
        selected_candidate_id: u64,
    ) -> anyhow::Result<super::composition::ClauseAdvance> {
        let request_id = current_or_next_request_id();
        let performance_start = client_performance_start();
        let result = self.run_non_idempotent_edit_with_reconnect("advance_clause", |this| {
            this.send_advance_clause(offset, selected_candidate_id, request_id)
        });
        self.log_client_performance_from_start(
            performance_start,
            request_id,
            "advance_clause",
            "rpc_total",
            || match &result {
                Ok((_, recovery)) => format!(
                    "status=success;recovery={};offset={offset}",
                    recovery.log_value()
                ),
                Err(error) => format!("status=error;offset={offset};error={error:?}"),
            },
        );
        result.map(|(advance, _)| advance)
    }

    #[tracing::instrument(skip(self, previous_candidates))]
    pub(crate) fn prepare_future_clauses(
        &mut self,
        initial_offset: i32,
        previous_candidates: &Candidates,
        initial_selected_candidate_id: u64,
        leave_at_last: bool,
    ) -> anyhow::Result<(Vec<super::composition::ClauseAdvance>, bool)> {
        let request_id = current_or_next_request_id();
        let performance_start = client_performance_start();
        let result = if leave_at_last {
            self.send_prepare_future_clauses(
                initial_offset,
                initial_selected_candidate_id,
                request_id,
                true,
            )
            .map_err(|error| {
                if Self::should_reconnect_rpc_error(&error) {
                    Self::mark_server_recovery_required(
                        &self.recovery,
                        "prepare_future_clauses_to_last_failed",
                    );
                    preserve_recovery_error(error)
                } else {
                    error
                }
            })
            .and_then(|prepared| {
                if self.take_server_reset_recovered() {
                    Self::mark_server_recovery_required(
                        &self.recovery,
                        "prepare_future_clauses_to_last_session_change",
                    );
                    Err(preserve_recovery_error(anyhow::anyhow!(
                        "prepare_future_clauses_to_last reached a different server session"
                    )))
                } else {
                    Ok(prepared)
                }
            })
        } else {
            self.run_rpc_with_reconnect("prepare_future_clauses", |this| {
                this.send_prepare_future_clauses(
                    initial_offset,
                    initial_selected_candidate_id,
                    request_id,
                    false,
                )
            })
            .map_err(|error| {
                if Self::should_reconnect_rpc_error(&error) {
                    Self::mark_server_recovery_required(
                        &self.recovery,
                        "prepare_future_clauses_reconnect_failed",
                    );
                    preserve_recovery_error(error)
                } else {
                    error
                }
            })
            .and_then(|(prepared, reconnected)| {
                if self.take_server_reset_recovered() {
                    Self::mark_server_recovery_required(
                        &self.recovery,
                        "prepare_future_clauses_session_change",
                    );
                    return Err(preserve_recovery_error(anyhow::anyhow!(
                        "prepare_future_clauses reached a different server session"
                    )));
                }

                if reconnected {
                    let refreshed = self
                        .send_move_cursor(0, request_id)
                        .map_err(preserve_recovery_error)?;
                    if !Self::prepare_future_clauses_reconnect_state_is_valid(
                        previous_candidates,
                        &refreshed,
                    ) {
                        Self::mark_server_recovery_required(
                            &self.recovery,
                            "prepare_future_clauses_state_mismatch",
                        );
                        return Err(preserve_recovery_error(anyhow::anyhow!(
                            "prepare_future_clauses composition changed after reconnect"
                        )));
                    }
                }

                Ok(prepared)
            })
        };
        self.log_client_performance_from_start(
            performance_start,
            request_id,
            "prepare_future_clauses",
            "rpc_total",
            || match &result {
                Ok((advances, completed)) => format!(
                    "status=success;initial_offset={initial_offset};advance_count={};leave_at_last={leave_at_last};completed={completed}",
                    advances.len(),
                ),
                Err(error) => {
                    format!("status=error;initial_offset={initial_offset};leave_at_last={leave_at_last};error={error:?}")
                }
            },
        );
        result
    }

    #[tracing::instrument]
    pub fn move_cursor(&mut self, offset: i32) -> anyhow::Result<Candidates> {
        let request_id = current_or_next_request_id();
        let performance_start = client_performance_start();
        let result = self.run_non_idempotent_edit_with_reconnect("move_cursor", |this| {
            this.send_move_cursor(offset, request_id)
        });
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

    #[tracing::instrument(skip(self, previous_candidates))]
    pub(crate) fn adjust_clause_boundary(
        &mut self,
        current_input_count: i32,
        direction: i32,
        expected_raw_input: &str,
        previous_candidates: &Candidates,
    ) -> anyhow::Result<super::composition::ClauseBoundaryAdjustment> {
        let request_id = current_or_next_request_id();
        let performance_start = client_performance_start();
        let result = self
            .run_rpc_with_reconnect("adjust_clause_boundary", |this| {
                this.send_adjust_clause_boundary(
                    current_input_count,
                    direction,
                    expected_raw_input,
                    request_id,
                )
            })
            .and_then(|(adjustment, reconnected)| {
                if reconnected
                    && adjustment.adjusted_input_count.is_some()
                    && !previous_candidates.hiragana.is_empty()
                    && adjustment.candidates.hiragana != previous_candidates.hiragana
                {
                    anyhow::bail!(
                        "adjust_clause_boundary reconnected into an unrelated composition"
                    );
                }
                Ok(adjustment)
            });
        self.log_client_performance_from_start(
            performance_start,
            request_id,
            "adjust_clause_boundary",
            "rpc_total",
            || match &result {
                Ok(adjustment) => format!(
                    "status=success;current_input_count={current_input_count};expected_input_count={};direction={direction};applied={};adjusted_input_count={}",
                    expected_raw_input.chars().count(),
                    adjustment.adjusted_input_count.is_some(),
                    adjustment
                        .adjusted_input_count
                        .map(|count| count.to_string())
                        .unwrap_or_else(|| "-".to_string())
                ),
                Err(error) => format!(
                    "status=error;current_input_count={current_input_count};expected_input_count={};direction={direction};error={error:?}",
                    expected_raw_input.chars().count()
                ),
            },
        );
        result
    }

    #[tracing::instrument(skip(self, _previous_candidates))]
    pub(crate) fn update_composition_snapshot(
        &mut self,
        operation: ClauseSnapshotOperation,
        _previous_candidates: &Candidates,
        selected_candidate_id: u64,
    ) -> anyhow::Result<()> {
        let request_id = current_or_next_request_id();
        let performance_start = client_performance_start();
        let first_attempt =
            self.send_update_composition_snapshot(operation, selected_candidate_id, request_id);
        let result: anyhow::Result<NonIdempotentEditRecovery> =
            (|| match Self::classify_non_idempotent_edit_attempt(
                "update_composition_snapshot",
                first_attempt,
            )? {
                NonIdempotentEditAttempt::Completed(()) => Ok(NonIdempotentEditRecovery::None),
                NonIdempotentEditAttempt::ReconnectAndRefresh(first_error) => {
                    self.reconnect().map_err(|reconnect_error| {
                        tracing::error!(
                            ?first_error,
                            ?reconnect_error,
                            "composition snapshot reconnect failed"
                        );
                        reconnect_error
                    })?;

                    if operation != ClauseSnapshotOperation::Clear {
                        // ReplaceComposition cannot rebuild the server snapshot
                        // stack or its candidate IDs. Never report a lost Pop as
                        // successful against the newly empty snapshot stack.
                        self.require_server_recovery("reconnect_lost_snapshots");
                        return Err(self.recovery_pending_error());
                    }
                    self.send_update_composition_snapshot(operation, 0, request_id)?;
                    Ok(NonIdempotentEditRecovery::RetriedAfterReconstruction)
                }
            })();
        self.log_client_performance_from_start(
            performance_start,
            request_id,
            "update_composition_snapshot",
            "rpc_total",
            || match &result {
                Ok(recovery) => format!(
                    "status=success;operation={operation:?};recovery={}",
                    recovery.log_value()
                ),
                Err(error) => {
                    format!("status=error;operation={operation:?};error={error:?}")
                }
            },
        );
        result.map(|_| ())
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
    fn ensure_window_client(
        &mut self,
        operation: &str,
    ) -> Option<&mut WindowServiceClient<Channel>> {
        if self.window_client.is_none() {
            match Self::connect_named_pipe_channel(
                self.runtime.as_ref(),
                "http://[::]:50052",
                shared::ui_pipe_path().ok()?,
                UI_PIPE_BUSY_TIMEOUT,
                None,
            ) {
                Ok(ui_channel) => {
                    tracing::info!(
                        operation,
                        "Candidate window IPC connected after deferred retry"
                    );
                    self.window_client = Some(WindowServiceClient::new(ui_channel));
                }
                Err(error) => {
                    tracing::debug!(
                        ?error,
                        operation,
                        "Candidate window IPC remains unavailable"
                    );
                    return None;
                }
            }
        }

        self.window_client.as_mut()
    }

    fn with_window_client_delivery(
        &mut self,
        operation: &str,
        send: impl FnOnce(
            &tokio::runtime::Runtime,
            &mut WindowServiceClient<Channel>,
        ) -> anyhow::Result<()>,
    ) -> anyhow::Result<WindowRpcDelivery> {
        let runtime = self.runtime.clone();
        let Some(window_client) = self.ensure_window_client(operation) else {
            return Ok(WindowRpcDelivery::SkippedUnavailable);
        };

        let result = send(runtime.as_ref(), window_client);
        if result.is_err() {
            self.window_client = None;
        }
        result.map(|()| WindowRpcDelivery::Sent)
    }

    fn with_window_client(
        &mut self,
        operation: &str,
        send: impl FnOnce(
            &tokio::runtime::Runtime,
            &mut WindowServiceClient<Channel>,
        ) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        self.with_window_client_delivery(operation, send)
            .map(|_| ())
    }

    fn ignore_window_rpc_error(operation: &str, result: anyhow::Result<()>) -> anyhow::Result<()> {
        if let Err(error) = result {
            tracing::warn!(
                ?error,
                operation,
                "Candidate window IPC request failed; continuing without UI connection"
            );
        }

        Ok(())
    }

    fn ignore_window_rpc_delivery_error(
        operation: &str,
        result: anyhow::Result<WindowRpcDelivery>,
    ) -> anyhow::Result<WindowRpcDelivery> {
        match result {
            Ok(delivery) => Ok(delivery),
            Err(error) => {
                tracing::warn!(
                    ?error,
                    operation,
                    "Candidate window IPC request failed; continuing without UI connection"
                );
                Ok(WindowRpcDelivery::SkippedUnavailable)
            }
        }
    }

    #[tracing::instrument]
    pub fn show_window(&mut self) -> anyhow::Result<()> {
        let request_id = current_or_next_request_id();
        let performance_start = client_performance_start();
        let result: anyhow::Result<()> = {
            let mut request = tonic::Request::new(shared::proto::EmptyResponse {});
            request.set_timeout(UI_RPC_DEADLINE);
            self.with_window_client("ui_show_window", |runtime, window_client| {
                Self::block_on_window_rpc(
                    runtime,
                    "ui_show_window",
                    window_client.show_window(request),
                )?;
                Ok(())
            })
        };
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
        Self::ignore_window_rpc_error("ui_show_window", result)
    }

    #[tracing::instrument]
    pub fn hide_window(&mut self) -> anyhow::Result<()> {
        let request_id = current_or_next_request_id();
        let performance_start = client_performance_start();
        let result: anyhow::Result<()> = {
            let mut request = tonic::Request::new(shared::proto::EmptyResponse {});
            request.set_timeout(UI_RPC_DEADLINE);
            self.with_window_client("ui_hide_window", |runtime, window_client| {
                Self::block_on_window_rpc(
                    runtime,
                    "ui_hide_window",
                    window_client.hide_window(request),
                )?;
                Ok(())
            })
        };
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
        Self::ignore_window_rpc_error("ui_hide_window", result)
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
        let result: anyhow::Result<()> = {
            let mut request = tonic::Request::new(shared::proto::SetPositionRequest {
                position: Some(shared::proto::WindowPosition {
                    top,
                    left,
                    bottom,
                    right,
                }),
            });
            request.set_timeout(UI_RPC_DEADLINE);
            self.with_window_client("ui_set_window_position", |runtime, window_client| {
                Self::block_on_window_rpc(
                    runtime,
                    "ui_set_window_position",
                    window_client.set_window_position(request),
                )?;
                Ok(())
            })
        };
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
        Self::ignore_window_rpc_error("ui_set_window_position", result)
    }

    #[tracing::instrument]
    pub fn set_candidates(&mut self, candidates: Vec<String>) -> anyhow::Result<()> {
        let request_id = current_or_next_request_id();
        let performance_start = client_performance_start();
        let candidate_count = performance_start.map(|_| candidates.len());
        let result: anyhow::Result<()> = {
            let mut request =
                tonic::Request::new(shared::proto::SetCandidateRequest { candidates });
            request.set_timeout(UI_RPC_DEADLINE);
            self.with_window_client("ui_set_candidates", |runtime, window_client| {
                Self::block_on_window_rpc(
                    runtime,
                    "ui_set_candidates",
                    window_client.set_candidate(request),
                )?;
                Ok(())
            })
        };
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
        Self::ignore_window_rpc_error("ui_set_candidates", result)
    }

    #[tracing::instrument]
    pub fn set_selection(&mut self, index: i32) -> anyhow::Result<()> {
        let request_id = current_or_next_request_id();
        let performance_start = client_performance_start();
        let result: anyhow::Result<()> = {
            let mut request = tonic::Request::new(shared::proto::SetSelectionRequest { index });
            request.set_timeout(UI_RPC_DEADLINE);
            self.with_window_client("ui_set_selection", |runtime, window_client| {
                Self::block_on_window_rpc(
                    runtime,
                    "ui_set_selection",
                    window_client.set_selection(request),
                )?;
                Ok(())
            })
        };
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
        Self::ignore_window_rpc_error("ui_set_selection", result)
    }

    #[tracing::instrument]
    pub fn set_input_mode(&mut self, mode: &str) -> anyhow::Result<()> {
        let request_id = current_or_next_request_id();
        let performance_start = client_performance_start();
        let result: anyhow::Result<()> = {
            let mut request = tonic::Request::new(shared::proto::SetInputModeRequest {
                mode: mode.to_string(),
            });
            request.set_timeout(UI_RPC_DEADLINE);
            self.with_window_client("ui_set_input_mode", |runtime, window_client| {
                Self::block_on_window_rpc(
                    runtime,
                    "ui_set_input_mode",
                    window_client.set_input_mode(request),
                )?;
                Ok(())
            })
        };
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
        Self::ignore_window_rpc_error("ui_set_input_mode", result)
    }

    #[tracing::instrument(skip(candidates))]
    pub(crate) fn update_candidate_window(
        &mut self,
        visible: Option<bool>,
        position: Option<shared::proto::WindowPosition>,
        candidates: Option<Vec<String>>,
        selected_index: Option<i32>,
        input_mode: Option<&str>,
    ) -> anyhow::Result<WindowRpcDelivery> {
        let clear_reading = visible == Some(false);
        self.update_candidate_window_with_reading(
            visible,
            position,
            candidates,
            selected_index,
            input_mode,
            clear_reading.then_some(""),
            clear_reading.then_some(false),
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    #[tracing::instrument(skip(candidates))]
    pub(crate) fn update_candidate_window_with_reading(
        &mut self,
        visible: Option<bool>,
        position: Option<shared::proto::WindowPosition>,
        candidates: Option<Vec<String>>,
        selected_index: Option<i32>,
        input_mode: Option<&str>,
        reading: Option<&str>,
        candidate_list_visible: Option<bool>,
        reading_vertical_adjustment: Option<i32>,
    ) -> anyhow::Result<WindowRpcDelivery> {
        let request_id = current_or_next_request_id();
        let performance_start = client_performance_start();
        let position_present = performance_start.map(|_| position.is_some());
        let candidate_count = performance_start.map(|_| candidates.as_ref().map(Vec::len));
        let input_mode_present = performance_start.map(|_| input_mode.is_some());
        let reading_present =
            performance_start.map(|_| reading.is_some_and(|value| !value.is_empty()));
        let result: anyhow::Result<WindowRpcDelivery> = {
            let mut request = tonic::Request::new(shared::proto::UpdateCandidateWindowRequest {
                visible,
                position,
                candidates: candidates
                    .map(|candidates| shared::proto::CandidateList { candidates }),
                selected_index,
                input_mode: input_mode.map(ToString::to_string),
                reading: reading.map(ToString::to_string),
                candidate_list_visible,
                reading_vertical_adjustment,
            });
            request.set_timeout(UI_RPC_DEADLINE);
            self.with_window_client_delivery(
                "ui_update_candidate_window",
                |runtime, window_client| {
                    Self::block_on_window_rpc(
                        runtime,
                        "ui_update_candidate_window",
                        window_client.update_candidate_window(request),
                    )?;
                    Ok(())
                },
            )
        };
        self.log_client_performance_from_start(
            performance_start,
            request_id,
            "ui_update_candidate_window",
            "rpc_total",
            || {
                let position_present = position_present.unwrap_or_default();
                let candidate_count = candidate_count.unwrap_or_default();
                let input_mode_present = input_mode_present.unwrap_or_default();
                let reading_present = reading_present.unwrap_or_default();
                match &result {
                    Ok(delivery) => format!(
                        "status={};visible={visible:?};position_present={position_present};candidate_count={candidate_count:?};selected_index={selected_index:?};input_mode_present={input_mode_present};reading_present={reading_present};candidate_list_visible={candidate_list_visible:?};reading_vertical_adjustment={reading_vertical_adjustment:?}",
                        delivery.log_status()
                    ),
                    Err(error) => format!(
                        "status=error;visible={visible:?};position_present={position_present};candidate_count={candidate_count:?};selected_index={selected_index:?};input_mode_present={input_mode_present};reading_present={reading_present};candidate_list_visible={candidate_list_visible:?};reading_vertical_adjustment={reading_vertical_adjustment:?};error={error:?}"
                    ),
                }
            },
        );
        Self::ignore_window_rpc_delivery_error("ui_update_candidate_window", result)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        append_input_segment, await_rpc_with_deadline, fallback_input_ledger,
        is_ipc_permission_denied, is_non_destructive_ipc_error, mark_input_ledger_incomplete,
        move_input_cursor, pop_input_segment_character, preserve_recovery_error,
        recovery_generation_is_current, requires_ipc_recovery, restart_generation_ready,
        restart_request_needed, Candidates, ClauseSnapshotOperation, CompositionOperation,
        IPCService, InputLedger, IpcDeadlineExceeded, NonIdempotentEditAttempt, RetirableIo,
        ServerRecoveryState, TransportLifecycle, INPUT_STYLE_DIRECT, INPUT_STYLE_ROMAN2KANA,
    };
    use std::{
        future::Future,
        pin::Pin,
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Arc,
        },
        task::{Context, Poll},
        time::Duration,
    };

    struct NeverResponds {
        dropped: Arc<AtomicBool>,
    }

    impl Future for NeverResponds {
        type Output = Result<(), tonic::Status>;

        fn poll(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Self::Output> {
            Poll::Pending
        }
    }

    impl Drop for NeverResponds {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::Release);
        }
    }

    #[test]
    fn retired_transport_closes_pipe_even_while_channel_clones_remain() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        struct WakeFlag(Arc<AtomicBool>);
        impl std::task::Wake for WakeFlag {
            fn wake(self: Arc<Self>) {
                self.0.store(true, Ordering::Release);
            }
        }
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let lifecycle = Arc::new(TransportLifecycle::default());
            let logging_clone = lifecycle.clone();
            let (client, mut server) = tokio::io::duplex(8);
            let mut client = RetirableIo::new(client, lifecycle.clone());
            server.write_all(b"a").await.unwrap();
            let mut buf = [0];
            client.read_exact(&mut buf).await.unwrap();
            assert_eq!(buf, [b'a']);

            // Poll a blocked read before retirement, as tonic does on an idle
            // channel. Retirement must wake it, rather than wait for more I/O.
            let mut read = Box::pin(client.read(&mut buf));
            let awakened = Arc::new(AtomicBool::new(false));
            let waker = std::task::Waker::from(Arc::new(WakeFlag(awakened.clone())));
            assert!(read
                .as_mut()
                .poll(&mut Context::from_waker(&waker))
                .is_pending());
            lifecycle.retire();
            assert!(awakened.load(Ordering::Acquire));
            let error = tokio::time::timeout(Duration::from_secs(1), read)
                .await
                .unwrap()
                .unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
            assert_eq!(server.read(&mut [0]).await.unwrap(), 0);
            assert!(logging_clone.retired.load(Ordering::Acquire));
        });
    }

    #[test]
    fn empty_contextual_append_preserves_partial_commit_candidates_without_rpc() {
        // The dummy transport cannot serve RPCs. Both input styles must keep the
        // ShrinkText response rather than replace it with an empty handshake response.
        let mut service = IPCService::recovery_for_test(false);
        service.record_successful_append("su", INPUT_STYLE_ROMAN2KANA);
        let ledger = service.input_ledger_snapshot().0;
        let remaining = Candidates {
            texts: vec!["す".into()],
            sub_texts: vec![String::new()],
            hiragana: "す".into(),
            corresponding_count: vec![2],
            candidate_ids: vec![7],
        };
        for direct in [false, true] {
            let actual = if direct {
                service.append_text_direct_with_context(String::new(), &remaining)
            } else {
                service.append_text_with_context(String::new(), &remaining)
            }
            .expect("empty contextual append must not make an RPC");
            assert!(actual.has_same_composition(&remaining));
            assert_eq!(actual.candidate_ids, remaining.candidate_ids);
            assert_eq!(service.input_ledger_snapshot().0, ledger);
            assert!(!service.recovery_pending());
        }
    }

    #[test]
    fn permission_denied_preserves_mutation_ledger_without_reconnect_or_restart() {
        let mut service = IPCService::recovery_for_test(false);
        service.record_successful_append("tya", INPUT_STYLE_ROMAN2KANA);
        service.record_successful_remove();
        service.record_successful_append("k", INPUT_STYLE_ROMAN2KANA);
        let before = service.input_ledger_snapshot().0;
        let mut attempts = 0;
        let error = service
            .run_non_idempotent_edit_with_reconnect::<Candidates>("remove_text", |_| {
                attempts += 1;
                Err(tonic::Status::permission_denied("another live owner").into())
            })
            .unwrap_err();

        assert_eq!(attempts, 1);
        assert!(is_ipc_permission_denied(&error));
        assert!(is_non_destructive_ipc_error(&error));
        assert!(!requires_ipc_recovery(&error));
        assert!(!service.recovery_pending());
        assert!(!service.transport.retired.load(Ordering::Acquire));
        assert_eq!(service.input_ledger_snapshot().0, before);
        let preserved = preserve_recovery_error(error);
        assert!(is_ipc_permission_denied(&preserved));
        assert!(!requires_ipc_recovery(&preserved));
    }

    #[test]
    fn reconnect_incomplete_ledger_defers_input_until_fallback_recovery_is_ready() {
        let mut service = IPCService::recovery_for_test(false);
        service.record_successful_append("ka", INPUT_STYLE_ROMAN2KANA);
        service.invalidate_input_ledger();
        let before = service.input_ledger_snapshot().0;
        let error = service.reconnect().unwrap_err();
        assert!(is_non_destructive_ipc_error(&error));
        assert!(requires_ipc_recovery(&error));
        assert!(service.recovery_pending());
        assert!(!service.recovery_restart_ready());
        assert_eq!(service.input_ledger_snapshot().0, before);
        service.complete_restart_for_test();
        assert!(service.recovery_restart_ready());
    }

    use shared::proto::{ReplaceCompositionRequest, ReplaceCompositionResponse};
    use tonic::codegen::{http, BoxFuture, Service};

    #[derive(Clone)]
    struct Probe {
        requests: Arc<std::sync::Mutex<Vec<ReplaceCompositionRequest>>>,
        deny: Arc<AtomicBool>,
        unavailable: Arc<AtomicBool>,
        missing_raw_input: Arc<AtomicBool>,
        append_requests: Arc<AtomicUsize>,
    }
    impl tonic::server::NamedService for Probe {
        const NAME: &'static str = "azookey.AzookeyService";
    }
    impl tonic::server::UnaryService<ReplaceCompositionRequest> for Probe {
        type Response = ReplaceCompositionResponse;
        type Future = BoxFuture<tonic::Response<Self::Response>, tonic::Status>;
        fn call(&mut self, request: tonic::Request<ReplaceCompositionRequest>) -> Self::Future {
            let request = request.into_inner();
            let reading = match request
                .operations
                .first()
                .map(|operation| operation.text.as_str())
            {
                Some("na") => "な",
                Some("kana") => "かな",
                _ => "",
            };
            self.requests.lock().unwrap().push(request);
            let denied = self.deny.load(Ordering::Acquire);
            Box::pin(async move {
                if denied {
                    return Err(tonic::Status::permission_denied("another live owner"));
                }
                Ok(tonic::Response::new(ReplaceCompositionResponse {
                    composing_text: Some(probe_composing_text(reading)),
                    server_session_id: 7,
                }))
            })
        }
    }
    fn probe_composing_text(reading: &str) -> shared::proto::ComposingText {
        shared::proto::ComposingText {
            hiragana: reading.to_string(),
            suggestions: vec![shared::proto::Suggestion {
                text: reading.to_string(),
                corresponding_count: reading.chars().count() as i32,
                candidate_id: 1,
                ..Default::default()
            }],
        }
    }

    impl tonic::server::UnaryService<shared::proto::RemoveTextRequest> for Probe {
        type Response = shared::proto::RemoveTextResponse;
        type Future = BoxFuture<tonic::Response<Self::Response>, tonic::Status>;
        fn call(&mut self, _: tonic::Request<shared::proto::RemoveTextRequest>) -> Self::Future {
            // Lose the first edit response, then accept its replay after replacement.
            let unavailable = self.unavailable.swap(false, Ordering::AcqRel);
            let missing_raw_input = self.missing_raw_input.load(Ordering::Acquire);
            Box::pin(async move {
                if unavailable {
                    return Err(tonic::Status::unavailable("edit response lost"));
                }
                Ok(tonic::Response::new(shared::proto::RemoveTextResponse {
                    composing_text: Some(probe_composing_text("t")),
                    raw_input: (!missing_raw_input).then(|| "ty".to_string()),
                    server_session_id: 7,
                }))
            })
        }
    }

    struct AdvanceProbe;
    impl tonic::server::UnaryService<shared::proto::AdvanceClauseRequest> for AdvanceProbe {
        type Response = shared::proto::AdvanceClauseResponse;
        type Future = BoxFuture<tonic::Response<Self::Response>, tonic::Status>;
        fn call(&mut self, _: tonic::Request<shared::proto::AdvanceClauseRequest>) -> Self::Future {
            Box::pin(async {
                Ok(tonic::Response::new(shared::proto::AdvanceClauseResponse {
                    shrunk_text: Some(probe_composing_text("な")),
                    navigation_text: Some(probe_composing_text("な")),
                    raw_input: "na".into(),
                    server_session_id: 7,
                }))
            })
        }
    }

    struct AdjustmentProbe;
    impl tonic::server::UnaryService<shared::proto::AdjustClauseBoundaryRequest> for AdjustmentProbe {
        type Response = shared::proto::AdjustClauseBoundaryResponse;
        type Future = BoxFuture<tonic::Response<Self::Response>, tonic::Status>;
        fn call(
            &mut self,
            _: tonic::Request<shared::proto::AdjustClauseBoundaryRequest>,
        ) -> Self::Future {
            Box::pin(async {
                Ok(tonic::Response::new(
                    shared::proto::AdjustClauseBoundaryResponse {
                        composing_text: Some(probe_composing_text("かな")),
                        applied: true,
                        adjusted_input_count: 2,
                        cursor_offset: 1,
                        server_session_id: 7,
                    },
                ))
            })
        }
    }

    impl Probe {
        // Match the Tonic error type used by the RPCs under test.
        #[allow(clippy::result_large_err)]
        fn mutation_status(&self) -> Result<(), tonic::Status> {
            if self.unavailable.load(Ordering::Acquire) {
                Err(tonic::Status::unavailable(
                    "transport closed after clause edit",
                ))
            } else if self.deny.load(Ordering::Acquire) {
                Err(tonic::Status::permission_denied("another live owner"))
            } else {
                Ok(())
            }
        }
    }
    impl tonic::server::UnaryService<shared::proto::AppendTextRequest> for Probe {
        type Response = shared::proto::AppendTextResponse;
        type Future = BoxFuture<tonic::Response<Self::Response>, tonic::Status>;
        fn call(
            &mut self,
            request: tonic::Request<shared::proto::AppendTextRequest>,
        ) -> Self::Future {
            self.append_requests.fetch_add(1, Ordering::Relaxed);
            let empty = request.into_inner().text_to_append.is_empty();
            let status = if empty {
                if self.unavailable.swap(false, Ordering::AcqRel) {
                    Err(tonic::Status::unavailable("handshake response lost"))
                } else {
                    Ok(())
                }
            } else {
                self.mutation_status()
            };
            Box::pin(async move {
                status?;
                Ok(tonic::Response::new(shared::proto::AppendTextResponse {
                    composing_text: Some(if empty {
                        shared::proto::ComposingText::default()
                    } else {
                        probe_composing_text("なk")
                    }),
                    server_session_id: 7,
                }))
            })
        }
    }
    impl tonic::server::UnaryService<shared::proto::UpdateCompositionSnapshotRequest> for Probe {
        type Response = shared::proto::UpdateCompositionSnapshotResponse;
        type Future = BoxFuture<tonic::Response<Self::Response>, tonic::Status>;
        fn call(
            &mut self,
            _: tonic::Request<shared::proto::UpdateCompositionSnapshotRequest>,
        ) -> Self::Future {
            let status = self.mutation_status();
            Box::pin(async move {
                status?;
                Ok(tonic::Response::new(
                    shared::proto::UpdateCompositionSnapshotResponse {
                        server_session_id: 7,
                    },
                ))
            })
        }
    }
    impl Service<http::Request<tonic::body::BoxBody>> for Probe {
        type Response = http::Response<tonic::body::BoxBody>;
        type Error = std::convert::Infallible;
        type Future = BoxFuture<Self::Response, Self::Error>;
        fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }
        fn call(&mut self, request: http::Request<tonic::body::BoxBody>) -> Self::Future {
            let path = request.uri().path().to_string();
            let probe = self.clone();
            Box::pin(async move {
                match path.rsplit('/').next().unwrap() {
                    "ReplaceComposition" => {
                        let mut grpc = tonic::server::Grpc::new(tonic::codec::ProstCodec::<
                            ReplaceCompositionResponse,
                            ReplaceCompositionRequest,
                        >::default(
                        ));
                        Ok(grpc.unary(probe, request).await)
                    }
                    "RemoveText" => {
                        let mut grpc = tonic::server::Grpc::new(tonic::codec::ProstCodec::<
                            shared::proto::RemoveTextResponse,
                            shared::proto::RemoveTextRequest,
                        >::default(
                        ));
                        Ok(grpc.unary(probe, request).await)
                    }
                    "AdvanceClause" => {
                        let mut grpc =
                            tonic::server::Grpc::new(tonic::codec::ProstCodec::default());
                        Ok(grpc.unary(AdvanceProbe, request).await)
                    }
                    "AdjustClauseBoundary" => {
                        let mut grpc =
                            tonic::server::Grpc::new(tonic::codec::ProstCodec::default());
                        Ok(grpc.unary(AdjustmentProbe, request).await)
                    }
                    "AppendText" => {
                        let mut grpc = tonic::server::Grpc::new(tonic::codec::ProstCodec::<
                            shared::proto::AppendTextResponse,
                            shared::proto::AppendTextRequest,
                        >::default(
                        ));
                        Ok(grpc.unary(probe, request).await)
                    }
                    "UpdateCompositionSnapshot" => {
                        let mut grpc = tonic::server::Grpc::new(tonic::codec::ProstCodec::<
                            shared::proto::UpdateCompositionSnapshotResponse,
                            shared::proto::UpdateCompositionSnapshotRequest,
                        >::default(
                        ));
                        Ok(grpc.unary(probe, request).await)
                    }
                    _ => panic!("unexpected probe RPC: {path}"),
                }
            })
        }
    }
    struct Incoming(tokio::net::TcpListener);
    impl tonic::codegen::tokio_stream::Stream for Incoming {
        type Item = std::io::Result<tokio::net::TcpStream>;
        fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            self.0
                .poll_accept(cx)
                .map(|result| Some(result.map(|(stream, _)| stream)))
        }
    }

    fn recovery_rpc_service() -> (
        IPCService,
        Probe,
        tokio::task::JoinHandle<Result<(), tonic::transport::Error>>,
    ) {
        let mut service = IPCService::recovery_for_test(false);
        let probe = Probe {
            requests: Arc::default(),
            deny: Arc::new(AtomicBool::new(true)),
            unavailable: Arc::new(AtomicBool::new(false)),
            missing_raw_input: Arc::new(AtomicBool::new(false)),
            append_requests: Arc::default(),
        };
        let listener = service
            .runtime
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = service.runtime.spawn(
            tonic::transport::Server::builder()
                .add_service(probe.clone())
                .serve_with_incoming(Incoming(listener)),
        );
        let channel = service
            .runtime
            .block_on(
                tonic::transport::Endpoint::from_shared(format!("http://{address}"))
                    .unwrap()
                    .connect(),
            )
            .unwrap();
        service.reconnect_channel_for_test = Some(channel.clone());
        service.azookey_client =
            shared::proto::azookey_service_client::AzookeyServiceClient::new(channel);
        (service, probe, server)
    }

    #[test]
    fn bare_empty_append_keeps_claim_free_handshake_with_or_without_reconnect() {
        for lose_response in [false, true] {
            let (mut service, probe, server) = recovery_rpc_service();
            probe.unavailable.store(lose_response, Ordering::Release);
            service.record_successful_append("kana", INPUT_STYLE_ROMAN2KANA);
            let before = service.input_ledger_snapshot().0;

            let candidates = service.append_text(String::new()).unwrap();
            assert!(candidates.is_empty_composition());
            assert_eq!(
                probe.append_requests.load(Ordering::Relaxed),
                1 + usize::from(lose_response)
            );
            assert!(
                probe.requests.lock().unwrap().is_empty(),
                "a handshake must not claim input through replacement"
            );
            assert_eq!(service.input_ledger_snapshot().0, before);
            assert!(!service.recovery_pending());
            server.abort();
        }
    }

    #[test]
    fn removal_returns_canonical_raw_input_with_or_without_reconstruction() {
        for lose_response in [false, true] {
            let (mut service, probe, server) = recovery_rpc_service();
            probe.deny.store(false, Ordering::Release);
            probe.unavailable.store(lose_response, Ordering::Release);
            service.record_successful_append("tya", INPUT_STYLE_ROMAN2KANA);

            let removal = service.remove_text().unwrap();
            assert_eq!(removal.raw_input, "ty");
            assert_eq!(removal.candidates.hiragana, "t");
            assert!(!service.recovery_pending());
            let requests = probe.requests.lock().unwrap();
            assert_eq!(requests.len(), usize::from(lose_response));
            if lose_response {
                assert_eq!(requests[0].operations.len(), 1);
                assert_eq!(requests[0].operations[0].text, "tya");
            }
            let ledger = service.input_ledger_snapshot().0;
            assert!(ledger.complete);
            assert_eq!(ledger.operations.len(), 2);
            assert_eq!(ledger.operations[1], CompositionOperation::Remove);
            server.abort();
        }
    }

    #[test]
    fn removal_without_canonical_raw_input_requires_recovery_without_replay() {
        let (mut service, probe, server) = recovery_rpc_service();
        probe.missing_raw_input.store(true, Ordering::Release);
        service.record_successful_append("tya", INPUT_STYLE_ROMAN2KANA);
        let before = service.input_ledger_snapshot().0;

        let error = service.remove_text().unwrap_err();
        assert!(requires_ipc_recovery(&error));
        assert!(service.recovery_pending());
        assert_eq!(service.input_ledger_snapshot().0, before);
        assert!(probe.requests.lock().unwrap().is_empty());
        server.abort();
    }

    #[test]
    fn clause_advance_returns_complete_response_after_reconstruction() {
        let (mut service, probe, server) = recovery_rpc_service();
        probe.deny.store(false, Ordering::Release);
        service.record_successful_append("kana", INPUT_STYLE_ROMAN2KANA);
        let mut attempts = 0;
        let (advance, recovery) = service
            .run_non_idempotent_edit_with_reconnect("advance_clause", |this| {
                attempts += 1;
                if attempts == 1 {
                    return Err(tonic::Status::unavailable("edit response lost").into());
                }
                this.send_advance_clause(2, 1, 123)
            })
            .unwrap();
        assert_eq!(attempts, 2);
        assert_eq!(
            recovery,
            super::NonIdempotentEditRecovery::RetriedAfterReconstruction
        );
        assert_eq!(advance.shrunk.hiragana, "な");
        assert_eq!(advance.navigation.hiragana, "な");
        assert!(matches!(advance.raw_input,
            crate::engine::composition::ClauseAdvanceRawInput::Verified(ref raw) if raw == "na"));
        assert_eq!(probe.requests.lock().unwrap().len(), 1);
        server.abort();
    }

    #[test]
    fn replacement_rpc_replays_mixed_mutations_and_preserves_ledger_on_owner_denial() {
        let (mut service, probe, server) = recovery_rpc_service();
        service.record_successful_append("tya", INPUT_STYLE_ROMAN2KANA);
        service.record_successful_remove();
        service.record_successful_append("あ", INPUT_STYLE_DIRECT);
        service.record_successful_move(-1);
        service.record_successful_append("k", INPUT_STYLE_ROMAN2KANA);
        let before = service.input_ledger_snapshot().0;
        assert!(before.complete);

        let error = service.send_replace_composition(&before, 123).unwrap_err();
        assert!(is_ipc_permission_denied(&error));
        assert!(!service.recovery_pending());
        assert_eq!(service.input_ledger_snapshot().0, before);
        probe.deny.store(false, Ordering::Release);
        service.send_replace_composition(&before, 123).unwrap();
        let requests = probe.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0], requests[1]);
        let operations = &requests[1].operations;
        assert_eq!(operations.len(), 5);
        assert_eq!(operations[0].text, "tya");
        assert_eq!(operations[0].input_style, INPUT_STYLE_ROMAN2KANA);
        assert_eq!(
            operations[1].kind,
            shared::proto::CompositionOperationKind::Remove as i32
        );
        assert_eq!(operations[2].text, "あ");
        assert_eq!(operations[2].input_style, INPUT_STYLE_DIRECT);
        assert_eq!(
            operations[3].kind,
            shared::proto::CompositionOperationKind::MoveCursor as i32
        );
        assert_eq!(operations[3].cursor_offset, -1);
        assert_eq!(operations[4].text, "k");
        assert_eq!(service.input_ledger_snapshot().0, before);
        server.abort();
    }

    #[test]
    fn successful_clause_edits_then_unavailable_transport_recover_and_continue_input() {
        for boundary_adjustment in [false, true] {
            let (mut service, probe, server) = recovery_rpc_service();
            probe.deny.store(false, Ordering::Release);
            service.record_successful_append("kana", INPUT_STYLE_ROMAN2KANA);
            let (raw_input, reading) = if boundary_adjustment {
                let adjustment = service
                    .send_adjust_clause_boundary(1, 1, "kana", 1)
                    .unwrap();
                assert_eq!(adjustment.adjusted_input_count, Some(2));
                ("kana", "かな")
            } else {
                let advance = service.send_advance_clause(2, 1, 1).unwrap();
                assert_eq!(advance.navigation.hiragana, "な");
                ("na", "な")
            };
            let before = service.input_ledger_snapshot().0;
            assert!(!before.complete);

            // A live owner's denial must never request fallback/restart even
            // when a preceding successful clause edit left the ledger incomplete.
            probe.deny.store(true, Ordering::Release);
            let denied = service.append_text("k".into()).unwrap_err();
            assert!(is_ipc_permission_denied(&denied));
            assert!(!service.recovery_pending());
            assert_eq!(service.input_ledger_snapshot().0, before);

            probe.deny.store(false, Ordering::Release);
            probe.unavailable.store(true, Ordering::Release);
            let error = service.append_text("k".into()).unwrap_err();
            assert!(requires_ipc_recovery(&error));
            assert!(service.recovery_pending());
            assert!(!service.recovery_restart_ready());
            assert_eq!(service.input_ledger_snapshot().0, before);
            assert!(probe.requests.lock().unwrap().is_empty());

            service.complete_restart_for_test();
            let recovered = service
                .recover_composition_if_needed(raw_input, reading)
                .unwrap()
                .unwrap();
            assert_eq!(recovered.candidates.hiragana, reading);
            assert!(!service.recovery_pending());
            let ledger = service.input_ledger_snapshot().0;
            assert!(ledger.complete);
            assert_eq!(ledger, fallback_input_ledger(raw_input, reading));
            let requests = probe.requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].operations[0].text, raw_input);
            assert_eq!(
                requests[0].operations[0].input_style,
                INPUT_STYLE_ROMAN2KANA
            );
            drop(requests);

            probe.unavailable.store(false, Ordering::Release);
            assert!(!service
                .append_text("k".into())
                .unwrap()
                .is_empty_composition());
            let ledger = service.input_ledger_snapshot().0;
            assert!(ledger.complete);
            assert_eq!(ledger.operations.len(), 2);
            assert_eq!(
                ledger.operations[1],
                CompositionOperation::Append {
                    text: "k".into(),
                    input_style: INPUT_STYLE_ROMAN2KANA,
                }
            );
            server.abort();
        }
    }

    #[test]
    fn lost_snapshot_after_transport_failure_enters_typed_recovery() {
        let (mut service, probe, server) = recovery_rpc_service();
        service.record_successful_append("kana", INPUT_STYLE_ROMAN2KANA);
        let before = service.input_ledger_snapshot().0;
        let denied = service
            .update_composition_snapshot(ClauseSnapshotOperation::Pop, &Candidates::default(), 1)
            .unwrap_err();
        assert!(is_ipc_permission_denied(&denied));
        assert!(!service.recovery_pending());
        probe.unavailable.store(true, Ordering::Release);
        // If reconnect reaches a different live owner, ReplaceComposition is
        // denied. Do not treat that as a lost-snapshot reason to restart it.
        let denied = service
            .update_composition_snapshot(ClauseSnapshotOperation::Pop, &Candidates::default(), 1)
            .unwrap_err();
        assert!(is_ipc_permission_denied(&denied));
        assert!(!service.recovery_pending());
        assert_eq!(service.input_ledger_snapshot().0, before);
        probe.deny.store(false, Ordering::Release);
        let error = service
            .update_composition_snapshot(ClauseSnapshotOperation::Pop, &Candidates::default(), 1)
            .unwrap_err();
        assert!(requires_ipc_recovery(&error));
        assert!(service.recovery_pending());
        assert_eq!(service.input_ledger_snapshot().0, before);
        service.complete_restart_for_test();
        assert!(service
            .recover_composition_if_needed("kana", "かな")
            .unwrap()
            .is_some());
        assert!(!service.recovery_pending());
        probe.unavailable.store(false, Ordering::Release);
        assert!(service.append_text("k".into()).is_ok());
        server.abort();
    }

    #[test]
    fn clear_attempt_invalidates_context_dedup_in_all_client_clones() {
        let mut service = IPCService::recovery_for_test(false);
        let other_clone = service.clone();
        let mut context = crate::tsf::text_service::SurroundingTextContextState::default();
        context.remember(other_clone.context_cache_key(), "same preceding text");
        assert!(!context.should_send(service.context_cache_key(), "same preceding text"));
        // The fake channel cannot succeed; even an ambiguous/failed release
        // must not leave a successful SetContext entry eligible for dedup.
        let _ = service.clear_text();
        assert!(context.should_send(other_clone.context_cache_key(), "same preceding text"));
    }

    #[test]
    fn timeout_cancels_and_drops_inflight_rpc_future() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime should initialize");
        let dropped = Arc::new(AtomicBool::new(false));
        let error = runtime
            .block_on(await_rpc_with_deadline(
                "fault_never_responds",
                Duration::from_millis(5),
                NeverResponds {
                    dropped: dropped.clone(),
                },
            ))
            .expect_err("hung RPC should hit its deadline");

        assert!(error.downcast_ref::<IpcDeadlineExceeded>().is_some());
        assert!(dropped.load(Ordering::Acquire));
    }

    #[test]
    fn response_arriving_after_cancel_cannot_complete_old_request() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime should initialize");
        let late_delivery_failed = runtime.block_on(async {
            let (sender, receiver) = tokio::sync::oneshot::channel::<()>();
            let late_sender = tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(20)).await;
                sender.send(()).is_err()
            });

            let result = await_rpc_with_deadline(
                "fault_late_response",
                Duration::from_millis(5),
                async move {
                    receiver
                        .await
                        .map_err(|_| tonic::Status::cancelled("receiver dropped"))
                },
            )
            .await;
            assert!(result.is_err());
            // The response receiver was part of the cancelled RPC future.
            // A late response has no route back into client state.
            late_sender.await.expect("late sender task should finish")
        });

        assert!(late_delivery_failed);
    }

    #[test]
    fn server_restart_recovery_ignores_stale_generation_completion() {
        assert!(recovery_generation_is_current(7, 7));
        assert!(!recovery_generation_is_current(7, 8));
    }

    #[test]
    fn explicit_recovery_marks_pending_and_advances_generation() {
        let recovery = ServerRecoveryState::default();

        IPCService::mark_recovery_pending(&recovery);

        assert!(recovery.pending.load(Ordering::Acquire));
        assert_eq!(recovery.generation.load(Ordering::Acquire), 1);
    }

    #[test]
    fn recovery_waits_for_the_requested_restart_generation() {
        assert!(!restart_generation_ready(0, 0));
        assert!(!restart_generation_ready(8, 7));
        assert!(restart_generation_ready(8, 8));
        assert!(restart_generation_ready(8, 9));
    }

    #[test]
    fn failed_launcher_request_is_retried_on_later_input() {
        assert!(restart_request_needed(true, false, false));
        assert!(!restart_request_needed(true, false, true));
        assert!(!restart_request_needed(true, true, false));
        assert!(!restart_request_needed(false, false, false));
    }

    #[test]
    fn mixed_input_ledger_preserves_order_and_style() {
        let mut ledger = InputLedger {
            complete: true,
            ..InputLedger::default()
        };
        append_input_segment(&mut ledger, "k", INPUT_STYLE_ROMAN2KANA);
        append_input_segment(&mut ledger, "あ", INPUT_STYLE_DIRECT);
        append_input_segment(&mut ledger, "a", INPUT_STYLE_ROMAN2KANA);

        assert_eq!(
            ledger.operations,
            vec![
                CompositionOperation::Append {
                    text: "k".to_string(),
                    input_style: INPUT_STYLE_ROMAN2KANA,
                },
                CompositionOperation::Append {
                    text: "あ".to_string(),
                    input_style: INPUT_STYLE_DIRECT,
                },
                CompositionOperation::Append {
                    text: "a".to_string(),
                    input_style: INPUT_STYLE_ROMAN2KANA,
                },
            ]
        );
    }

    #[test]
    fn input_ledger_records_successful_mutations_in_order() {
        let mut ledger = InputLedger {
            complete: true,
            ..InputLedger::default()
        };
        append_input_segment(&mut ledger, "ka", INPUT_STYLE_ROMAN2KANA);
        move_input_cursor(&mut ledger, -1);
        append_input_segment(&mut ledger, "あ", INPUT_STYLE_DIRECT);
        pop_input_segment_character(&mut ledger);

        assert_eq!(
            ledger.operations,
            vec![
                CompositionOperation::Append {
                    text: "ka".to_string(),
                    input_style: INPUT_STYLE_ROMAN2KANA,
                },
                CompositionOperation::MoveCursor(-1),
                CompositionOperation::Append {
                    text: "あ".to_string(),
                    input_style: INPUT_STYLE_DIRECT,
                },
                CompositionOperation::Remove,
            ]
        );
    }

    #[test]
    fn input_ledger_preserves_full_i32_cursor_offsets() {
        let mut ledger = InputLedger {
            complete: true,
            ..InputLedger::default()
        };

        for offset in [125, 126, 127, 128, 129, 1024, -125, i32::MIN] {
            move_input_cursor(&mut ledger, offset);
        }

        assert_eq!(
            ledger.operations,
            vec![
                CompositionOperation::MoveCursor(125),
                CompositionOperation::MoveCursor(126),
                CompositionOperation::MoveCursor(127),
                CompositionOperation::MoveCursor(128),
                CompositionOperation::MoveCursor(129),
                CompositionOperation::MoveCursor(1024),
                CompositionOperation::MoveCursor(-125),
                CompositionOperation::MoveCursor(i32::MIN),
            ]
        );
    }

    #[test]
    fn snapshot_operations_have_distinct_proto_values() {
        assert_eq!(
            ClauseSnapshotOperation::Clear.proto_value(),
            shared::proto::CompositionSnapshotOperation::Clear as i32
        );
        assert_eq!(
            ClauseSnapshotOperation::Push.proto_value(),
            shared::proto::CompositionSnapshotOperation::Push as i32
        );
        assert_eq!(
            ClauseSnapshotOperation::Pop.proto_value(),
            shared::proto::CompositionSnapshotOperation::Pop as i32
        );
    }

    #[test]
    fn server_restart_connection_gap_preserves_client_composition() {
        let error = preserve_recovery_error(anyhow::anyhow!(
            "named pipe is briefly absent while launcher restarts server"
        ));

        assert!(is_non_destructive_ipc_error(&error));
        assert!(error.to_string().contains("recovery is still pending"));
    }

    #[test]
    fn deadline_never_uses_immediate_retry_policy() {
        let error = anyhow::Error::new(IpcDeadlineExceeded {
            operation: "append_text",
            deadline: Duration::from_secs(2),
        });

        assert!(!IPCService::should_reconnect_rpc_error(&error));
    }

    #[test]
    fn pending_recovery_never_uses_immediate_retry_policy() {
        let error = preserve_recovery_error(anyhow::anyhow!(
            "server response requires absolute-state reconstruction"
        ));

        assert!(requires_ipc_recovery(&error));
        assert!(!IPCService::should_reconnect_rpc_error(&error));
    }

    #[test]
    fn grpc_deadline_status_never_replays_non_idempotent_rpc() {
        let error = anyhow::Error::new(tonic::Status::deadline_exceeded("server timed out"));

        assert!(is_non_destructive_ipc_error(&error));
        assert!(!IPCService::should_reconnect_rpc_error(&error));
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
        assert!(is_non_destructive_ipc_error(&error));
        assert!(!requires_ipc_recovery(&error));
    }

    #[test]
    fn reconnect_retry_is_enabled_for_non_status_error() {
        let error = anyhow::anyhow!("named pipe disconnected");

        assert!(IPCService::should_reconnect_rpc_error(&error));
    }

    #[test]
    fn prepare_future_clauses_retry_accepts_unchanged_composition() {
        let candidates = Candidates {
            texts: vec!["いい加減".to_string()],
            sub_texts: vec!["統一".to_string()],
            hiragana: "いいかげんとういつ".to_string(),
            corresponding_count: vec![7],
            candidate_ids: vec![1],
        };
        let refreshed = Candidates {
            candidate_ids: vec![2],
            ..candidates.clone()
        };

        assert!(IPCService::prepare_future_clauses_reconnect_state_is_valid(
            &candidates,
            &refreshed
        ));
    }

    #[test]
    fn prepare_future_clauses_retry_rejects_reset_server_state() {
        let previous = Candidates {
            texts: vec!["いい加減".to_string()],
            sub_texts: vec!["統一".to_string()],
            hiragana: "いいかげんとういつ".to_string(),
            corresponding_count: vec![7],
            candidate_ids: vec![1],
        };

        assert!(
            !IPCService::prepare_future_clauses_reconnect_state_is_valid(
                &previous,
                &Candidates::default()
            )
        );
    }

    #[test]
    fn non_idempotent_edit_attempt_completes_without_recovery_on_success() {
        let candidates = Candidates {
            texts: vec!["か".to_string()],
            sub_texts: vec![String::new()],
            hiragana: "か".to_string(),
            corresponding_count: vec![1],
            candidate_ids: vec![1],
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

    #[test]
    fn ambiguous_non_idempotent_refresh_invalidates_input_ledger() {
        let mut ledger = InputLedger {
            operations: vec![
                CompositionOperation::Append {
                    text: "かな".to_string(),
                    input_style: 0,
                },
                CompositionOperation::MoveCursor(-1),
            ],
            complete: true,
        };

        mark_input_ledger_incomplete(&mut ledger);

        assert!(ledger.operations.is_empty());
        assert!(!ledger.complete);
    }

    #[test]
    fn move_to_last_recovery_uses_current_suffix_after_ledger_invalidation() {
        let mut ledger = InputLedger {
            operations: vec![CompositionOperation::Append {
                text: "aruteidonagaibunsyou".to_string(),
                input_style: INPUT_STYLE_ROMAN2KANA,
            }],
            complete: true,
        };

        mark_input_ledger_incomplete(&mut ledger);
        let recovery_ledger = if ledger.complete {
            ledger
        } else {
            fallback_input_ledger("bunsyou", "ぶんしょう")
        };

        assert_eq!(
            recovery_ledger.operations,
            vec![CompositionOperation::Append {
                text: "bunsyou".to_string(),
                input_style: INPUT_STYLE_ROMAN2KANA,
            }]
        );
        assert!(recovery_ledger.complete);
    }

    #[test]
    fn fallback_input_ledger_preserves_pending_romaji_state() {
        let ledger = fallback_input_ledger("k", "k");

        assert_eq!(
            ledger.operations,
            vec![CompositionOperation::Append {
                text: "k".to_string(),
                input_style: INPUT_STYLE_ROMAN2KANA,
            }]
        );
        assert!(ledger.complete);
    }

    #[test]
    fn fallback_input_ledger_uses_direct_reading_without_raw_input() {
        let ledger = fallback_input_ledger("", "かな");

        assert_eq!(
            ledger.operations,
            vec![CompositionOperation::Append {
                text: "かな".to_string(),
                input_style: INPUT_STYLE_DIRECT,
            }]
        );
        assert!(ledger.complete);
    }
}
