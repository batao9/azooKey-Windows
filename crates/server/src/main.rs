use azookey_server::TonicNamedPipeServer;
use tonic::{transport::Server, Request, Response, Status};
use tonic_reflection::server::Builder as ReflectionBuilder;
use windows::Win32::System::Threading::{GetCurrentProcess, SetPriorityClass, HIGH_PRIORITY_CLASS};

use shared::proto::azookey_service_server::{AzookeyService, AzookeyServiceServer};
use shared::proto::{
    AppendTextRequest, AppendTextResponse, ClearTextRequest, ClearTextResponse, ComposingText,
    MoveCursorRequest, MoveCursorResponse, PerformanceLogRequest, PerformanceLogResponse,
    RemoveTextRequest, RemoveTextResponse, ShrinkTextRequest, ShrinkTextResponse, Suggestion,
};
use shared::AppConfig;

use std::{
    backtrace::Backtrace,
    collections::HashSet,
    ffi::{c_char, c_int, CStr, CString},
    fs::{self, File, OpenOptions},
    io::{BufWriter, Write},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering},
        mpsc, OnceLock,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const USE_ZENZAI: bool = true;
const INPUT_STYLE_DIRECT: i32 = 1;
const SERVER_LOG_FILE_NAME: &str = "server.log";
const SERVER_PERFORMANCE_LOG_FILE_NAME: &str = "server-performance.tsv";
const SERVER_CRASH_TRACE_FILE_NAME: &str = "server-crash-trace.json";
const SERVER_PREVIOUS_CRASH_TRACE_FILE_NAME: &str = "server-crash-trace.previous.json";
const LAUNCHER_CRASH_TRACE_FILE_NAME: &str = "launcher-crash-trace.json";
const LAUNCHER_PREVIOUS_CRASH_TRACE_FILE_NAME: &str = "launcher-crash-trace.previous.json";
const SERVER_PERFORMANCE_LOG_HEADER: &str =
    "timestamp_ms\trequest_id\tcomponent\toperation\tstage\telapsed_ms\tdetails";
const LOG_MAX_BYTES: u64 = 4 * 1024 * 1024;
const LOG_FLUSH_INTERVAL: Duration = Duration::from_millis(500);
const LOG_FLUSH_ACK_TIMEOUT: Duration = Duration::from_secs(2);
const WARMUP_INTERVAL_SECS: u64 = 30;
const WARMUP_RECENT_INPUT_SKIP_MS: u64 = 2_000;

static SERVER_LOG_WORKER: OnceLock<ServerLogWorker> = OnceLock::new();
static SERVER_LOG_ENABLED: AtomicBool = AtomicBool::new(false);
static SERVER_PERFORMANCE_LOG_ENABLED: AtomicBool = AtomicBool::new(false);
static SERVER_LOG_LEVEL: AtomicU8 = AtomicU8::new(ServerLogLevel::Warn as u8);
static SERVER_CRASH_TRACE_ENABLED: AtomicBool = AtomicBool::new(true);
static REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);
const SERVER_GENERATED_REQUEST_ID_PREFIX: u64 = 1 << 63;
static SERVER_SESSION_ID: OnceLock<u64> = OnceLock::new();
static HAS_ACTIVE_COMPOSITION: AtomicBool = AtomicBool::new(false);
static SERVER_REQUESTS_IN_FLIGHT: AtomicU64 = AtomicU64::new(0);
static LAST_INPUT_REQUEST_FINISHED_MS: AtomicU64 = AtomicU64::new(0);
static MONOTONIC_START: OnceLock<Instant> = OnceLock::new();

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum ServerLogLevel {
    Off = 0,
    Error = 1,
    Warn = 2,
    Info = 3,
    Debug = 4,
}

impl ServerLogLevel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::Error => "ERROR",
            Self::Warn => "WARN",
            Self::Info => "INFO",
            Self::Debug => "DEBUG",
        }
    }

    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Error,
            2 => Self::Warn,
            3 => Self::Info,
            4 => Self::Debug,
            _ => Self::Off,
        }
    }
}

#[derive(Clone, Debug)]
struct LogPaths {
    server_log: PathBuf,
    performance_log: PathBuf,
}

#[derive(Default)]
struct ServerLogSinks {
    normal: Option<RotatingLogSink>,
    performance: Option<RotatingLogSink>,
}

impl ServerLogSinks {
    fn flush_all(&mut self) {
        if let Some(sink) = self.normal.as_mut() {
            sink.flush();
        }
        if let Some(sink) = self.performance.as_mut() {
            sink.flush();
        }
    }
}

struct ServerLogWorker {
    sender: mpsc::Sender<ServerLogCommand>,
}

#[derive(Default)]
struct ServerLogConfigureResult {
    normal_enabled: bool,
    performance_enabled: bool,
}

enum ServerLogCommand {
    Configure {
        paths: Option<LogPaths>,
        ack: mpsc::Sender<ServerLogConfigureResult>,
    },
    WriteLog(String),
    WritePerformance(String),
    Flush(mpsc::Sender<()>),
}

struct RotatingLogSink {
    path: PathBuf,
    writer: BufWriter<File>,
    bytes_written: u64,
    header: Option<&'static str>,
    last_flush: Instant,
}

impl RotatingLogSink {
    fn open(path: PathBuf, header: Option<&'static str>) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        rotate_existing_log_if_needed(&path)?;

        let bytes_written = path.metadata().map(|metadata| metadata.len()).unwrap_or(0);
        let needs_header = header.is_some() && bytes_written == 0;
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let mut sink = Self {
            path,
            writer: BufWriter::new(file),
            bytes_written,
            header,
            last_flush: Instant::now(),
        };

        if needs_header {
            if let Some(header) = sink.header {
                sink.write_raw_line(header)?;
                sink.flush();
            }
        }

        Ok(sink)
    }

    fn write_line(&mut self, line: &str) -> std::io::Result<()> {
        let line_bytes = line.len() as u64 + 1;
        if self.bytes_written.saturating_add(line_bytes) > LOG_MAX_BYTES {
            self.rotate()?;
        }

        self.write_raw_line(line)?;
        if self.last_flush.elapsed() >= LOG_FLUSH_INTERVAL {
            self.flush();
        }

        Ok(())
    }

    fn write_raw_line(&mut self, line: &str) -> std::io::Result<()> {
        self.writer.write_all(line.as_bytes())?;
        self.writer.write_all(b"\n")?;
        self.bytes_written = self.bytes_written.saturating_add(line.len() as u64 + 1);
        Ok(())
    }

