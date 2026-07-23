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
    error::Error as StdError,
    fmt,
    future::Future,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};
use tokio::{net::windows::named_pipe::ClientOptions, time};
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
}

#[derive(Debug)]
struct ServerRecoveryState {
    pending: AtomicBool,
    generation: AtomicU64,
    restart_completed_generation: AtomicU64,
    restart_request_in_flight: AtomicBool,
    input_ledger: Mutex<InputLedger>,
}

impl Default for ServerRecoveryState {
    fn default() -> Self {
        Self {
            pending: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            restart_completed_generation: AtomicU64::new(0),
            restart_request_in_flight: AtomicBool::new(false),
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
    is_ipc_deadline(error) || error.downcast_ref::<IpcRecoveryPending>().is_some()
}

fn preserve_recovery_error(error: anyhow::Error) -> anyhow::Error {
    if is_ipc_deadline(&error) {
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
    pub fn new() -> Result<Self> {
        let runtime = Arc::new(tokio::runtime::Runtime::new()?);
        let connection_id = IPC_CONNECTION_SEQUENCE.fetch_add(1, Ordering::Relaxed);

        let server_channel = Self::connect_named_pipe_channel(
            &runtime,
            "http://[::]:50051",
            r"\\.\pipe\azookey_server",
            SERVER_PIPE_BUSY_TIMEOUT,
        )?;
        let window_client = match Self::connect_named_pipe_channel(
            &runtime,
            "http://[::]:50052",
            r"\\.\pipe\azookey_ui",
            UI_PIPE_BUSY_TIMEOUT,
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
        })
    }

    fn connect_named_pipe_channel(
        runtime: &tokio::runtime::Runtime,
        endpoint: &'static str,
        pipe_name: &'static str,
        busy_timeout: Duration,
    ) -> Result<Channel> {
        let endpoint = Endpoint::try_from(endpoint)?;
        let connect = endpoint.connect_with_connector(service_fn(move |_| async move {
            let busy_started_at = Instant::now();
            let client = loop {
                match ClientOptions::new().open(pipe_name) {
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

            Ok::<_, std::io::Error>(TokioIo::new(client))
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
        let refreshed = Self::new()?;
        self.connection_id = refreshed.connection_id;
        self.azookey_client = refreshed.azookey_client;
        self.window_client = refreshed.window_client;
        self.runtime = refreshed.runtime;
        self.performance_log_tx = refreshed.performance_log_tx;
        Ok(())
    }

    fn mark_server_timeout(recovery: &Arc<ServerRecoveryState>, operation: &'static str) {
        recovery.generation.fetch_add(1, Ordering::AcqRel);
        recovery.pending.store(true, Ordering::Release);
        Self::request_server_restart(recovery, operation);
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

    pub(crate) fn recovery_restart_ready(&self) -> bool {
        let generation = self.recovery.generation.load(Ordering::Acquire);
        restart_generation_ready(
            generation,
            self.recovery
                .restart_completed_generation
                .load(Ordering::Acquire),
        )
    }

    pub(crate) fn wait_for_recovery_restart(&self) -> bool {
        let started = Instant::now();
        while started.elapsed() < INPUT_RPC_DEADLINE {
            self.ensure_server_restart_requested();
            if self.recovery_restart_ready() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        self.recovery_restart_ready()
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
        if offset == 0 || offset.abs() >= 125 {
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
            Self::mark_server_timeout(recovery, operation);
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
            previous.has_same_composition(refreshed_candidates)
                && !refreshed_candidates.is_empty_composition()
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

                        // The edit may have completed before the connection failed, but
                        // refresh alone cannot safely reproduce that mutation in the local
                        // recovery ledger. Discard it so a later deadline recovery cannot
                        // rebuild the pre-edit composition and resurrect removed text or an
                        // old cursor position.
                        self.invalidate_input_ledger();
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
        if is_ipc_deadline(error) {
            // A local timeout means the server may still apply the operation.
            // Replaying before absolute-state reconstruction is unsafe.
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
        if response.is_err() && !response.as_ref().is_err_and(is_ipc_deadline) {
            self.invalidate_input_ledger();
        }
        let response = response?;
        let response = response.into_inner();
        self.observe_server_session("append_text", response.server_session_id);
        self.record_successful_append(text, input_style);
        Ok(response)
    }

    fn send_remove_text(&mut self, request_id: u64) -> anyhow::Result<Candidates> {
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
        self.record_successful_remove();
        Self::candidates_from_composing_text(response.composing_text)
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

        let recovery_generation = self.recovery.generation.load(Ordering::Acquire);
        let input_ledger = self
            .recovery
            .input_ledger
            .lock()
            .ok()
            .filter(|ledger| ledger.complete)
            .map(|ledger| ledger.clone())
            .unwrap_or_else(|| fallback_input_ledger(raw_input, raw_hiragana));

        self.reconnect().map_err(preserve_recovery_error)?;
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

    #[inline]
    fn should_retry_append_after_refresh(
        previous_candidates: Option<&Candidates>,
        refreshed_candidates: &Candidates,
    ) -> bool {
        previous_candidates.is_some_and(|previous| {
            previous.has_same_composition(refreshed_candidates)
                || refreshed_candidates.is_empty_composition()
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
    fn ensure_window_client(
        &mut self,
        operation: &str,
    ) -> Option<&mut WindowServiceClient<Channel>> {
        if self.window_client.is_none() {
            match Self::connect_named_pipe_channel(
                self.runtime.as_ref(),
                "http://[::]:50052",
                r"\\.\pipe\azookey_ui",
                UI_PIPE_BUSY_TIMEOUT,
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
        let result: anyhow::Result<()> = (|| {
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
        Self::ignore_window_rpc_error("ui_show_window", result)
    }

    #[tracing::instrument]
    pub fn hide_window(&mut self) -> anyhow::Result<()> {
        let request_id = current_or_next_request_id();
        let performance_start = client_performance_start();
        let result: anyhow::Result<()> = (|| {
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
        let result: anyhow::Result<()> = (|| {
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
        Self::ignore_window_rpc_error("ui_set_window_position", result)
    }

    #[tracing::instrument]
    pub fn set_candidates(&mut self, candidates: Vec<String>) -> anyhow::Result<()> {
        let request_id = current_or_next_request_id();
        let performance_start = client_performance_start();
        let candidate_count = performance_start.map(|_| candidates.len());
        let result: anyhow::Result<()> = (|| {
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
        Self::ignore_window_rpc_error("ui_set_candidates", result)
    }

    #[tracing::instrument]
    pub fn set_selection(&mut self, index: i32) -> anyhow::Result<()> {
        let request_id = current_or_next_request_id();
        let performance_start = client_performance_start();
        let result: anyhow::Result<()> = (|| {
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
        Self::ignore_window_rpc_error("ui_set_selection", result)
    }

    #[tracing::instrument]
    pub fn set_input_mode(&mut self, mode: &str) -> anyhow::Result<()> {
        let request_id = current_or_next_request_id();
        let performance_start = client_performance_start();
        let result: anyhow::Result<()> = (|| {
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
        let result: anyhow::Result<WindowRpcDelivery> = (|| {
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
        is_non_destructive_ipc_error, mark_input_ledger_incomplete, move_input_cursor,
        pop_input_segment_character, preserve_recovery_error, recovery_generation_is_current,
        restart_generation_ready, restart_request_needed, Candidates, CompositionOperation,
        IPCService, InputLedger, IpcDeadlineExceeded, NonIdempotentEditAttempt, INPUT_STYLE_DIRECT,
        INPUT_STYLE_ROMAN2KANA,
    };
    use std::{
        future::Future,
        pin::Pin,
        sync::{
            atomic::{AtomicBool, Ordering},
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
    fn grpc_deadline_status_never_replays_non_idempotent_rpc() {
        let error = anyhow::Error::new(tonic::Status::deadline_exceeded("server timed out"));

        assert!(is_non_destructive_ipc_error(&error));
        assert!(!IPCService::should_reconnect_rpc_error(&error));
    }

    #[test]
    fn append_retry_is_enabled_when_server_state_is_unchanged() {
        let previous = Candidates {
            texts: vec!["か".to_string()],
            sub_texts: vec![String::new()],
            hiragana: "か".to_string(),
            corresponding_count: vec![1],
            candidate_ids: vec![1],
        };

        assert!(IPCService::should_retry_append_after_refresh(
            Some(&previous),
            &previous
        ));
    }

    #[test]
    fn append_retry_ignores_refreshed_candidate_ids() {
        let previous = Candidates {
            texts: vec!["か".to_string()],
            sub_texts: vec![String::new()],
            hiragana: "か".to_string(),
            corresponding_count: vec![1],
            candidate_ids: vec![1],
        };
        let refreshed = Candidates {
            candidate_ids: vec![2],
            ..previous.clone()
        };

        assert!(IPCService::should_retry_append_after_refresh(
            Some(&previous),
            &refreshed
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
            candidate_ids: vec![1],
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
            candidate_ids: vec![1],
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
            candidate_ids: vec![1],
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
            candidate_ids: vec![1],
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
            candidate_ids: vec![1],
        };

        assert!(IPCService::should_retry_non_idempotent_edit_after_refresh(
            Some(&previous),
            &previous
        ));
    }

    #[test]
    fn non_idempotent_edit_retry_ignores_refreshed_candidate_ids() {
        let previous = Candidates {
            texts: vec!["か".to_string()],
            sub_texts: vec![String::new()],
            hiragana: "か".to_string(),
            corresponding_count: vec![1],
            candidate_ids: vec![1],
        };
        let refreshed = Candidates {
            candidate_ids: vec![2],
            ..previous.clone()
        };

        assert!(IPCService::should_retry_non_idempotent_edit_after_refresh(
            Some(&previous),
            &refreshed
        ));
    }

    #[test]
    fn non_idempotent_edit_retry_is_disabled_when_refreshed_state_changed() {
        let previous = Candidates {
            texts: vec!["か".to_string()],
            sub_texts: vec![String::new()],
            hiragana: "か".to_string(),
            corresponding_count: vec![1],
            candidate_ids: vec![1],
        };
        let refreshed = Candidates {
            texts: vec!["".to_string()],
            sub_texts: vec![String::new()],
            hiragana: String::new(),
            corresponding_count: vec![0],
            candidate_ids: vec![1],
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
            candidate_ids: vec![1],
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
