use std::{
    fs, io,
    path::PathBuf,
    sync::{Arc, LazyLock, Mutex, MutexGuard},
    time::SystemTime,
};

use shared::AppConfig;
use windows::{
    core::Interface as _,
    Win32::UI::TextServices::{ITfCompartmentMgr, ITfContext, GUID_COMPARTMENT_KEYBOARD_DISABLED},
};

use super::{input_mode::InputMode, ipc_service::IPCService, romaji_lookup::RomajiLookup};

#[derive(Clone, Debug, PartialEq, Eq)]
enum AppConfigCacheKey {
    File {
        path: PathBuf,
        len: u64,
        modified: Option<SystemTime>,
    },
    Missing {
        path: PathBuf,
    },
    Unavailable {
        reason: String,
    },
}

impl AppConfigCacheKey {
    fn current() -> Result<Self, shared::ConfigError> {
        Self::for_path(AppConfig::settings_path()?)
    }

    fn for_path(path: PathBuf) -> Result<Self, shared::ConfigError> {
        match fs::metadata(&path) {
            Ok(metadata) => Ok(Self::File {
                path,
                len: metadata.len(),
                modified: metadata.modified().ok(),
            }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::Missing { path }),
            Err(source) => Err(shared::ConfigError::Read { path, source }),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        env,
        ffi::OsString,
        fs,
        path::PathBuf,
        sync::{Mutex, MutexGuard, OnceLock},
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{AppConfigCacheKey, IMEState};

    fn env_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    struct AppDataGuard {
        _guard: MutexGuard<'static, ()>,
        previous: Option<OsString>,
    }

    impl AppDataGuard {
        fn set(path: &PathBuf) -> Self {
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

    fn unique_test_dir(test_name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after UNIX_EPOCH")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "azookey-windows-{test_name}-{}-{nanos}",
            std::process::id()
        ))
    }

    #[test]
    fn app_config_cache_key_uses_missing_variant_for_absent_settings_file() {
        let root = unique_test_dir("missing-config");
        fs::create_dir_all(&root).expect("temp config dir should be created");
        let path = root.join("settings.json");

        let cache_key = AppConfigCacheKey::for_path(path.clone()).unwrap();

        assert_eq!(cache_key, AppConfigCacheKey::Missing { path });
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn app_config_cache_key_changes_when_settings_file_changes_size() {
        let root = unique_test_dir("changed-config");
        fs::create_dir_all(&root).expect("temp config dir should be created");
        let path = root.join("settings.json");
        fs::write(&path, "a").expect("initial settings should be written");
        let initial = AppConfigCacheKey::for_path(path.clone()).unwrap();

        fs::write(&path, "abcd").expect("updated settings should be written");
        let updated = AppConfigCacheKey::for_path(path).unwrap();

        assert_ne!(initial, updated);
        assert!(matches!(updated, AppConfigCacheKey::File { len: 4, .. }));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn app_config_snapshot_does_not_cache_default_after_read_error() {
        let root = unique_test_dir("config-read-error");
        let config_root = root.join("Azookey");
        fs::create_dir_all(&config_root).expect("temp config dir should be created");
        fs::create_dir(config_root.join("settings.json"))
            .expect("settings path directory should cause read_to_string to fail");
        let _appdata = AppDataGuard::set(&root);

        IMEState::get().unwrap().app_config_snapshot = None;

        let snapshot = IMEState::app_config_snapshot().unwrap();
        assert!(matches!(
            &snapshot.cache_key,
            AppConfigCacheKey::Unavailable { .. }
        ));
        assert!(IMEState::get().unwrap().app_config_snapshot.is_none());

        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn app_config_snapshot_keeps_previous_cache_after_read_error() {
        let root = unique_test_dir("config-read-error-keeps-cache");
        let config_root = root.join("Azookey");
        fs::create_dir_all(&config_root).expect("temp config dir should be created");
        let _appdata = AppDataGuard::set(&root);

        IMEState::get().unwrap().app_config_snapshot = None;
        let initial = IMEState::app_config_snapshot().unwrap();
        assert!(matches!(
            &initial.cache_key,
            AppConfigCacheKey::Missing { .. }
        ));

        fs::create_dir(config_root.join("settings.json"))
            .expect("settings path directory should cause read_to_string to fail");
        let after_error = IMEState::app_config_snapshot().unwrap();
        assert_eq!(&after_error.cache_key, &initial.cache_key);
        assert_eq!(
            &IMEState::get()
                .unwrap()
                .app_config_snapshot
                .as_ref()
                .unwrap()
                .cache_key,
            &initial.cache_key
        );

        IMEState::get().unwrap().app_config_snapshot = None;
        fs::remove_dir_all(root).ok();
    }
}

#[derive(Clone, Debug)]
pub(crate) struct AppConfigSnapshot {
    app_config: Arc<AppConfig>,
    romaji_lookup: Arc<RomajiLookup>,
    cache_key: AppConfigCacheKey,
}

impl AppConfigSnapshot {
    fn new(app_config: AppConfig, cache_key: AppConfigCacheKey) -> Self {
        let romaji_lookup = RomajiLookup::from_rows(&app_config.romaji_table.rows);
        Self {
            app_config: Arc::new(app_config),
            romaji_lookup: Arc::new(romaji_lookup),
            cache_key,
        }
    }

    pub(super) fn app_config(&self) -> &AppConfig {
        self.app_config.as_ref()
    }

    pub(super) fn romaji_lookup(&self) -> &RomajiLookup {
        self.romaji_lookup.as_ref()
    }
}

#[derive(Debug)]
pub struct IMEState {
    pub ipc_service: Option<IPCService>,
    pub input_mode: InputMode,
    pub keyboard_disabled: bool,
    app_config_snapshot: Option<AppConfigSnapshot>,
}

pub static IME_STATE: LazyLock<Mutex<IMEState>> = LazyLock::new(|| {
    tracing::debug!("Creating IMEState");
    Mutex::new(IMEState {
        ipc_service: None,
        input_mode: InputMode::default(),
        keyboard_disabled: false,
        app_config_snapshot: None,
    })
});

impl IMEState {
    pub fn get() -> anyhow::Result<MutexGuard<'static, IMEState>> {
        Ok(IME_STATE.lock().unwrap_or_else(|poisoned| {
            tracing::error!("IME state mutex was poisoned; recovering state");
            poisoned.into_inner()
        }))
    }

    pub fn ipc_service() -> anyhow::Result<Option<IPCService>> {
        Ok(Self::get()?.ipc_service.clone())
    }

    pub fn set_ipc_service(ipc_service: IPCService) -> anyhow::Result<()> {
        Self::get()?.ipc_service = Some(ipc_service);
        Ok(())
    }

    pub fn ensure_ipc_service() -> anyhow::Result<bool> {
        if Self::ipc_service()?.is_some() {
            return Ok(false);
        }

        let mut ipc_service = IPCService::new()?;
        ipc_service.append_text(String::new())?;
        Self::set_ipc_service(ipc_service)?;

        Ok(true)
    }

    pub fn input_mode() -> anyhow::Result<InputMode> {
        Ok(Self::get()?.input_mode.clone())
    }

    pub fn set_input_mode(input_mode: InputMode) -> anyhow::Result<()> {
        Self::get()?.input_mode = input_mode;
        Ok(())
    }

    pub fn keyboard_disabled() -> anyhow::Result<bool> {
        Ok(Self::get()?.keyboard_disabled)
    }

    pub fn set_keyboard_disabled_and_clone_ipc(
        disabled: bool,
    ) -> anyhow::Result<(bool, Option<IPCService>)> {
        let mut state = Self::get()?;
        let changed = state.keyboard_disabled != disabled;
        state.keyboard_disabled = disabled;
        let ipc_service = if disabled {
            state.ipc_service.clone()
        } else {
            None
        };

        Ok((changed, ipc_service))
    }

    pub(super) fn app_config_snapshot() -> anyhow::Result<AppConfigSnapshot> {
        let (cache_key, inspect_error) = match AppConfigCacheKey::current() {
            Ok(cache_key) => (cache_key, None),
            Err(error) => {
                let reason = error.to_string();
                (
                    AppConfigCacheKey::Unavailable {
                        reason: reason.clone(),
                    },
                    Some(reason),
                )
            }
        };

        let mut state = Self::get()?;
        if let Some(snapshot) = &state.app_config_snapshot {
            if snapshot.cache_key == cache_key {
                return Ok(snapshot.clone());
            }
        }
        if let Some(error) = inspect_error {
            tracing::error!("Failed to inspect settings; using cached/default config: {error}");
        }
        match AppConfig::read() {
            Ok(app_config) => {
                let snapshot = AppConfigSnapshot::new(app_config, cache_key);
                state.app_config_snapshot = Some(snapshot.clone());
                Ok(snapshot)
            }
            Err(shared::ConfigError::Read { path, source }) => {
                tracing::error!(
                    "Failed to load settings; using cached/default config without updating cache: failed to read config {}: {}",
                    path.display(),
                    source
                );
                if let Some(snapshot) = &state.app_config_snapshot {
                    return Ok(snapshot.clone());
                }

                Ok(AppConfigSnapshot::new(
                    AppConfig::default(),
                    AppConfigCacheKey::Unavailable {
                        reason: format!("read failed: {}: {}", path.display(), source),
                    },
                ))
            }
            Err(error) => {
                tracing::error!("Failed to load settings; using defaults: {error}");
                let snapshot = AppConfigSnapshot::new(AppConfig::default(), cache_key);
                state.app_config_snapshot = Some(snapshot.clone());
                Ok(snapshot)
            }
        }
    }
}

pub fn keyboard_disabled_from_context(context: &ITfContext) -> bool {
    unsafe {
        let Ok(compartment_mgr) = context.cast::<ITfCompartmentMgr>() else {
            return false;
        };
        let Ok(compartment) = compartment_mgr.GetCompartment(&GUID_COMPARTMENT_KEYBOARD_DISABLED)
        else {
            return false;
        };
        let Ok(value) = compartment.GetValue() else {
            return false;
        };

        i32::try_from(&value)
            .map(|value| value != 0)
            .unwrap_or(false)
    }
}