    fn rotate(&mut self) -> std::io::Result<()> {
        self.flush();
        let rotated_path = rotated_log_path(&self.path);
        match fs::remove_file(&rotated_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        match fs::rename(&self.path, rotated_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        self.writer = BufWriter::new(file);
        self.bytes_written = 0;
        self.last_flush = Instant::now();

        if let Some(header) = self.header {
            self.write_raw_line(header)?;
        }

        Ok(())
    }

    fn flush(&mut self) {
        let _ = self.writer.flush();
        self.last_flush = Instant::now();
    }
}

fn rotated_log_path(path: &std::path::Path) -> PathBuf {
    let Some(file_name) = path.file_name() else {
        return path.with_extension("1");
    };
    let mut rotated_file_name = file_name.to_os_string();
    rotated_file_name.push(".1");
    path.with_file_name(rotated_file_name)
}

fn rotate_existing_log_if_needed(path: &std::path::Path) -> std::io::Result<()> {
    let Ok(metadata) = path.metadata() else {
        return Ok(());
    };

    if metadata.len() <= LOG_MAX_BYTES {
        return Ok(());
    }

    let rotated_path = rotated_log_path(path);
    match fs::remove_file(&rotated_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    fs::rename(path, rotated_path)
}

fn server_log_worker() -> &'static ServerLogWorker {
    SERVER_LOG_WORKER.get_or_init(|| {
        let (sender, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("azookey-server-log-writer".to_owned())
            .spawn(move || server_log_worker_loop(receiver))
            .expect("failed to spawn azookey server log writer");
        ServerLogWorker { sender }
    })
}

fn server_log_worker_loop(receiver: mpsc::Receiver<ServerLogCommand>) {
    let mut sinks = ServerLogSinks::default();

    loop {
        match receiver.recv_timeout(LOG_FLUSH_INTERVAL) {
            Ok(command) => handle_server_log_command(&mut sinks, command),
            Err(mpsc::RecvTimeoutError::Timeout) => sinks.flush_all(),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                sinks.flush_all();
                break;
            }
        }
    }
}

fn handle_server_log_command(sinks: &mut ServerLogSinks, command: ServerLogCommand) {
    match command {
        ServerLogCommand::Configure { paths, ack } => {
            let result = configure_server_log_sinks(sinks, paths);
            let _ = ack.send(result);
        }
        ServerLogCommand::WriteLog(line) => {
            if let Some(sink) = sinks.normal.as_mut() {
                eprintln!("{line}");
                let _ = sink.write_line(&line);
            }
        }
        ServerLogCommand::WritePerformance(line) => {
            if let Some(sink) = sinks.performance.as_mut() {
                let _ = sink.write_line(&line);
            }
        }
        ServerLogCommand::Flush(ack) => {
            sinks.flush_all();
            let _ = ack.send(());
        }
    }
}

fn configure_server_log_sinks(
    sinks: &mut ServerLogSinks,
    paths: Option<LogPaths>,
) -> ServerLogConfigureResult {
    sinks.flush_all();
    *sinks = ServerLogSinks::default();

    let Some(paths) = paths else {
        return ServerLogConfigureResult::default();
    };

    sinks.normal = match RotatingLogSink::open(paths.server_log, None) {
        Ok(sink) => Some(sink),
        Err(error) => {
            eprintln!("Failed to open server log file: {error}");
            None
        }
    };
    sinks.performance =
        match RotatingLogSink::open(paths.performance_log, Some(SERVER_PERFORMANCE_LOG_HEADER)) {
            Ok(sink) => Some(sink),
            Err(error) => {
                eprintln!("Failed to open server performance log file: {error}");
                None
            }
        };

    ServerLogConfigureResult {
        normal_enabled: sinks.normal.is_some(),
        performance_enabled: sinks.performance.is_some(),
    }
}

fn send_server_log_command(command: ServerLogCommand) {
    if let Err(error) = server_log_worker().sender.send(command) {
        eprintln!("Failed to send server log command: {error}");
    }
}

fn should_log(level: ServerLogLevel) -> bool {
    if level == ServerLogLevel::Off {
        return false;
    }

    SERVER_LOG_ENABLED.load(Ordering::Relaxed)
        && level <= ServerLogLevel::from_u8(SERVER_LOG_LEVEL.load(Ordering::Relaxed))
}

fn should_log_performance() -> bool {
    SERVER_PERFORMANCE_LOG_ENABLED.load(Ordering::Relaxed)
        && ServerLogLevel::from_u8(SERVER_LOG_LEVEL.load(Ordering::Relaxed))
            >= ServerLogLevel::Debug
}

macro_rules! log_event_lazy {
    ($level:expr, $($arg:tt)*) => {{
        let level = $level;
        if should_log(level) {
            log_event(level, &format!($($arg)*));
        }
    }};
}

macro_rules! performance_event_lazy {
    ($request_id:expr, $operation:expr, $stage:expr, $elapsed_ms:expr, $($arg:tt)*) => {{
        if should_log_performance() {
            log_performance_event(
                $request_id,
                "rust",
                $operation,
                $stage,
                $elapsed_ms,
                &format!($($arg)*),
            );
        }
    }};
}

struct RawComposingText {
    text: String,
    cursor: i8,
}

struct ComposedText {
    hiragana: Option<String>,
    suggestions: Vec<Suggestion>,
}

#[derive(Debug, Clone)]
#[repr(C)]
struct FFICandidate {
    text: *mut c_char,
    subtext: *mut c_char,
    hiragana: *mut c_char,
    corresponding_count: c_int,
}

unsafe extern "C" {
    fn Initialize(path: *const c_char, use_zenzai: bool);
    fn SetContext(context: *const c_char);
    fn AppendText(input: *const c_char, cursorPtr: *mut c_int) -> *mut c_char;
    fn AppendTextDirect(input: *const c_char, cursorPtr: *mut c_int) -> *mut c_char;
    fn RemoveText(cursorPtr: *mut c_int) -> *mut c_char;
    fn MoveCursor(offset: c_int, cursorPtr: *mut c_int) -> *mut c_char;
    fn ShrinkText(offset: c_int) -> *mut c_char;
    fn ClearText();
    fn Warmup() -> bool;
    fn HasActiveComposition() -> bool;
    fn GetComposedText(lengthPtr: *mut c_int) -> *mut *mut FFICandidate;
    fn GetComposedTextForCursorPrefix(lengthPtr: *mut c_int) -> *mut *mut FFICandidate;
    fn FreeCString(ptr: *mut c_char);
    fn FreeCandidateList(ptr: *mut *mut FFICandidate, length: c_int);
    fn LoadConfig();
    fn SetRequestId(request_id: u64);
    fn SetServerLogCallbacks(
        log_enabled: extern "C" fn() -> bool,
        log_level_enabled: extern "C" fn(*const c_char) -> bool,
        performance_log_enabled: extern "C" fn() -> bool,
        write_log: extern "C" fn(*const c_char, *const c_char),
        write_performance_log: extern "C" fn(u64, *const c_char, *const c_char, u64, *const c_char),
        flush_log: extern "C" fn(),
        crash_trace_enabled: extern "C" fn() -> bool,
        write_crash_trace: extern "C" fn(
            *const c_char,
            *const c_char,
            *const c_char,
            *const c_char,
        ),
    );
}

struct OwnedFfiString {
    ptr: *mut c_char,
}

impl OwnedFfiString {
    unsafe fn from_raw(scope: &str, ptr: *mut c_char) -> Result<Self, String> {
        if ptr.is_null() {
            return Err(format!("[{scope}] Swift FFI returned null pointer"));
        }

        Ok(Self { ptr })
    }

    fn to_string_lossy(&self) -> String {
        unsafe { CStr::from_ptr(self.ptr as *const c_char) }
            .to_string_lossy()
            .into_owned()
    }
}

impl Drop for OwnedFfiString {
    fn drop(&mut self) {
        unsafe {
            FreeCString(self.ptr);
        }
    }
}

struct OwnedFfiCandidates {
    ptr: *mut *mut FFICandidate,
    length: c_int,
}

impl OwnedFfiCandidates {
    unsafe fn from_raw(
        scope: &str,
        ptr: *mut *mut FFICandidate,
        length: c_int,
    ) -> Result<Self, String> {
        if length < 0 {
            if !ptr.is_null() {
                FreeCandidateList(ptr, 0);
            }
            return Err(format!("[{scope}] invalid negative length: {length}"));
        }

        if length > 0 && ptr.is_null() {
            return Err(format!(
                "[{scope}] null candidate list pointer (length={length})"
            ));
        }

        Ok(Self { ptr, length })
    }

    fn len(&self) -> usize {
        self.length as usize
    }

    unsafe fn candidate_ptr(&self, index: usize) -> *mut FFICandidate {
        *self.ptr.add(index)
    }

    fn free_with_performance(mut self, request_id: u64, operation: &str) {
        let ptr = self.ptr;
        let length = self.length;
        self.ptr = std::ptr::null_mut();
        self.length = 0;

        let free_start = Instant::now();
        unsafe {
            FreeCandidateList(ptr, length);
        }
        performance_event_lazy!(
            request_id,
            operation,
            "free_candidate_list",
            elapsed_ms(free_start),
            "candidate_count={}",
            length.max(0)
        );
    }
}

impl Drop for OwnedFfiCandidates {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe {
                FreeCandidateList(self.ptr, self.length);
            }
        }
    }
}

