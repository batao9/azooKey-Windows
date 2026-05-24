mod ipc;

use serde::{Deserialize, Serialize};
use shared::{AppConfig, ConfigRecovery, RomajiRule};
use std::{path::PathBuf, sync::Mutex};

#[derive(Debug)]
pub struct AppState {
    settings: Mutex<AppConfig>,
    ipc: Mutex<Option<ipc::IPCService>>,
    startup_notice: Mutex<Option<ConfigStartupNotice>>,
}

impl AppState {
    fn new() -> Self {
        let (settings, startup_notice) = match AppConfig::new_with_recovery() {
            Ok(result) => {
                let notice = result.recovery.as_ref().map(notice_from_recovery);
                (result.config, notice)
            }
            Err(error) => {
                eprintln!("Failed to load settings; using defaults: {}", error);
                (
                    AppConfig::default(),
                    Some(ConfigStartupNotice {
                        kind: "load_error".to_string(),
                        message: format!(
                            "設定の読み込みに失敗したため、既定値で起動しました: {error}"
                        ),
                        backup_path: None,
                    }),
                )
            }
        };

        let ipc = match ipc::IPCService::new() {
            Ok(service) => Some(service),
            Err(error) => {
                eprintln!("Failed to initialize IPC service: {}", error);
                None
            }
        };

        AppState {
            settings: Mutex::new(settings),
            ipc: Mutex::new(ipc),
            startup_notice: Mutex::new(startup_notice),
        }
    }
}

