use azookey_server::TonicNamedPipeServer;
use tonic::{transport::Server, Request, Response, Status};
use tonic_reflection::server::Builder as ReflectionBuilder;

use shared::proto::azookey_service_server::{AzookeyService, AzookeyServiceServer};
use shared::proto::{
    AppendTextRequest, AppendTextResponse, ClearTextRequest, ClearTextResponse, ComposingText,
    MoveCursorRequest, MoveCursorResponse, RemoveTextRequest, RemoveTextResponse,
    ShrinkTextRequest, ShrinkTextResponse, Suggestion,
};

use std::{
    backtrace::Backtrace,
    ffi::{c_char, c_int, CStr, CString},
    fs::{self, File, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, OnceLock,
    },
    time::{SystemTime, UNIX_EPOCH},
};

const USE_ZENZAI: bool = true;
const INPUT_STYLE_DIRECT: i32 = 1;
const SERVER_LOG_FILE_NAME: &str = "server.log";

static SERVER_LOG_FILE: OnceLock<Mutex<Option<File>>> = OnceLock::new();
static SERVER_LOG_LEVEL: OnceLock<ServerLogLevel> = OnceLock::new();
static REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum ServerLogLevel {
    Off = 0,
    Error = 1,
    Warn = 2,
    Info = 3,
    Debug = 4,
}

impl ServerLogLevel {
    fn from_label(label: &str) -> Self {
        if label.eq_ignore_ascii_case("off") {
            Self::Off
        } else if label.eq_ignore_ascii_case("error") || label.eq_ignore_ascii_case("panic") {
            Self::Error
        } else if label.eq_ignore_ascii_case("warn") || label.eq_ignore_ascii_case("warning") {
            Self::Warn
        } else if label.eq_ignore_ascii_case("debug") {
            Self::Debug
        } else {
            Self::Info
        }
    }

    fn from_environment() -> Self {
        std::env::var("AZOOKEY_SERVER_LOG_LEVEL")
            .map(|value| Self::from_label(&value))
            .unwrap_or(Self::Warn)
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::Error => "ERROR",
            Self::Warn => "WARN",
            Self::Info => "INFO",
            Self::Debug => "DEBUG",
        }
    }
}

fn current_server_log_level() -> ServerLogLevel {
    *SERVER_LOG_LEVEL.get_or_init(ServerLogLevel::from_environment)
}

fn should_log(level: ServerLogLevel) -> bool {
    level <= current_server_log_level()
}

macro_rules! log_event_lazy {
    ($level:expr, $($arg:tt)*) => {{
        let level = $level;
        if should_log(level) {
            log_event(level, &format!($($arg)*));
        }
    }};
}

struct RawComposingText {
    text: String,
    cursor: i8,
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
    fn GetComposedText(lengthPtr: *mut c_int) -> *mut *mut FFICandidate;
    fn GetComposedTextForCursorPrefix(lengthPtr: *mut c_int) -> *mut *mut FFICandidate;
    fn LoadConfig();
}

fn next_request_id() -> u64 {
    REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
}

fn now_timestamp_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

fn resolve_server_log_path() -> PathBuf {
    if let Ok(appdata) = std::env::var("APPDATA") {
        PathBuf::from(appdata)
            .join("Azookey")
            .join("logs")
            .join(SERVER_LOG_FILE_NAME)
    } else {
        std::env::temp_dir()
            .join("Azookey")
            .join("logs")
            .join(SERVER_LOG_FILE_NAME)
    }
}

fn init_server_log_file() -> PathBuf {
    let log_path = resolve_server_log_path();
    let mut file = None;

    if let Some(parent) = log_path.parent() {
        if let Err(error) = fs::create_dir_all(parent) {
            eprintln!("Failed to create log directory: {error}");
        } else {
            match OpenOptions::new().create(true).append(true).open(&log_path) {
                Ok(opened) => file = Some(opened),
                Err(error) => eprintln!("Failed to open server log file: {error}"),
            }
        }
    }

    let slot = SERVER_LOG_FILE.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = slot.lock() {
        *guard = file;
    }

    log_path
}

fn log_event(level: ServerLogLevel, message: &str) {
    if !should_log(level) {
        return;
    }

    let line = format!(
        "[{}] [{}] {}",
        now_timestamp_millis(),
        level.as_str(),
        message
    );
    eprintln!("{line}");

    if let Some(slot) = SERVER_LOG_FILE.get() {
        if let Ok(mut guard) = slot.lock() {
            if let Some(file) = guard.as_mut() {
                let _ = writeln!(file, "{line}");
                let _ = file.flush();
            }
        }
    }
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
        log_event(
            ServerLogLevel::Error,
            &format!("payload={payload}; location={location}; backtrace={backtrace}"),
        );

        default_hook(panic_info);
    }));
}