fn next_request_id() -> u64 {
    SERVER_GENERATED_REQUEST_ID_PREFIX | REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
}

fn request_id_or_next(request_id: u64) -> u64 {
    if request_id == 0 {
        next_request_id()
    } else {
        request_id
    }
}

fn server_session_id() -> u64 {
    *SERVER_SESSION_ID.get_or_init(|| {
        let session_id = (u64::from(std::process::id()) << 32) ^ (now_timestamp_millis() as u64);
        session_id.max(1)
    })
}

fn set_request_id(request_id: u64) {
    unsafe {
        SetRequestId(request_id);
    }
}

fn register_server_log_callbacks() {
    unsafe {
        SetServerLogCallbacks(
            AzookeyServerLogEnabled,
            AzookeyServerLogLevelEnabled,
            AzookeyServerPerformanceLogEnabled,
            AzookeyServerLogFromSwift,
            AzookeyServerPerformanceLogFromSwift,
            AzookeyServerLogFlushFromSwift,
            AzookeyServerCrashTraceEnabled,
            AzookeyServerCrashTraceFromSwift,
        );
    }
}

fn elapsed_ms(start: Instant) -> u128 {
    start.elapsed().as_millis()
}

fn performance_instant(enabled: bool) -> Option<Instant> {
    enabled.then(Instant::now)
}

fn add_elapsed_ms(total: &mut u128, start: Option<Instant>) {
    if let Some(start) = start {
        *total += elapsed_ms(start);
    }
}

fn monotonic_millis() -> u64 {
    let elapsed = MONOTONIC_START
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis();
    u64::try_from(elapsed).unwrap_or(u64::MAX)
}

fn record_input_request_finished() {
    LAST_INPUT_REQUEST_FINISHED_MS.store(monotonic_millis().max(1), Ordering::Release);
}

struct ServerRequestGuard {
    is_input_request: bool,
}

impl ServerRequestGuard {
    fn begin(is_input_request: bool) -> Self {
        SERVER_REQUESTS_IN_FLIGHT.fetch_add(1, Ordering::AcqRel);
        Self { is_input_request }
    }
}

impl Drop for ServerRequestGuard {
    fn drop(&mut self) {
        if self.is_input_request {
            record_input_request_finished();
        }
        SERVER_REQUESTS_IN_FLIGHT.fetch_sub(1, Ordering::AcqRel);
    }
}

enum WarmupSkipReason {
    ActiveComposition,
    RequestInFlight { in_flight: u64 },
    RecentInput { elapsed_ms: u64 },
}

impl WarmupSkipReason {
    fn details(&self) -> String {
        match self {
            Self::ActiveComposition => "reason=active_composition".to_owned(),
            Self::RequestInFlight { in_flight } => {
                format!("reason=request_in_flight;in_flight={in_flight}")
            }
            Self::RecentInput { elapsed_ms } => format!(
                "reason=recent_input;elapsed_ms={elapsed_ms};skip_ms={WARMUP_RECENT_INPUT_SKIP_MS}"
            ),
        }
    }
}

fn warmup_skip_reason() -> Option<WarmupSkipReason> {
    if has_active_composition() {
        return Some(WarmupSkipReason::ActiveComposition);
    }

    let in_flight = SERVER_REQUESTS_IN_FLIGHT.load(Ordering::Acquire);
    if in_flight > 0 {
        return Some(WarmupSkipReason::RequestInFlight { in_flight });
    }

    let last_input_finished_ms = LAST_INPUT_REQUEST_FINISHED_MS.load(Ordering::Acquire);
    if last_input_finished_ms > 0 {
        let elapsed_ms = monotonic_millis().saturating_sub(last_input_finished_ms);
        if elapsed_ms < WARMUP_RECENT_INPUT_SKIP_MS {
            return Some(WarmupSkipReason::RecentInput { elapsed_ms });
        }
    }

    None
}

fn now_timestamp_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

fn resolve_log_path(file_name: &str) -> PathBuf {
    if let Ok(appdata) = std::env::var("APPDATA") {
        PathBuf::from(appdata)
            .join("Azookey")
            .join("logs")
            .join(file_name)
    } else {
        std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("logs")
            .join(file_name)
    }
}

