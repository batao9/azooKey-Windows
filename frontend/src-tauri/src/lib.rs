mod ipc;
mod server_process;
mod updater;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use shared::{
    AppConfig, AppConfigLoadResult, ConfigError, ConfigRecovery, ConfigWriteGuard, RomajiRule,
};
use std::{path::PathBuf, sync::Mutex, time::Duration};

use anyhow::Context as _;

#[derive(Debug)]
pub struct AppState {
    settings: Mutex<AppConfig>,
    ipc: Mutex<Option<ipc::IPCService>>,
    config_update_lock: Mutex<()>,
    server_config_dirty: Mutex<bool>,
    startup_notice: Mutex<Option<ConfigStartupNotice>>,
}

impl AppState {
    fn new() -> Self {
        let (settings, startup_notice) = match AppConfig::new_with_recovery() {
            Ok(result) => {
                if let Some(error) = result.rewrite_error.as_ref() {
                    eprintln!("Failed to rewrite loaded settings: {}", error);
                }
                let notice = notice_from_load_result(&result);
                (result.config, notice)
            }
            Err(error) => {
                eprintln!("Failed to load settings; using defaults: {}", error);
                (AppConfig::default(), Some(notice_from_load_error(&error)))
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
            config_update_lock: Mutex::new(()),
            server_config_dirty: Mutex::new(false),
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
    changed: bool,
    config: AppConfig,
    message: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
struct ResetLearningHistoryResponse {
    reset: bool,
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

fn notice_from_rewrite_error(error: &ConfigError) -> ConfigStartupNotice {
    ConfigStartupNotice {
        kind: "rewrite_error".to_string(),
        message: format!("設定は読み込めましたが、設定ファイルの保存に失敗しました: {error}"),
        backup_path: None,
    }
}

fn notice_from_recovered_rewrite_error(
    recovery: &ConfigRecovery,
    error: &ConfigError,
) -> ConfigStartupNotice {
    ConfigStartupNotice {
        kind: "recovered_rewrite_error".to_string(),
        message: format!(
            "壊れた設定ファイルを退避しましたが、既定値設定の保存に失敗しました: {error}"
        ),
        backup_path: Some(recovery.backup_path.display().to_string()),
    }
}

fn notice_from_load_error(error: &ConfigError) -> ConfigStartupNotice {
    match error {
        ConfigError::FutureVersion {
            stored, current, ..
        } => ConfigStartupNotice {
            kind: "future_version".to_string(),
            message: format!(
                "このアプリより新しい設定ファイル（version {stored}、対応 version {current}）を検出しました。設定ファイルは書き換えず、既定値で起動しました。設定を変更するには新しいバージョンのアプリを使用してください。"
            ),
            backup_path: None,
        },
        _ => ConfigStartupNotice {
            kind: "load_error".to_string(),
            message: format!("設定の読み込みに失敗したため、既定値で起動しました: {error}"),
            backup_path: None,
        },
    }
}

fn notice_from_load_result(result: &AppConfigLoadResult) -> Option<ConfigStartupNotice> {
    match (&result.recovery, &result.rewrite_error) {
        (Some(recovery), Some(error)) => Some(notice_from_recovered_rewrite_error(recovery, error)),
        (Some(recovery), None) => Some(notice_from_recovery(recovery)),
        (None, Some(error)) => Some(notice_from_rewrite_error(error)),
        (None, None) => None,
    }
}

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[tauri::command]
fn get_config(state: tauri::State<AppState>) -> AppConfig {
    get_config_impl(&state)
}

fn get_config_impl(state: &AppState) -> AppConfig {
    let _update_guard = state.config_update_lock.lock().unwrap();
    match AppConfig::read() {
        Ok(config) => {
            *state.settings.lock().unwrap() = config.clone();
            config
        }
        Err(error) => {
            eprintln!("Failed to refresh settings from disk: {error}");
            state.settings.lock().unwrap().clone()
        }
    }
}

#[tauri::command]
fn take_config_startup_notice(state: tauri::State<AppState>) -> Option<ConfigStartupNotice> {
    state.startup_notice.lock().unwrap().take()
}

#[tauri::command]
fn update_config(
    state: tauri::State<AppState>,
    base_config: AppConfig,
    new_config: AppConfig,
) -> Result<UpdateConfigResponse, String> {
    update_config_impl(&state, base_config, new_config)
}

fn apply_config_delta(current: &mut Value, base: &Value, updated: &Value) {
    if base == updated {
        return;
    }

    let (Some(current_object), Some(base_object), Some(updated_object)) = (
        current.as_object_mut(),
        base.as_object(),
        updated.as_object(),
    ) else {
        *current = updated.clone();
        return;
    };

    for (key, updated_value) in updated_object {
        match base_object.get(key) {
            Some(base_value) if base_value == updated_value => {}
            Some(base_value) => {
                if let Some(current_value) = current_object.get_mut(key) {
                    apply_config_delta(current_value, base_value, updated_value);
                } else {
                    current_object.insert(key.clone(), updated_value.clone());
                }
            }
            None => {
                current_object.insert(key.clone(), updated_value.clone());
            }
        }
    }

    for key in base_object.keys() {
        if !updated_object.contains_key(key) {
            current_object.remove(key);
        }
    }
}

fn merge_config_update(
    current_config: AppConfig,
    base_config: AppConfig,
    new_config: AppConfig,
) -> Result<AppConfig, String> {
    let mut current = serde_json::to_value(current_config).map_err(|error| error.to_string())?;
    let base = serde_json::to_value(base_config).map_err(|error| error.to_string())?;
    let updated = serde_json::to_value(new_config).map_err(|error| error.to_string())?;
    apply_config_delta(&mut current, &base, &updated);
    serde_json::from_value(current).map_err(|error| error.to_string())
}

fn update_config_impl(
    state: &AppState,
    base_config: AppConfig,
    new_config: AppConfig,
) -> Result<UpdateConfigResponse, String> {
    let _update_guard = state.config_update_lock.lock().unwrap();
    let _cross_process_guard = ConfigWriteGuard::acquire().map_err(|error| error.to_string())?;
    let current_config = match AppConfig::read() {
        Ok(config) => config,
        Err(error) if error.is_version_compatibility_error() => {
            eprintln!("Refusing to update incompatible settings: {error}");
            return Err(error.to_string());
        }
        Err(error) => {
            eprintln!("Failed to read latest settings before update: {error}");
            state.settings.lock().unwrap().clone()
        }
    };
    let new_config = merge_config_update(current_config.clone(), base_config, new_config)?;
    let changed = current_config != new_config;
    if changed {
        new_config.write().map_err(|error| error.to_string())?;
    }
    *state.settings.lock().unwrap() = new_config.clone();

    if changed {
        *state.server_config_dirty.lock().unwrap() = true;
    } else if !*state.server_config_dirty.lock().unwrap() {
        return Ok(UpdateConfigResponse {
            saved: true,
            server_applied: true,
            changed: false,
            config: new_config,
            message: None,
        });
    }

    if let Some(ipc) = state.ipc.lock().unwrap().as_mut() {
        if let Err(error) = ipc.update_config() {
            eprintln!("Failed to notify IPC config update: {}", error);
            return Ok(UpdateConfigResponse {
                saved: true,
                server_applied: false,
                changed,
                config: new_config,
                message: Some(error.to_string()),
            });
        }
    } else {
        return Ok(UpdateConfigResponse {
            saved: true,
            server_applied: false,
            changed,
            config: new_config,
            message: Some("IPC service is not initialized".to_string()),
        });
    }

    *state.server_config_dirty.lock().unwrap() = false;
    Ok(UpdateConfigResponse {
        saved: true,
        server_applied: true,
        changed,
        config: new_config,
        message: None,
    })
}

#[tauri::command]
fn reset_learning_history(
    state: tauri::State<AppState>,
) -> Result<ResetLearningHistoryResponse, String> {
    Ok(reset_learning_history_impl(&state))
}

fn reset_learning_history_impl(state: &AppState) -> ResetLearningHistoryResponse {
    if let Some(ipc) = state.ipc.lock().unwrap().as_mut() {
        let reset = match ipc.reset_learning_memory() {
            Ok(reset) => reset,
            Err(error) => {
                eprintln!("Failed to reset learning memory: {}", error);
                return ResetLearningHistoryResponse {
                    reset: false,
                    server_applied: false,
                    message: Some(error.to_string()),
                };
            }
        };
        if !reset {
            return ResetLearningHistoryResponse {
                reset: false,
                server_applied: true,
                message: Some("サーバー側で学習履歴の削除に失敗しました。".to_string()),
            };
        }
    } else {
        return ResetLearningHistoryResponse {
            reset: false,
            server_applied: false,
            message: Some("IPC service is not initialized".to_string()),
        };
    }

    ResetLearningHistoryResponse {
        reset: true,
        server_applied: true,
        message: None,
    }
}

#[tauri::command]
fn restart_server(state: tauri::State<AppState>) -> Result<(), String> {
    restart_server_impl(&state).map_err(|error| error.to_string())
}

fn restart_server_impl(state: &AppState) -> Result<(), anyhow::Error> {
    restart_server_impl_with(state, server_process::restart_server)
}

fn restart_server_impl_with(
    state: &AppState,
    restart_process: impl FnOnce(&AppConfig) -> Result<(), anyhow::Error>,
) -> Result<(), anyhow::Error> {
    let _update_guard = state.config_update_lock.lock().unwrap();
    let config = {
        let _cross_process_guard = ConfigWriteGuard::acquire()?;
        let config = AppConfig::read().unwrap_or_else(|error| {
            eprintln!("Failed to refresh settings before server restart: {error}");
            state.settings.lock().unwrap().clone()
        });
        *state.settings.lock().unwrap() = config.clone();
        config
    };

    restart_process(&config)?;

    let ipc = ipc::IPCService::new_with_timeout(Duration::from_secs(10))
        .context("Server restarted, but IPC reconnect failed")?;
    *state.ipc.lock().unwrap() = Some(ipc);
    *state.server_config_dirty.lock().unwrap() = false;

    Ok(())
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

#[tauri::command]
async fn check_for_updates() -> Result<updater::UpdateCheckResponse, String> {
    updater::check_for_updates()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn start_update() -> Result<updater::UpdateStartResponse, String> {
    updater::download_and_launch_update()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn take_update_install_result() -> Result<Option<updater::UpdateInstallResult>, String> {
    updater::take_update_install_result().map_err(|error| error.to_string())
}

/// Runs the production updater without starting the Tauri UI for Windows VM
/// verification. The marker must be created beside the protected executable by
/// an administrator, so normal users cannot turn environment overrides into an
/// unattended update trigger.
pub fn run_updater_integration_test() -> Result<updater::UpdateStartResponse, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let install_dir = executable
        .parent()
        .ok_or_else(|| "failed to resolve frontend install directory".to_string())?;
    let marker = install_dir.join(".azookey-updater-integration-test");
    if !marker.is_file() {
        return Err(format!(
            "protected updater integration marker is missing: {}",
            marker.display()
        ));
    }

    let runtime = tokio::runtime::Runtime::new().map_err(|error| error.to_string())?;
    runtime
        .block_on(updater::download_and_launch_update_for_integration_test())
        .map_err(|error| error.to_string())
}

/// Runs the protected updater helper copy. The helper reopens and hashes the
/// downloaded installer while denying write/delete sharing, then holds that
/// handle until Windows has created the elevated installer process.
pub fn run_updater_helper() -> Result<(), String> {
    updater::run_installer_helper_cli(std::env::args_os().skip(2))
        .map_err(|error| error.to_string())
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
            get_default_romaji_rows,
            check_for_updates,
            start_update,
            take_update_install_result,
            restart_server,
            reset_learning_history
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
pub(crate) fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{env, ffi::OsString, fs, io, path::Path, sync::MutexGuard};

    fn env_lock() -> MutexGuard<'static, ()> {
        crate::test_env_lock()
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
            config_update_lock: Mutex::new(()),
            server_config_dirty: Mutex::new(false),
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

        let result = update_config_impl(&state, AppConfig::default(), config).unwrap();

        assert!(result.saved);
        assert!(!result.server_applied);
        assert!(result.changed);
        assert!(result.message.is_some());
        assert!(temp.path().join("Azookey").join("settings.json").exists());
        assert!(state.settings.lock().unwrap().zenzai.enable);
    }

    #[test]
    fn update_config_skips_disk_and_server_for_unchanged_settings() {
        let _appdata = AppDataGuard::unset();
        let state = test_state();

        let config = AppConfig::default();
        let result = update_config_impl(&state, config.clone(), config).unwrap();

        assert!(result.saved);
        assert!(result.server_applied);
        assert!(!result.changed);
        assert!(result.message.is_none());
    }

    #[test]
    fn update_config_returns_error_when_save_fails() {
        let _appdata = AppDataGuard::unset();
        let state = test_state();
        let mut config = AppConfig::default();
        config.zenzai.enable = true;

        let error = update_config_impl(&state, AppConfig::default(), config)
            .expect_err("save failure should be returned to the UI");

        assert!(error.contains("APPDATA"));
        assert!(!state.settings.lock().unwrap().zenzai.enable);
    }

    #[test]
    fn unchanged_settings_remain_pending_after_server_apply_failure() {
        let temp = tempfile::tempdir().unwrap();
        let _appdata = AppDataGuard::set(temp.path());
        let state = test_state();
        let mut config = AppConfig::default();
        config.zenzai.profile = "profile".to_string();

        let first = update_config_impl(&state, AppConfig::default(), config.clone()).unwrap();
        let retry = update_config_impl(&state, config.clone(), config).unwrap();

        assert!(first.changed);
        assert!(!first.server_applied);
        assert!(!retry.changed);
        assert!(!retry.server_applied);
        assert!(*state.server_config_dirty.lock().unwrap());
    }

    #[test]
    fn stale_full_config_update_preserves_newer_unrelated_fields() {
        let temp = tempfile::tempdir().unwrap();
        let _appdata = AppDataGuard::set(temp.path());
        let state = test_state();
        let base = AppConfig::default();
        let mut current = base.clone();
        current.zenzai.profile = "newer profile".to_string();
        current.write().unwrap();
        let mut stale_update = base.clone();
        stale_update.shortcuts.eisu_toggle = true;

        let result = update_config_impl(&state, base, stale_update).unwrap();

        assert!(result.changed);
        assert_eq!(result.config.zenzai.profile, "newer profile");
        assert!(result.config.shortcuts.eisu_toggle);
        let persisted = AppConfig::read().unwrap();
        assert_eq!(persisted.zenzai.profile, "newer profile");
        assert!(persisted.shortcuts.eisu_toggle);
    }

    #[test]
    fn update_config_rejects_future_version_without_modifying_the_file() {
        let temp = tempfile::tempdir().unwrap();
        let _appdata = AppDataGuard::set(temp.path());
        let config_root = temp.path().join("Azookey");
        fs::create_dir_all(&config_root).unwrap();
        let config_path = config_root.join("settings.json");
        let future_settings = r#"{
            "version": "0.1.4",
            "zenzai": "future-schema",
            "future_only": { "must_be_preserved": true }
        }"#;
        fs::write(&config_path, future_settings).unwrap();
        let state = test_state();
        let mut update = AppConfig::default();
        update.zenzai.enable = true;

        let error = update_config_impl(&state, AppConfig::default(), update)
            .expect_err("future-version settings must be read-only");

        assert!(error.contains("newer than supported"));
        assert_eq!(fs::read_to_string(config_path).unwrap(), future_settings);
        assert!(!state.settings.lock().unwrap().zenzai.enable);
    }

    #[test]
    fn reset_learning_history_reports_unavailable_server() {
        let state = test_state();

        let result = reset_learning_history_impl(&state);

        assert!(!result.reset);
        assert!(!result.server_applied);
        assert!(result.message.is_some());
    }

    #[test]
    fn restart_server_keeps_existing_ipc_when_restart_fails() {
        let temp = tempfile::tempdir().unwrap();
        let _appdata = AppDataGuard::set(temp.path());
        let state = test_state();
        *state.ipc.lock().unwrap() = Some(ipc::IPCService::new_for_test());

        let error =
            restart_server_impl(&state).expect_err("missing server exe should fail restart");

        assert!(error.to_string().contains("Server executable not found"));
        assert!(state.ipc.lock().unwrap().is_some());
    }

    #[cfg(windows)]
    #[test]
    fn restart_server_releases_config_guard_before_process_request() {
        let temp = tempfile::tempdir().unwrap();
        let _appdata = AppDataGuard::set(temp.path());
        let state = test_state();

        let error = restart_server_impl_with(&state, |_| {
            let (acquired_tx, acquired_rx) = std::sync::mpsc::channel();
            let waiter = std::thread::spawn(move || {
                let _guard = ConfigWriteGuard::acquire().unwrap();
                acquired_tx.send(()).unwrap();
            });

            acquired_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("launcher-side config load must not wait for the frontend restart lock");
            waiter.join().unwrap();
            Err(anyhow::anyhow!("stop after lock-scope assertion"))
        })
        .expect_err("injected restart failure should be returned");

        assert!(error.to_string().contains("lock-scope assertion"));
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

    #[test]
    fn rewrite_error_notice_keeps_loaded_config_message() {
        let result = AppConfigLoadResult {
            config: AppConfig::default(),
            recovery: None,
            rewrite_error: Some(ConfigError::WriteTemp {
                path: PathBuf::from("settings.json.tmp-test"),
                source: io::Error::new(io::ErrorKind::PermissionDenied, "test write failure"),
            }),
        };

        let notice = notice_from_load_result(&result).unwrap();

        assert_eq!(notice.kind, "rewrite_error");
        assert!(notice.message.contains("設定は読み込めました"));
        assert!(notice.backup_path.is_none());
    }

    #[test]
    fn future_version_notice_explains_read_only_fallback() {
        let error = ConfigError::FutureVersion {
            path: PathBuf::from("settings.json"),
            stored: "0.1.4".to_string(),
            current: "0.1.3".to_string(),
        };

        let notice = notice_from_load_error(&error);

        assert_eq!(notice.kind, "future_version");
        assert!(notice.message.contains("version 0.1.4"));
        assert!(notice.message.contains("書き換えず"));
        assert!(notice.message.contains("新しいバージョン"));
        assert!(notice.backup_path.is_none());
    }
}