fn cstring_from_input(scope: &str, value: &str) -> Result<CString, String> {
    CString::new(value).map_err(|error| format!("[{scope}] CString::new failed: {error}"))
}

fn ffi_text_result(scope: &str, result: *mut c_char) -> Result<String, String> {
    if result.is_null() {
        return Err(format!("[{scope}] Swift FFI returned null pointer"));
    }

    let text = unsafe { CStr::from_ptr(result as *const c_char) }
        .to_string_lossy()
        .into_owned();

    Ok(text)
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

fn get_composed_text(use_cursor_prefix: bool) -> Result<Vec<Suggestion>, String> {
    unsafe {
        let mut length: c_int = 0;
        let result = if use_cursor_prefix {
            GetComposedTextForCursorPrefix(&mut length)
        } else {
            GetComposedText(&mut length)
        };
        let call_name = if use_cursor_prefix {
            "GetComposedTextForCursorPrefix"
        } else {
            "GetComposedText"
        };
        if length < 0 {
            return Err(format!("[{call_name}] invalid negative length: {length}"));
        }

        let length = length as usize;
        if length > 0 && result.is_null() {
            return Err(format!(
                "[{call_name}] null candidate list pointer (length={length})"
            ));
        }

        let mut suggestions = Vec::with_capacity(length);
        log_event_lazy!(
            ServerLogLevel::Debug,
            "[{call_name}] candidate_count={length}"
        );

        for index in 0..length {
            let candidate_ptr = *result.add(index);
            if candidate_ptr.is_null() {
                log_event(
                    ServerLogLevel::Warn,
                    &format!("[{call_name}] candidate[{index}] is null and skipped"),
                );
                continue;
            }

            let candidate = (*candidate_ptr).clone();
            if candidate.text.is_null() || candidate.subtext.is_null() {
                log_event(
                    ServerLogLevel::Warn,
                    &format!(
                        "[{call_name}] candidate[{index}] has null text/subtext pointer and was skipped"
                    ),
                );
                continue;
            }

            let text = CStr::from_ptr(candidate.text)
                .to_string_lossy()
                .into_owned();
            let subtext = CStr::from_ptr(candidate.subtext)
                .to_string_lossy()
                .into_owned();
            let corresponding_count = candidate.corresponding_count;

            let suggestion = Suggestion {
                text,
                subtext,
                corresponding_count,
            };

            if suggestions
                .iter()
                .any(|s: &Suggestion| s.text == suggestion.text)
            {
                continue;
            }
            suggestions.push(suggestion);
        }

        Ok(suggestions)
    }
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
        let request_id = next_request_id();
        let request = request.into_inner();
        let input = request.text_to_append;
        log_event_lazy!(
            ServerLogLevel::Info,
            "[append_text:{request_id}] start input_len={} input_style={}",
            input.chars().count(),
            request.input_style
        );
        let composing_text = if request.input_style == INPUT_STYLE_DIRECT {
            add_text_direct(&input).map_err(|error| status_from_error("append_text", error))?
        } else {
            add_text(&input).map_err(|error| status_from_error("append_text", error))?
        };
        let suggestions =
            get_composed_text(false).map_err(|error| status_from_error("append_text", error))?;

        log_event_lazy!(
            ServerLogLevel::Info,
            "[append_text:{request_id}] success cursor={} hiragana_len={} suggestions={}",
            composing_text.cursor,
            composing_text.text.chars().count(),
            suggestions.len()
        );

        Ok(Response::new(AppendTextResponse {
            composing_text: Some(ComposingText {
                hiragana: composing_text.text,
                suggestions,
            }),
        }))
    }

    async fn remove_text(
        &self,
        _: Request<RemoveTextRequest>,
    ) -> Result<Response<RemoveTextResponse>, Status> {
        let request_id = next_request_id();
        log_event_lazy!(ServerLogLevel::Info, "[remove_text:{request_id}] start");

        let composing_text =
            remove_text().map_err(|error| status_from_error("remove_text", error))?;
        let suggestions =
            get_composed_text(false).map_err(|error| status_from_error("remove_text", error))?;

        log_event_lazy!(
            ServerLogLevel::Info,
            "[remove_text:{request_id}] success cursor={} hiragana_len={} suggestions={}",
            composing_text.cursor,
            composing_text.text.chars().count(),
            suggestions.len()
        );

        Ok(Response::new(RemoveTextResponse {
            composing_text: Some(ComposingText {
                hiragana: composing_text.text,
                suggestions,
            }),
        }))
    }

    async fn move_cursor(
        &self,
        request: Request<MoveCursorRequest>,
    ) -> Result<Response<MoveCursorResponse>, Status> {
        let request_id = next_request_id();
        let raw_offset = request.into_inner().offset;
        log_event_lazy!(
            ServerLogLevel::Info,
            "[move_cursor:{request_id}] start offset={raw_offset}"
        );

        let offset = i8_offset_from_i32("move_cursor", raw_offset)?;
        let use_cursor_prefix = offset == 0;
        let composing_text =
            move_cursor(offset).map_err(|error| status_from_error("move_cursor", error))?;
        let suggestions = get_composed_text(use_cursor_prefix)
            .map_err(|error| status_from_error("move_cursor", error))?;

        log_event_lazy!(
            ServerLogLevel::Info,
            "[move_cursor:{request_id}] success cursor={} hiragana_len={} suggestions={} use_cursor_prefix={use_cursor_prefix}",
            composing_text.cursor,
            composing_text.text.chars().count(),
            suggestions.len()
        );

        Ok(Response::new(MoveCursorResponse {
            composing_text: Some(ComposingText {
                hiragana: composing_text.text,
                suggestions,
            }),
        }))
    }

    async fn clear_text(
        &self,
        _: Request<ClearTextRequest>,
    ) -> Result<Response<ClearTextResponse>, Status> {
        let request_id = next_request_id();
        log_event_lazy!(ServerLogLevel::Info, "[clear_text:{request_id}] start");
        clear_text();
        log_event_lazy!(ServerLogLevel::Info, "[clear_text:{request_id}] success");
        Ok(Response::new(ClearTextResponse {}))
    }

    async fn shrink_text(
        &self,
        request: Request<ShrinkTextRequest>,
    ) -> Result<Response<ShrinkTextResponse>, Status> {
        let request_id = next_request_id();
        let raw_offset = request.into_inner().offset;
        log_event_lazy!(
            ServerLogLevel::Info,
            "[shrink_text:{request_id}] start offset={raw_offset}"
        );

        let offset = i8_offset_from_i32("shrink_text", raw_offset)?;
        let composing_text =
            shrink_text(offset).map_err(|error| status_from_error("shrink_text", error))?;
        let suggestions =
            get_composed_text(false).map_err(|error| status_from_error("shrink_text", error))?;

        log_event_lazy!(
            ServerLogLevel::Info,
            "[shrink_text:{request_id}] success hiragana_len={} suggestions={}",
            composing_text.text.chars().count(),
            suggestions.len()
        );

        Ok(Response::new(ShrinkTextResponse {
            composing_text: Some(ComposingText {
                hiragana: composing_text.text,
                suggestions,
            }),
        }))
    }

    async fn set_context(
        &self,
        request: Request<shared::proto::SetContextRequest>,
    ) -> Result<Response<shared::proto::SetContextResponse>, Status> {
        let request_id = next_request_id();
        let context = request.into_inner().context;
        let trimmed_context = context
            .split('\r')
            .filter(|s| !s.is_empty())
            .last()
            .unwrap_or_default();
        log_event_lazy!(
            ServerLogLevel::Info,
            "[set_context:{request_id}] start original_len={} trimmed_len={}",
            context.chars().count(),
            trimmed_context.chars().count()
        );

        let context = cstring_from_input("SetContext.context", trimmed_context)
            .map_err(|error| status_from_error("set_context", error))?;

        unsafe { SetContext(context.as_ptr()) };
        log_event_lazy!(ServerLogLevel::Info, "[set_context:{request_id}] success");
        Ok(Response::new(shared::proto::SetContextResponse {}))
    }

    async fn update_config(
        &self,
        _: Request<shared::proto::UpdateConfigRequest>,
    ) -> Result<Response<shared::proto::UpdateConfigResponse>, Status> {
        let request_id = next_request_id();
        log_event_lazy!(ServerLogLevel::Info, "[update_config:{request_id}] start");
        unsafe { LoadConfig() };
        log_event_lazy!(ServerLogLevel::Info, "[update_config:{request_id}] success");
        Ok(Response::new(shared::proto::UpdateConfigResponse {}))
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let log_path = init_server_log_file();
    install_panic_hook();
    log_event_lazy!(
        ServerLogLevel::Info,
        "AzookeyServer started (log_path={})",
        log_path.display()
    );

    let current_exe = std::env::current_exe()?;
    let parent_dir = current_exe
        .parent()
        .ok_or_else(|| std::io::Error::other("failed to get executable parent directory"))?;
    let parent_dir_str = parent_dir
        .to_str()
        .ok_or_else(|| std::io::Error::other("executable path is not valid UTF-8"))?;
    initialize(parent_dir_str).map_err(std::io::Error::other)?;

    let service = MyAzookeyService::default();

    log_event_lazy!(ServerLogLevel::Info, "AzookeyServer listening");
    let reflection_service = ReflectionBuilder::configure()
        .register_encoded_file_descriptor_set(shared::proto::FILE_DESCRIPTOR_SET)
        .build_v1()
        .map_err(std::io::Error::other)?;

    Server::builder()
        .add_service(AzookeyServiceServer::new(service))
        .add_service(reflection_service)
        .serve_with_incoming(TonicNamedPipeServer::new("azookey_server"))
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