fn configure_server_logging(config: &AppConfig) -> Option<LogPaths> {
    let level = server_log_level_from_str(&config.debug.server_log_level);
    SERVER_LOG_LEVEL.store(level as u8, Ordering::Relaxed);
    SERVER_CRASH_TRACE_ENABLED.store(config.debug.server_crash_trace_enabled, Ordering::Relaxed);

    let paths = config.debug.server_log_enabled.then(|| LogPaths {
        server_log: resolve_log_path(SERVER_LOG_FILE_NAME),
        performance_log: resolve_log_path(SERVER_PERFORMANCE_LOG_FILE_NAME),
    });
    let (ack_tx, ack_rx) = mpsc::channel();
    send_server_log_command(ServerLogCommand::Configure {
        paths: paths.clone(),
        ack: ack_tx,
    });

    match ack_rx.recv_timeout(LOG_FLUSH_ACK_TIMEOUT) {
        Ok(result) => {
            SERVER_LOG_ENABLED.store(result.normal_enabled, Ordering::Relaxed);
            SERVER_PERFORMANCE_LOG_ENABLED.store(result.performance_enabled, Ordering::Relaxed);
        }
        Err(error) => {
            SERVER_LOG_ENABLED.store(false, Ordering::Relaxed);
            SERVER_PERFORMANCE_LOG_ENABLED.store(false, Ordering::Relaxed);
            eprintln!("Timed out configuring server logging: {error}");
        }
    }

    paths
}

fn reload_server_logging_from_settings() -> Option<LogPaths> {
    let config = AppConfig::read().unwrap_or_default();
    configure_server_logging(&config)
}

fn flush_server_logs() {
    let (ack_tx, ack_rx) = mpsc::channel();
    send_server_log_command(ServerLogCommand::Flush(ack_tx));
    if let Err(error) = ack_rx.recv_timeout(LOG_FLUSH_ACK_TIMEOUT) {
        eprintln!("Timed out flushing server logs: {error}");
    }
}

fn log_event(level: ServerLogLevel, message: &str) {
    log_event_with_component(None, level, message);
}

fn log_event_with_component(component: Option<&str>, level: ServerLogLevel, message: &str) {
    if !should_log(level) {
        return;
    }

    let level_label = if let Some(component) = component {
        format!("{component}/{}", level.as_str())
    } else {
        level.as_str().to_owned()
    };
    let line = format!("[{}] [{}] {}", now_timestamp_millis(), level_label, message);

    send_server_log_command(ServerLogCommand::WriteLog(line));
}

fn crash_trace_enabled() -> bool {
    SERVER_CRASH_TRACE_ENABLED.load(Ordering::Relaxed)
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

fn compact_trace_for_log(trace: &str) -> String {
    let compact = trace.split_whitespace().collect::<Vec<_>>().join(" ");
    const MAX_TRACE_LOG_CHARS: usize = 512;
    if compact.chars().count() <= MAX_TRACE_LOG_CHARS {
        return compact;
    }
    compact.chars().take(MAX_TRACE_LOG_CHARS).collect()
}

fn crash_trace_is_in_progress(trace: &str) -> bool {
    trace.contains("\"state\":\"begin\"") || trace.contains("\"state\": \"begin\"")
}

fn preserve_crash_trace_if_incomplete(
    source_file_name: &str,
    previous_file_name: &str,
) -> Option<String> {
    if !crash_trace_enabled() {
        return None;
    }

    let source_path = resolve_log_path(source_file_name);
    let Ok(trace) = fs::read_to_string(&source_path) else {
        return None;
    };
    if !crash_trace_is_in_progress(&trace) {
        return None;
    }

    let previous_path = resolve_log_path(previous_file_name);
    if let Some(parent) = previous_path.parent() {
        if let Err(error) = fs::create_dir_all(parent) {
            eprintln!("Failed to create previous crash trace directory: {error}");
            return Some(trace);
        }
    }
    if let Err(error) = fs::write(&previous_path, trace.as_bytes()) {
        eprintln!("Failed to preserve previous crash trace: {error}");
    }
    Some(trace)
}

fn log_previous_crash_trace(label: &str, trace: &str) {
    log_event(
        ServerLogLevel::Warn,
        &format!(
            "{label} was still in progress: {}",
            compact_trace_for_log(trace)
        ),
    );
}

fn log_and_consume_previous_crash_trace_if_incomplete(file_name: &str, label: &str) {
    if !crash_trace_enabled() {
        return;
    }

    let path = resolve_log_path(file_name);
    let Ok(trace) = fs::read_to_string(&path) else {
        return;
    };
    if crash_trace_is_in_progress(&trace) {
        let should_consume = should_log(ServerLogLevel::Warn);
        log_previous_crash_trace(label, &trace);
        if should_consume {
            flush_server_logs();
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    log_event(
                        ServerLogLevel::Warn,
                        &format!("failed to clear consumed {label}: {error}"),
                    );
                    flush_server_logs();
                }
            }
        }
    }
}

fn write_crash_trace_file(
    file_name: &str,
    component: &str,
    operation: &str,
    stage: &str,
    state: &str,
    details: &str,
) {
    if !crash_trace_enabled() {
        return;
    }

    let path = resolve_log_path(file_name);
    if let Some(parent) = path.parent() {
        if let Err(error) = fs::create_dir_all(parent) {
            eprintln!("Failed to create crash trace directory: {error}");
            return;
        }
    }

    let trace = format!(
        concat!(
            "{{\n",
            "  \"timestamp_ms\": {},\n",
            "  \"process_id\": {},\n",
            "  \"component\": \"{}\",\n",
            "  \"operation\": \"{}\",\n",
            "  \"stage\": \"{}\",\n",
            "  \"state\": \"{}\",\n",
            "  \"details\": \"{}\"\n",
            "}}\n"
        ),
        now_timestamp_millis(),
        std::process::id(),
        json_escape(component),
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
        Err(error) => eprintln!("Failed to write crash trace: {error}"),
    }
}

fn write_server_crash_trace(
    component: &str,
    operation: &str,
    stage: &str,
    state: &str,
    details: &str,
) {
    write_crash_trace_file(
        SERVER_CRASH_TRACE_FILE_NAME,
        component,
        operation,
        stage,
        state,
        details,
    );
}

fn sanitize_performance_field(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '\t' | '\r' | '\n' => ' ',
            _ => ch,
        })
        .collect()
}

fn log_performance_event(
    request_id: u64,
    component: &str,
    operation: &str,
    stage: &str,
    elapsed_ms: u128,
    details: &str,
) {
    if !should_log_performance() {
        return;
    }

    let line = format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}",
        now_timestamp_millis(),
        request_id,
        sanitize_performance_field(component),
        sanitize_performance_field(operation),
        sanitize_performance_field(stage),
        elapsed_ms,
        sanitize_performance_field(details),
    );

    send_server_log_command(ServerLogCommand::WritePerformance(line));
}

fn optional_cstr_lossy(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }

    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}

fn server_log_level_from_str(level: &str) -> ServerLogLevel {
    match level.to_ascii_uppercase().as_str() {
        "OFF" => ServerLogLevel::Off,
        "ERROR" => ServerLogLevel::Error,
        "WARN" | "WARNING" => ServerLogLevel::Warn,
        "INFO" => ServerLogLevel::Info,
        "DEBUG" => ServerLogLevel::Debug,
        _ => ServerLogLevel::Warn,
    }
}