#[derive(Debug, Serialize, Clone)]
struct ConfigStartupNotice {
    kind: String,
    message: String,
    backup_path: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
struct UpdateConfigResponse {
    saved: bool,
    server_applied: bool,
    message: Option<String>,
}

fn notice_from_recovery(recovery: &ConfigRecovery) -> ConfigStartupNotice {
    ConfigStartupNotice {
        kind: "recovered".to_string(),
        message: "壊れた設定ファイルを退避し、既定値で起動しました。".to_string(),
        backup_path: Some(recovery.backup_path.display().to_string()),
    }
}

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[tauri::command]
fn get_config(state: tauri::State<AppState>) -> AppConfig {
    let config = state.settings.lock().unwrap();
    config.clone()
}

#[tauri::command]
fn take_config_startup_notice(state: tauri::State<AppState>) -> Option<ConfigStartupNotice> {
    state.startup_notice.lock().unwrap().take()
}

#[tauri::command]
fn update_config(
    state: tauri::State<AppState>,
    new_config: AppConfig,
) -> Result<UpdateConfigResponse, String> {
    update_config_impl(&state, new_config)
}

fn update_config_impl(
    state: &AppState,
    new_config: AppConfig,
) -> Result<UpdateConfigResponse, String> {
    new_config.write().map_err(|error| error.to_string())?;

    {
        let mut config = state.settings.lock().unwrap();
        *config = new_config;
    }

    if let Some(ipc) = state.ipc.lock().unwrap().as_mut() {
        if let Err(error) = ipc.update_config() {
            eprintln!("Failed to notify IPC config update: {}", error);
            return Ok(UpdateConfigResponse {
                saved: true,
                server_applied: false,
                message: Some(error.to_string()),
            });
        }
    } else {
        return Ok(UpdateConfigResponse {
            saved: true,
            server_applied: false,
            message: Some("IPC service is not initialized".to_string()),
        });
    }

    Ok(UpdateConfigResponse {
        saved: true,
        server_applied: true,
        message: None,
    })
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct Capability {
    cpu: bool,
    cuda: bool,
    vulkan: bool,
}

#[tauri::command]
fn check_capability() -> Capability {
    // cuda:
    // cudart64_12.dll
    // cublas64_12.dll

    // vulkan:
    // vulkan-1.dllの存在確認

    let mut capability = Capability {
        cpu: shared::zenzai_cpu_backend_supported(),
        cuda: false,
        vulkan: false,
    };

    // Check for CUDA availability
    let cuda_files = ["cudart64_12.dll", "cublas64_12.dll"];
    let cuda_available = cuda_files.iter().all(|file| {
        // Check if the file exists in system path or in the current directory
        std::env::var("PATH")
            .unwrap_or_default()
            .split(';')
            .map(PathBuf::from)
            .chain(std::iter::once(std::env::current_dir().unwrap_or_default()))
            .any(|path| path.join(file).exists())
    });
    capability.cuda = cuda_available;

    // Check for Vulkan availability
    let vulkan_file = "vulkan-1.dll";
    let vulkan_available = std::env::var("PATH")
        .unwrap_or_default()
        .split(';')
        .map(PathBuf::from)
        .chain(std::iter::once(std::env::current_dir().unwrap_or_default()))
        .any(|path| path.join(vulkan_file).exists());
    capability.vulkan = vulkan_available;

    capability
}

#[tauri::command]
fn get_default_romaji_rows() -> Vec<RomajiRule> {
    shared::get_default_romaji_rows()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app_state = AppState::new();

    tauri::Builder::default()
        .manage(app_state)
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            greet,
            get_config,
            take_config_startup_notice,
            update_config,
            check_capability,
            get_default_romaji_rows
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        env,
        ffi::OsString,
        path::Path,
        sync::{Mutex, MutexGuard, OnceLock},
    };

    fn env_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    struct AppDataGuard {
        _guard: MutexGuard<'static, ()>,
        previous: Option<OsString>,
    }

    impl AppDataGuard {
        fn set(path: &Path) -> Self {
            let guard = env_lock();
            let previous = env::var_os("APPDATA");
            unsafe {
                env::set_var("APPDATA", path);
            }
            Self {
                _guard: guard,
                previous,
            }
        }

        fn unset() -> Self {
            let guard = env_lock();
            let previous = env::var_os("APPDATA");
            unsafe {
                env::remove_var("APPDATA");
            }
            Self {
                _guard: guard,
                previous,
            }
        }
    }

    impl Drop for AppDataGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.previous {
                    Some(value) => env::set_var("APPDATA", value),
                    None => env::remove_var("APPDATA"),
                }
            }
        }
    }

    fn test_state() -> AppState {
        AppState {
            settings: Mutex::new(AppConfig::default()),
            ipc: Mutex::new(None),
            startup_notice: Mutex::new(None),
        }
    }

    #[test]
    fn update_config_reports_saved_when_server_is_unavailable() {
        let temp = tempfile::tempdir().unwrap();
        let _appdata = AppDataGuard::set(temp.path());
        let state = test_state();
        let mut config = AppConfig::default();
        config.zenzai.enable = true;

        let result = update_config_impl(&state, config).unwrap();

        assert!(result.saved);
        assert!(!result.server_applied);
        assert!(result.message.is_some());
        assert!(temp.path().join("Azookey").join("settings.json").exists());
        assert!(state.settings.lock().unwrap().zenzai.enable);
    }

    #[test]
    fn update_config_returns_error_when_save_fails() {
        let _appdata = AppDataGuard::unset();
        let state = test_state();

        let error = update_config_impl(&state, AppConfig::default())
            .expect_err("save failure should be returned to the UI");

        assert!(error.contains("APPDATA"));
        assert!(!state.settings.lock().unwrap().zenzai.enable);
    }

    #[test]
    fn recovery_notice_includes_backup_path() {
        let recovery = ConfigRecovery {
            original_path: PathBuf::from("settings.json"),
            backup_path: PathBuf::from("settings.json.broken-20260524120000"),
        };

        let notice = notice_from_recovery(&recovery);

        assert_eq!(notice.kind, "recovered");
        assert_eq!(
            notice.backup_path.as_deref(),
            Some("settings.json.broken-20260524120000")
        );
    }
}
