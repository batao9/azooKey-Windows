use std::sync::OnceLock;

#[cfg(debug_assertions)]
use std::{fmt::Write as _, path::PathBuf};
#[cfg(debug_assertions)]
use tracing::field::{Field, Visit};
#[cfg(debug_assertions)]
use tracing_core::LevelFilter;
#[cfg(debug_assertions)]
use tracing_subscriber::filter::Targets;
#[cfg(debug_assertions)]
use tracing_subscriber::{layer::SubscriberExt as _, util::SubscriberInitExt};
#[cfg(debug_assertions)]
use windows::{core::PCWSTR, Win32::System::Diagnostics::Debug::OutputDebugStringW};

#[cfg(debug_assertions)]
use crate::extension::StringExt as _;
#[cfg(debug_assertions)]
use crate::tracing_chrome::{ChromeLayerBuilder, EventOrSpan};

static LOGGER_INIT_RESULT: OnceLock<Result<(), String>> = OnceLock::new();

#[cfg(debug_assertions)]
pub struct StringVisitor<'a> {
    string: &'a mut String,
}

#[cfg(debug_assertions)]
impl<'a> Visit for StringVisitor<'a> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        // do nothing
        if field.name() == "message" {
            write!(self.string, "{:?}", value).unwrap();
        }
    }
}

pub fn diagnostic_log(_message: impl AsRef<str>) {}

#[inline(always)]
pub const fn diagnostic_log_enabled() -> bool {
    false
}

#[inline(always)]
pub fn diagnostic_log_lazy(message: impl FnOnce() -> String) {
    if diagnostic_log_enabled() {
        diagnostic_log(message());
    }
}

#[cfg(debug_assertions)]
fn resolve_trace_log_folder() -> PathBuf {
    if let Ok(appdata) = std::env::var("APPDATA") {
        return PathBuf::from(appdata).join("Azookey").join("logs");
    }

    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("logs")
}

pub fn initialize_logger() {
    if let Err(error) =
        LOGGER_INIT_RESULT.get_or_init(|| contain_logger_initialization(setup_logger))
    {
        diagnostic_log(format!("logger initialization disabled: {error}"));
    }
}

fn contain_logger_initialization(
    initializer: impl FnOnce() -> anyhow::Result<()>,
) -> Result<(), String> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(initializer)) {
        Ok(result) => result.map_err(|error| error.to_string()),
        Err(_) => Err("logger initialization panicked".to_string()),
    }
}

#[cfg(not(debug_assertions))]
fn setup_logger() -> anyhow::Result<()> {
    Ok(())
}

#[cfg(debug_assertions)]
fn setup_logger() -> anyhow::Result<()> {
    diagnostic_log("setup_logger called");
    let timestamp = chrono::Local::now().format("%Y-%m-%d-%H.%M.%S");
    let log_folder = resolve_trace_log_folder();
    let _ = std::fs::create_dir_all(&log_folder);
    let path = log_folder.join(format!("{}.json", timestamp));

    let writer = {
        if let Ok(file) = std::fs::File::create(&path) {
            file
        } else {
            return Ok(());
        }
    };

    let builder = ChromeLayerBuilder::new()
        .file(writer)
        .include_locations(true)
        .include_args(true)
        .name_fn(Box::new(|event_or_span| match event_or_span {
            EventOrSpan::Event(event) => {
                let message = {
                    let mut message = String::new();
                    event.record(&mut StringVisitor {
                        string: &mut message,
                    });
                    message
                };

                let (level, file, line) = {
                    let metadeta = event.metadata();
                    let level = metadeta.level().as_str();
                    let file = metadeta.file().unwrap_or_default();
                    let line = metadeta.line().unwrap_or_default();

                    (level, file, line)
                };

                let str = format!("[{}: {}:{}] {}", level, file, line, message);
                let wide: Vec<u16> = str.as_str().to_wide_16();
                unsafe { OutputDebugStringW(PCWSTR(wide.as_ptr())) };

                message
            }
            EventOrSpan::Span(span) => span.metadata().name().to_string(),
        }));

    let chrome_layer = builder.build();

    // ignore traces from other crates
    let filter = Targets::new()
        .with_target("azookey_windows", LevelFilter::DEBUG)
        .with_default(LevelFilter::OFF);

    tracing_subscriber::registry()
        .with(filter)
        .with(chrome_layer)
        .try_init()
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::{contain_logger_initialization, diagnostic_log_lazy};

    #[test]
    fn disabled_diagnostic_log_does_not_build_message() {
        let called = Cell::new(false);

        diagnostic_log_lazy(|| {
            called.set(true);
            "candidate and snapshot details".to_owned()
        });

        assert!(!called.get());
    }

    #[test]
    fn logger_initialization_error_is_contained() {
        let result = contain_logger_initialization(|| anyhow::bail!("expected failure"));

        assert_eq!(result, Err("expected failure".to_string()));
    }

    #[test]
    fn logger_initialization_panic_is_contained() {
        let result = contain_logger_initialization(|| panic!("expected panic"));

        assert_eq!(result, Err("logger initialization panicked".to_string()));
    }
}