#[no_mangle]
pub extern "C" fn AzookeyServerLogEnabled() -> bool {
    SERVER_LOG_ENABLED.load(Ordering::Relaxed)
}

#[no_mangle]
pub extern "C" fn AzookeyServerLogLevelEnabled(level: *const c_char) -> bool {
    should_log(server_log_level_from_str(&optional_cstr_lossy(level)))
}

#[no_mangle]
pub extern "C" fn AzookeyServerPerformanceLogEnabled() -> bool {
    should_log_performance()
}

#[no_mangle]
pub extern "C" fn AzookeyServerLogFromSwift(level: *const c_char, message: *const c_char) {
    let level = server_log_level_from_str(&optional_cstr_lossy(level));
    if !should_log(level) {
        return;
    }

    let message = optional_cstr_lossy(message);
    log_event_with_component(Some("SWIFT"), level, &message);
}

#[no_mangle]
pub extern "C" fn AzookeyServerPerformanceLogFromSwift(
    request_id: u64,
    operation: *const c_char,
    stage: *const c_char,
    elapsed_ms: u64,
    details: *const c_char,
) {
    if !should_log_performance() {
        return;
    }

    log_performance_event(
        request_id,
        "swift",
        &optional_cstr_lossy(operation),
        &optional_cstr_lossy(stage),
        elapsed_ms as u128,
        &optional_cstr_lossy(details),
    );
}

#[no_mangle]
pub extern "C" fn AzookeyServerLogFlushFromSwift() {
    flush_server_logs();
}

#[no_mangle]
pub extern "C" fn AzookeyServerCrashTraceEnabled() -> bool {
    crash_trace_enabled()
}

#[no_mangle]
pub extern "C" fn AzookeyServerCrashTraceFromSwift(
    operation: *const c_char,
    stage: *const c_char,
    state: *const c_char,
    details: *const c_char,
) {
    write_server_crash_trace(
        "swift",
        &optional_cstr_lossy(operation),
        &optional_cstr_lossy(stage),
        &optional_cstr_lossy(state),
        &optional_cstr_lossy(details),
    );
}

fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let payload = if let Some(message) = panic_info.payload().downcast_ref::<&str>() {
            (*message).to_owned()
        } else if let Some(message) = panic_info.payload().downcast_ref::<String>() {
            message.clone()
        } else {
            "<non-string panic payload>".to_owned()
        };

        let location = panic_info
            .location()
            .map(|location| {
                format!(
                    "{}:{}:{}",
                    location.file(),
                    location.line(),
                    location.column()
                )
            })
            .unwrap_or_else(|| "<unknown>".to_owned());

        let backtrace = Backtrace::force_capture();
        write_server_crash_trace(
            "rust",
            "panic",
            "panic_hook",
            "error",
            &format!("payload={payload};location={location}"),
        );
        log_event(
            ServerLogLevel::Error,
            &format!("payload={payload}; location={location}; backtrace={backtrace}"),
        );
        flush_server_logs();

        default_hook(panic_info);
    }));
}

fn cstring_from_input(scope: &str, value: &str) -> Result<CString, String> {
    CString::new(value).map_err(|error| format!("[{scope}] CString::new failed: {error}"))
}

fn ffi_text_result(scope: &str, result: *mut c_char) -> Result<String, String> {
    let result = unsafe { OwnedFfiString::from_raw(scope, result)? };
    Ok(result.to_string_lossy())
}

fn i8_offset_from_i32(scope: &str, raw: i32) -> Result<i8, Status> {
    i8::try_from(raw).map_err(|_| {
        log_event(
            ServerLogLevel::Warn,
            &format!("[{scope}] offset out of range: {raw}"),
        );
        Status::invalid_argument("offset out of range")
    })
}

fn cursor_from_c_int(scope: &str, cursor: c_int) -> i8 {
    match i8::try_from(cursor) {
        Ok(value) => value,
        Err(_) => {
            let clamped = cursor.clamp(i8::MIN as c_int, i8::MAX as c_int) as i8;
            log_event(
                ServerLogLevel::Warn,
                &format!("[{scope}] cursor out of range: {cursor}, clamped to {clamped}"),
            );
            clamped
        }
    }
}

fn status_from_error(scope: &str, error: String) -> Status {
    log_event(ServerLogLevel::Error, &error);
    Status::internal(format!("{scope} failed"))
}

fn initialize(path: &str) -> Result<(), String> {
    let path = cstring_from_input("Initialize.path", path)?;
    unsafe {
        Initialize(path.as_ptr(), USE_ZENZAI);
    }
    Ok(())
}

fn add_text(input: &str) -> Result<RawComposingText, String> {
    let input = cstring_from_input("AppendText.input", input)?;

    unsafe {
        let mut cursor: c_int = 0;
        let result = AppendText(input.as_ptr(), &mut cursor);
        let text = ffi_text_result("AppendText", result)?;

        Ok(RawComposingText {
            text,
            cursor: cursor_from_c_int("AppendText", cursor),
        })
    }
}

fn add_text_direct(input: &str) -> Result<RawComposingText, String> {
    let input = cstring_from_input("AppendTextDirect.input", input)?;

    unsafe {
        let mut cursor: c_int = 0;
        let result = AppendTextDirect(input.as_ptr(), &mut cursor);
        let text = ffi_text_result("AppendTextDirect", result)?;

        Ok(RawComposingText {
            text,
            cursor: cursor_from_c_int("AppendTextDirect", cursor),
        })
    }
}

fn move_cursor(offset: i8) -> Result<RawComposingText, String> {
    unsafe {
        let offset = c_int::from(offset);
        let mut cursor: c_int = 0;
        let result = MoveCursor(offset, &mut cursor);
        let text = ffi_text_result("MoveCursor", result)?;

        Ok(RawComposingText {
            text,
            cursor: cursor_from_c_int("MoveCursor", cursor),
        })
    }
}

fn remove_text() -> Result<RawComposingText, String> {
    unsafe {
        let mut cursor: c_int = 0;
        let result = RemoveText(&mut cursor);
        let text = ffi_text_result("RemoveText", result)?;

        Ok(RawComposingText {
            text,
            cursor: cursor_from_c_int("RemoveText", cursor),
        })
    }
}

fn clear_text() {
    unsafe {
        ClearText();
    }
}

fn warmup() -> bool {
    unsafe { Warmup() }
}

fn query_active_composition_state() -> bool {
    unsafe { HasActiveComposition() }
}

fn has_active_composition() -> bool {
    HAS_ACTIVE_COMPOSITION.load(Ordering::Relaxed)
}

fn update_active_composition_state(text: &str) {
    HAS_ACTIVE_COMPOSITION.store(!text.is_empty(), Ordering::Relaxed);
}

fn get_composed_text(use_cursor_prefix: bool, request_id: u64) -> Result<ComposedText, String> {
    let mut length: c_int = 0;
    let operation = if use_cursor_prefix {
        "get_composed_text_for_cursor_prefix"
    } else {
        "get_composed_text"
    };
    let ffi_call_start = Instant::now();
    let result = unsafe {
        if use_cursor_prefix {
            GetComposedTextForCursorPrefix(&mut length)
        } else {
            GetComposedText(&mut length)
        }
    };
    let call_name = if use_cursor_prefix {
        "GetComposedTextForCursorPrefix"
    } else {
        "GetComposedText"
    };
    let candidates = unsafe { OwnedFfiCandidates::from_raw(call_name, result, length)? };
    let length = candidates.len();
    performance_event_lazy!(
        request_id,
        operation,
        "ffi_call",
        elapsed_ms(ffi_call_start),
        "candidate_count={length};use_cursor_prefix={use_cursor_prefix}"
    );

    let mut suggestions = Vec::with_capacity(length);
    let mut hiragana = None;
    let mut seen_texts = HashSet::with_capacity(length);
    let performance_enabled = should_log_performance();
    let mut cstr_decode_ms = 0;
    let mut dedup_ms = 0;
    let mut duplicate_count = 0usize;
    log_event_lazy!(
        ServerLogLevel::Debug,
        "[{call_name}] candidate_count={length}"
    );

    for index in 0..length {
        let candidate_ptr = unsafe { candidates.candidate_ptr(index) };
        if candidate_ptr.is_null() {
            log_event(
                ServerLogLevel::Warn,
                &format!("[{call_name}] candidate[{index}] is null and skipped"),
            );
            continue;
        }

        let candidate = unsafe { (*candidate_ptr).clone() };
        if candidate.text.is_null() || candidate.subtext.is_null() {
            log_event(
                ServerLogLevel::Warn,
                &format!(
                    "[{call_name}] candidate[{index}] has null text/subtext pointer and was skipped"
                ),
            );
            continue;
        }

        if hiragana.is_none() && !candidate.hiragana.is_null() {
            let decode_start = performance_instant(performance_enabled);
            hiragana = Some(
                unsafe { CStr::from_ptr(candidate.hiragana) }
                    .to_string_lossy()
                    .into_owned(),
            );
            add_elapsed_ms(&mut cstr_decode_ms, decode_start);
        }

        let text_decode_start = performance_instant(performance_enabled);
        let text = unsafe { CStr::from_ptr(candidate.text) }
            .to_string_lossy()
            .into_owned();
        add_elapsed_ms(&mut cstr_decode_ms, text_decode_start);

        let dedup_start = performance_instant(performance_enabled);
        if !seen_texts.insert(text.clone()) {
            duplicate_count += 1;
            add_elapsed_ms(&mut dedup_ms, dedup_start);
            continue;
        }
        add_elapsed_ms(&mut dedup_ms, dedup_start);

        let subtext_decode_start = performance_instant(performance_enabled);
        let subtext = unsafe { CStr::from_ptr(candidate.subtext) }
            .to_string_lossy()
            .into_owned();
        add_elapsed_ms(&mut cstr_decode_ms, subtext_decode_start);
        let corresponding_count = candidate.corresponding_count;

        let suggestion = Suggestion {
            text,
            subtext,
            corresponding_count,
        };

        suggestions.push(suggestion);
    }
    performance_event_lazy!(
        request_id,
        operation,
        "cstr_decode",
        cstr_decode_ms,
        "candidate_count={length};unique_candidate_count={};duplicate_count={duplicate_count}",
        suggestions.len()
    );
    performance_event_lazy!(
        request_id,
        operation,
        "dedup",
        dedup_ms,
        "candidate_count={length};unique_candidate_count={};duplicate_count={duplicate_count}",
        suggestions.len()
    );
    candidates.free_with_performance(request_id, operation);

    Ok(ComposedText {
        hiragana,
        suggestions,
    })
}

fn shrink_text(offset: i8) -> Result<RawComposingText, String> {
    unsafe {
        let offset = c_int::from(offset);
        let result = ShrinkText(offset);
        let text = ffi_text_result("ShrinkText", result)?;

        Ok(RawComposingText { text, cursor: 0 })
    }
}

#[derive(Debug, Default)]
pub struct MyAzookeyService;

#[tonic::async_trait]
impl AzookeyService for MyAzookeyService {
    async fn append_text(
        &self,
        request: Request<AppendTextRequest>,
    ) -> Result<Response<AppendTextResponse>, Status> {
        let request = request.into_inner();
        let _request_guard = ServerRequestGuard::begin(true);
        let request_id = request_id_or_next(request.request_id);
        set_request_id(request_id);
        let handler_start = Instant::now();
        let input_style = request.input_style;
        let input = request.text_to_append;
        let input_len = input.chars().count();
        let append_start = Instant::now();
        let composing_text = if input_style == INPUT_STYLE_DIRECT {
            add_text_direct(&input).map_err(|error| status_from_error("append_text", error))?
        } else {
            add_text(&input).map_err(|error| status_from_error("append_text", error))?
        };
        performance_event_lazy!(
            request_id,
            "append_text",
            "swift_append_text",
            elapsed_ms(append_start),
            "input_len={input_len};input_style={input_style}"
        );
        let get_composed_start = Instant::now();
        let composed_text = get_composed_text(false, request_id)
            .map_err(|error| status_from_error("append_text", error))?;
        performance_event_lazy!(
            request_id,
            "append_text",
            "swift_get_composed_text",
            elapsed_ms(get_composed_start),
            "suggestions={};hiragana_len={}",
            composed_text.suggestions.len(),
            composed_text
                .hiragana
                .as_ref()
                .unwrap_or(&composing_text.text)
                .chars()
                .count()
        );
        update_active_composition_state(&composing_text.text);
        performance_event_lazy!(
            request_id,
            "append_text",
            "total",
            elapsed_ms(handler_start),
            "status=success;cursor={};hiragana_len={};suggestions={}",
            composing_text.cursor,
            composing_text.text.chars().count(),
            composed_text.suggestions.len()
        );

        Ok(Response::new(AppendTextResponse {
            composing_text: Some(ComposingText {
                hiragana: composed_text.hiragana.unwrap_or(composing_text.text),
                suggestions: composed_text.suggestions,
            }),
            server_session_id: server_session_id(),
        }))
    }

    async fn remove_text(
        &self,
        request: Request<RemoveTextRequest>,
    ) -> Result<Response<RemoveTextResponse>, Status> {
        let request = request.into_inner();
        let _request_guard = ServerRequestGuard::begin(true);
        let request_id = request_id_or_next(request.request_id);
        set_request_id(request_id);
        let handler_start = Instant::now();

        let remove_start = Instant::now();
        let composing_text =
            remove_text().map_err(|error| status_from_error("remove_text", error))?;
        performance_event_lazy!(
            request_id,
            "remove_text",
            "swift_remove_text",
            elapsed_ms(remove_start),
            "hiragana_len={}",
            composing_text.text.chars().count()
        );
        let get_composed_start = Instant::now();
        let composed_text = get_composed_text(false, request_id)
            .map_err(|error| status_from_error("remove_text", error))?;
        performance_event_lazy!(
            request_id,
            "remove_text",
            "swift_get_composed_text",
            elapsed_ms(get_composed_start),
            "suggestions={}",
            composed_text.suggestions.len()
        );
        update_active_composition_state(&composing_text.text);
        performance_event_lazy!(
            request_id,
            "remove_text",
            "total",
            elapsed_ms(handler_start),
            "status=success;cursor={};hiragana_len={};suggestions={}",
            composing_text.cursor,
            composing_text.text.chars().count(),
            composed_text.suggestions.len()
        );

        Ok(Response::new(RemoveTextResponse {
            composing_text: Some(ComposingText {
                hiragana: composed_text.hiragana.unwrap_or(composing_text.text),
                suggestions: composed_text.suggestions,
            }),
            server_session_id: server_session_id(),
        }))
    }

    async fn move_cursor(
        &self,
        request: Request<MoveCursorRequest>,
    ) -> Result<Response<MoveCursorResponse>, Status> {
        let request = request.into_inner();
        let _request_guard = ServerRequestGuard::begin(true);
        let request_id = request_id_or_next(request.request_id);
        set_request_id(request_id);
        let handler_start = Instant::now();
        let raw_offset = request.offset;

        let offset = i8_offset_from_i32("move_cursor", raw_offset)?;
        let use_cursor_prefix = offset == 0;
        let move_start = Instant::now();
        let composing_text =
            move_cursor(offset).map_err(|error| status_from_error("move_cursor", error))?;
        performance_event_lazy!(
            request_id,
            "move_cursor",
            "swift_move_cursor",
            elapsed_ms(move_start),
            "offset={raw_offset};hiragana_len={}",
            composing_text.text.chars().count()
        );
        let get_composed_start = Instant::now();
        let composed_text = get_composed_text(use_cursor_prefix, request_id)
            .map_err(|error| status_from_error("move_cursor", error))?;
        performance_event_lazy!(
            request_id,
            "move_cursor",
            "swift_get_composed_text",
            elapsed_ms(get_composed_start),
            "use_cursor_prefix={use_cursor_prefix};suggestions={}",
            composed_text.suggestions.len()
        );
        update_active_composition_state(&composing_text.text);
        performance_event_lazy!(
            request_id,
            "move_cursor",
            "total",
            elapsed_ms(handler_start),
            "status=success;cursor={};hiragana_len={};suggestions={};use_cursor_prefix={use_cursor_prefix}",
            composing_text.cursor,
            composing_text.text.chars().count(),
            composed_text.suggestions.len()
        );

        Ok(Response::new(MoveCursorResponse {
            composing_text: Some(ComposingText {
                hiragana: composed_text.hiragana.unwrap_or(composing_text.text),
                suggestions: composed_text.suggestions,
            }),
            server_session_id: server_session_id(),
        }))
    }

    async fn clear_text(
        &self,
        request: Request<ClearTextRequest>,
    ) -> Result<Response<ClearTextResponse>, Status> {
        let request = request.into_inner();
        let _request_guard = ServerRequestGuard::begin(true);
        let request_id = request_id_or_next(request.request_id);
        set_request_id(request_id);
        let handler_start = Instant::now();
        let clear_start = Instant::now();
        clear_text();
        performance_event_lazy!(
            request_id,
            "clear_text",
            "swift_clear_text",
            elapsed_ms(clear_start),
            "status=success"
        );
        performance_event_lazy!(
            request_id,
            "clear_text",
            "total",
            elapsed_ms(handler_start),
            "status=success"
        );
        HAS_ACTIVE_COMPOSITION.store(false, Ordering::Relaxed);
        Ok(Response::new(ClearTextResponse {
            server_session_id: server_session_id(),
        }))
    }

    async fn shrink_text(
        &self,
        request: Request<ShrinkTextRequest>,
    ) -> Result<Response<ShrinkTextResponse>, Status> {
        let request = request.into_inner();
        let _request_guard = ServerRequestGuard::begin(true);
        let request_id = request_id_or_next(request.request_id);
        set_request_id(request_id);
        let handler_start = Instant::now();
        let raw_offset = request.offset;

        let offset = i8_offset_from_i32("shrink_text", raw_offset)?;
        let shrink_start = Instant::now();
        let composing_text =
            shrink_text(offset).map_err(|error| status_from_error("shrink_text", error))?;
        performance_event_lazy!(
            request_id,
            "shrink_text",
            "swift_shrink_text",
            elapsed_ms(shrink_start),
            "offset={raw_offset};hiragana_len={}",
            composing_text.text.chars().count()
        );
        let get_composed_start = Instant::now();
        let composed_text = get_composed_text(false, request_id)
            .map_err(|error| status_from_error("shrink_text", error))?;
        performance_event_lazy!(
            request_id,
            "shrink_text",
            "swift_get_composed_text",
            elapsed_ms(get_composed_start),
            "suggestions={}",
            composed_text.suggestions.len()
        );
        update_active_composition_state(&composing_text.text);
        performance_event_lazy!(
            request_id,
            "shrink_text",
            "total",
            elapsed_ms(handler_start),
            "status=success;hiragana_len={};suggestions={}",
            composing_text.text.chars().count(),
            composed_text.suggestions.len()
        );

        Ok(Response::new(ShrinkTextResponse {
            composing_text: Some(ComposingText {
                hiragana: composed_text.hiragana.unwrap_or(composing_text.text),
                suggestions: composed_text.suggestions,
            }),
            server_session_id: server_session_id(),
        }))
    }

    async fn set_context(
        &self,
        request: Request<shared::proto::SetContextRequest>,
    ) -> Result<Response<shared::proto::SetContextResponse>, Status> {
        let request = request.into_inner();
        let _request_guard = ServerRequestGuard::begin(false);
        let request_id = request_id_or_next(request.request_id);
        set_request_id(request_id);
        let handler_start = Instant::now();
        let context = request.context;
        let trimmed_context = context
            .split('\r')
            .filter(|s| !s.is_empty())
            .last()
            .unwrap_or_default();
        let original_len = context.chars().count();
        let trimmed_len = trimmed_context.chars().count();

        let context = cstring_from_input("SetContext.context", trimmed_context)
            .map_err(|error| status_from_error("set_context", error))?;

        let set_context_start = Instant::now();
        unsafe { SetContext(context.as_ptr()) };
        performance_event_lazy!(
            request_id,
            "set_context",
            "swift_set_context",
            elapsed_ms(set_context_start),
            "original_len={original_len};trimmed_len={trimmed_len}"
        );
        performance_event_lazy!(
            request_id,
            "set_context",
            "total",
            elapsed_ms(handler_start),
            "status=success"
        );
        Ok(Response::new(shared::proto::SetContextResponse {
            server_session_id: server_session_id(),
        }))
    }

    async fn update_config(
        &self,
        request: Request<shared::proto::UpdateConfigRequest>,
    ) -> Result<Response<shared::proto::UpdateConfigResponse>, Status> {
        let request = request.into_inner();
        let _request_guard = ServerRequestGuard::begin(false);
        let request_id = request_id_or_next(request.request_id);
        let _log_paths = reload_server_logging_from_settings();
        set_request_id(request_id);
        let handler_start = Instant::now();
        let load_config_start = Instant::now();
        unsafe { LoadConfig() };
        let has_active_composition = query_active_composition_state();
        performance_event_lazy!(
            request_id,
            "update_config",
            "swift_load_config",
            elapsed_ms(load_config_start),
            "active_composition={has_active_composition}"
        );
        HAS_ACTIVE_COMPOSITION.store(has_active_composition, Ordering::Relaxed);
        performance_event_lazy!(
            request_id,
            "update_config",
            "total",
            elapsed_ms(handler_start),
            "status=success;active_composition={has_active_composition}"
        );
        Ok(Response::new(shared::proto::UpdateConfigResponse {
            server_session_id: server_session_id(),
        }))
    }

    async fn log_performance(
        &self,
        request: Request<PerformanceLogRequest>,
    ) -> Result<Response<PerformanceLogResponse>, Status> {
        let _request_guard = ServerRequestGuard::begin(false);
        let request = request.into_inner();
        log_performance_event(
            request.request_id,
            &request.component,
            &request.operation,
            &request.stage,
            u128::from(request.elapsed_ms),
            &request.details,
        );
        Ok(Response::new(PerformanceLogResponse {}))
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    install_panic_hook();
    let log_paths = reload_server_logging_from_settings();
    if let Some(trace) = preserve_crash_trace_if_incomplete(
        SERVER_CRASH_TRACE_FILE_NAME,
        SERVER_PREVIOUS_CRASH_TRACE_FILE_NAME,
    ) {
        log_previous_crash_trace("previous server crash trace", &trace);
    }
    log_and_consume_previous_crash_trace_if_incomplete(
        LAUNCHER_PREVIOUS_CRASH_TRACE_FILE_NAME,
        "previous launcher startup crash trace",
    );
    write_server_crash_trace("rust", "server_startup", "main", "begin", "");
    if let Some(log_paths) = log_paths {
        log_event(
            ServerLogLevel::Info,
            &format!(
                "AzookeyServer started (log_path={} performance_log_path={})",
                log_paths.server_log.display(),
                log_paths.performance_log.display()
            ),
        );
    }
    register_server_log_callbacks();

    // プロセス優先度を HIGH_PRIORITY_CLASS に引き上げ（放置後のOSスケジューリング遅延を抑制）
    unsafe {
        match SetPriorityClass(GetCurrentProcess(), HIGH_PRIORITY_CLASS) {
            Ok(()) => log_event_lazy!(ServerLogLevel::Info, "process priority set to HIGH"),
            Err(e) => log_event_lazy!(ServerLogLevel::Info, "SetPriorityClass failed: {e}"),
        }
    }

    let current_exe = std::env::current_exe()?;
    let parent_dir = current_exe
        .parent()
        .ok_or_else(|| std::io::Error::other("failed to get executable parent directory"))?;
    let parent_dir_str = parent_dir
        .to_str()
        .ok_or_else(|| std::io::Error::other("executable path is not valid UTF-8"))?;
    initialize(parent_dir_str).map_err(std::io::Error::other)?;

    let service = MyAzookeyService::default();

    tokio::spawn(async {
        let mut interval =
            tokio::time::interval(std::time::Duration::from_secs(WARMUP_INTERVAL_SECS));
        interval.tick().await;

        loop {
            interval.tick().await;
            tokio::task::yield_now().await;

            let request_id = next_request_id();
            if let Some(reason) = warmup_skip_reason() {
                let details = reason.details();
                log_event_lazy!(
                    ServerLogLevel::Debug,
                    "request_id={request_id} [warmup] skipped {details}"
                );
                performance_event_lazy!(request_id, "warmup", "skip", 0, "{details}");
                continue;
            }

            log_event_lazy!(
                ServerLogLevel::Debug,
                "request_id={request_id} [warmup] schedule_start interval_secs={WARMUP_INTERVAL_SECS};recent_input_skip_ms={WARMUP_RECENT_INPUT_SKIP_MS}"
            );
            set_request_id(request_id);
            let schedule_start = Instant::now();
            let scheduled = warmup();
            let schedule_elapsed_ms = elapsed_ms(schedule_start);
            if scheduled {
                performance_event_lazy!(
                    request_id,
                    "warmup",
                    "schedule",
                    schedule_elapsed_ms,
                    "status=scheduled"
                );
                log_event_lazy!(
                    ServerLogLevel::Debug,
                    "request_id={request_id} [warmup] scheduled elapsed_ms={schedule_elapsed_ms}"
                );
            } else {
                performance_event_lazy!(
                    request_id,
                    "warmup",
                    "skip",
                    schedule_elapsed_ms,
                    "reason=warmup_in_progress"
                );
                log_event_lazy!(
                    ServerLogLevel::Debug,
                    "request_id={request_id} [warmup] skipped reason=warmup_in_progress elapsed_ms={schedule_elapsed_ms}"
                );
            }
        }
    });

    let reflection_service = ReflectionBuilder::configure()
        .register_encoded_file_descriptor_set(shared::proto::FILE_DESCRIPTOR_SET)
        .build_v1()
        .map_err(std::io::Error::other)?;

    let incoming = TonicNamedPipeServer::new_with_first_pipe_callback("azookey_server", || {
        log_event_lazy!(ServerLogLevel::Info, "AzookeyServer listening");
        write_server_crash_trace(
            "rust",
            "server_startup",
            "listening",
            "completed",
            "pipe=azookey_server",
        );
        write_crash_trace_file(
            LAUNCHER_CRASH_TRACE_FILE_NAME,
            "rust",
            "server_startup",
            "server_listening",
            "completed",
            &format!("server_pid={};pipe=azookey_server", std::process::id()),
        );
    });

    Server::builder()
        .add_service(AzookeyServiceServer::new(service))
        .add_service(reflection_service)
        .serve_with_incoming(incoming)
        .await
        .map_err(|error| {
            log_event(
                ServerLogLevel::Error,
                &format!("AzookeyServer terminated with error: {error}"),
            );
            std::io::Error::other(error)
        })?;

    Ok(())
}
