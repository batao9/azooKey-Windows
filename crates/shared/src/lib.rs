use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    env, error, fmt, fs, io,
    path::{Path, PathBuf},
};

pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/azookey.rs"));
    include!(concat!(env!("OUT_DIR"), "/window.rs"));
    pub const FILE_DESCRIPTOR_SET: &[u8] =
        tonic::include_file_descriptor_set!("azookey_service_descriptor");
}

fn get_config_root() -> Result<PathBuf, ConfigError> {
    let appdata = env::var_os("APPDATA").ok_or(ConfigError::MissingAppData)?;
    Ok(PathBuf::from(appdata).join("Azookey"))
}

const SETTINGS_FILENAME: &str = "settings.json";
const CONFIG_VERSION: &str = "0.1.2";

#[derive(Debug)]
pub enum ConfigError {
    MissingAppData,
    CreateDir {
        path: PathBuf,
        source: io::Error,
    },
    Read {
        path: PathBuf,
        source: io::Error,
    },
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
    Backup {
        from: PathBuf,
        to: PathBuf,
        source: io::Error,
    },
    Serialize {
        source: serde_json::Error,
    },
    WriteTemp {
        path: PathBuf,
        source: io::Error,
    },
    Persist {
        from: PathBuf,
        to: PathBuf,
        source: io::Error,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::MissingAppData => write!(f, "APPDATA is not set"),
            ConfigError::CreateDir { path, source } => {
                write!(
                    f,
                    "failed to create config directory {}: {}",
                    path.display(),
                    source
                )
            }
            ConfigError::Read { path, source } => {
                write!(f, "failed to read config {}: {}", path.display(), source)
            }
            ConfigError::Parse { path, source } => {
                write!(f, "failed to parse config {}: {}", path.display(), source)
            }
            ConfigError::Backup { from, to, source } => write!(
                f,
                "failed to back up corrupted config {} to {}: {}",
                from.display(),
                to.display(),
                source
            ),
            ConfigError::Serialize { source } => {
                write!(f, "failed to serialize config: {}", source)
            }
            ConfigError::WriteTemp { path, source } => {
                write!(
                    f,
                    "failed to write temporary config {}: {}",
                    path.display(),
                    source
                )
            }
            ConfigError::Persist { from, to, source } => write!(
                f,
                "failed to replace config {} with {}: {}",
                to.display(),
                from.display(),
                source
            ),
        }
    }
}

impl error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            ConfigError::MissingAppData => None,
            ConfigError::CreateDir { source, .. }
            | ConfigError::Read { source, .. }
            | ConfigError::Backup { source, .. }
            | ConfigError::WriteTemp { source, .. }
            | ConfigError::Persist { source, .. } => Some(source),
            ConfigError::Parse { source, .. } | ConfigError::Serialize { source } => Some(source),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigRecovery {
    pub original_path: PathBuf,
    pub backup_path: PathBuf,
}

#[derive(Debug)]
pub struct AppConfigLoadResult {
    pub config: AppConfig,
    pub recovery: Option<ConfigRecovery>,
    pub rewrite_error: Option<ConfigError>,
}

pub const CHARACTER_WIDTH_SYMBOL_DEFAULTS: [(&str, bool); 42] = [
    ("0", false),
    ("1", false),
    ("2", false),
    ("3", false),
    ("4", false),
    ("5", false),
    ("6", false),
    ("7", false),
    ("8", false),
    ("9", false),
    ("!", true),
    ("\"", true),
    ("#", false),
    ("$", false),
    ("%", false),
    ("&", false),
    ("'", true),
    ("(", true),
    (")", true),
    ("*", true),
    ("+", true),
    (",", true),
    ("-", true),
    (".", true),
    ("/", true),
    (":", true),
    (";", true),
    ("<", true),
    ("=", true),
    (">", true),
    ("?", true),
    ("@", false),
    ("[", true),
    ("\\", false),
    ("]", true),
    ("^", false),
    ("_", false),
    ("`", false),
    ("{", true),
    ("|", false),
    ("}", true),
    ("~", true),
];

pub fn default_symbol_fullwidth_map() -> HashMap<String, bool> {
    CHARACTER_WIDTH_SYMBOL_DEFAULTS
        .into_iter()
        .map(|(symbol, is_fullwidth)| (symbol.to_string(), is_fullwidth))
        .collect()
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WidthMode {
    Half,
    Full,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PunctuationStyle {
    ToutenKuten,
    FullwidthCommaFullwidthPeriod,
    ToutenFullwidthPeriod,
    FullwidthCommaKuten,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SymbolStyle {
    CornerBracketMiddleDot,
    SquareBracketBackslash,
    CornerBracketBackslash,
    SquareBracketMiddleDot,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SpaceInputMode {
    AlwaysHalf,
    #[serde(alias = "always_full")]
    FollowInputMode,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NumpadInputMode {
    DirectInput,
    AlwaysHalf,
    FollowInputMode,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
pub struct CharacterWidthGroups {
    pub alphabet: WidthMode,
    pub number: WidthMode,
    pub bracket: WidthMode,
    pub comma_period: WidthMode,
    pub middle_dot_corner_bracket: WidthMode,
    pub quote: WidthMode,
    pub colon_semicolon: WidthMode,
    pub hash_group: WidthMode,
    pub tilde: WidthMode,
    pub math_symbol: WidthMode,
    pub question_exclamation: WidthMode,
}

impl Default for CharacterWidthGroups {
    fn default() -> Self {
        Self {
            alphabet: WidthMode::Half,
            number: WidthMode::Half,
            bracket: WidthMode::Full,
            comma_period: WidthMode::Full,
            middle_dot_corner_bracket: WidthMode::Full,
            quote: WidthMode::Full,
            colon_semicolon: WidthMode::Full,
            hash_group: WidthMode::Half,
            tilde: WidthMode::Full,
            math_symbol: WidthMode::Full,
            question_exclamation: WidthMode::Full,
        }
    }
}

fn group_mode_from_legacy(
    symbol_fullwidth: &HashMap<String, bool>,
    keys: &[&str],
    fallback: WidthMode,
) -> WidthMode {
    let mut full = 0;
    let mut half = 0;

    for key in keys {
        if let Some(value) = symbol_fullwidth.get(*key) {
            if *value {
                full += 1;
            } else {
                half += 1;
            }
        }
    }

    if full == 0 && half == 0 {
        fallback
    } else if full >= half {
        WidthMode::Full
    } else {
        WidthMode::Half
    }
}

fn legacy_groups_from_symbol_fullwidth(
    symbol_fullwidth: &HashMap<String, bool>,
) -> CharacterWidthGroups {
    let defaults = CharacterWidthGroups::default();
    CharacterWidthGroups {
        alphabet: defaults.alphabet,
        number: group_mode_from_legacy(
            symbol_fullwidth,
            &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"],
            defaults.number,
        ),
        bracket: group_mode_from_legacy(
            symbol_fullwidth,
            &["(", ")", "{", "}", "[", "]"],
            defaults.bracket,
        ),
        comma_period: group_mode_from_legacy(symbol_fullwidth, &[",", "."], defaults.comma_period),
        middle_dot_corner_bracket: group_mode_from_legacy(
            symbol_fullwidth,
            &["/", "[", "]"],
            defaults.middle_dot_corner_bracket,
        ),
        quote: group_mode_from_legacy(symbol_fullwidth, &["\"", "'"], defaults.quote),
        colon_semicolon: group_mode_from_legacy(
            symbol_fullwidth,
            &[":", ";"],
            defaults.colon_semicolon,
        ),
        hash_group: group_mode_from_legacy(
            symbol_fullwidth,
            &["#", "%", "&", "@", "$", "^", "_", "|", "`", "\\"],
            defaults.hash_group,
        ),
        tilde: group_mode_from_legacy(symbol_fullwidth, &["~"], defaults.tilde),
        math_symbol: group_mode_from_legacy(
            symbol_fullwidth,
            &["<", ">", "=", "+", "-", "/", "*"],
            defaults.math_symbol,
        ),
        question_exclamation: group_mode_from_legacy(
            symbol_fullwidth,
            &["?", "!"],
            defaults.question_exclamation,
        ),
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GeneralConfig {
    #[serde(default)]
    pub punctuation_style: PunctuationStyle,
    #[serde(default)]
    pub symbol_style: SymbolStyle,
    #[serde(default)]
    pub space_input: SpaceInputMode,
    #[serde(default)]
    pub numpad_input: NumpadInputMode,
    #[serde(default)]
    pub punctuation_commit: bool,
    #[serde(default = "default_punctuation_commit_target_enabled")]
    pub punctuation_commit_punctuation: bool,
    #[serde(default = "default_punctuation_commit_target_enabled")]
    pub punctuation_commit_exclamation: bool,
    #[serde(default = "default_punctuation_commit_target_enabled")]
    pub punctuation_commit_question: bool,
    #[serde(default)]
    pub show_candidate_window_after_space: bool,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            punctuation_style: PunctuationStyle::ToutenKuten,
            symbol_style: SymbolStyle::CornerBracketMiddleDot,
            space_input: SpaceInputMode::AlwaysHalf,
            numpad_input: NumpadInputMode::DirectInput,
            punctuation_commit: false,
            punctuation_commit_punctuation: true,
            punctuation_commit_exclamation: true,
            punctuation_commit_question: true,
            show_candidate_window_after_space: false,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RomajiRule {
    pub input: String,
    pub output: String,
    #[serde(default)]
    pub next_input: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct LegacyCharacterWidthConfig {
    #[serde(default = "default_symbol_fullwidth_map")]
    symbol_fullwidth: HashMap<String, bool>,
}

fn is_legacy_removed_default_row(row: &RomajiRule) -> bool {
    matches!(
        (
            row.input.as_str(),
            row.output.as_str(),
            row.next_input.as_str()
        ),
        ("~", "〜", "") | (".", "。", "") | (",", "、", "") | ("[", "「", "") | ("]", "」", "")
    )
}

fn default_romaji_rows() -> Vec<RomajiRule> {
    include_str!("default_romaji_table.txt")
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return None;
            }

            let mut parts = trimmed.split('\t');
            let input = parts.next()?.trim();
            let output = parts.next()?.trim();
            if input.is_empty() || output.is_empty() {
                return None;
            }
            let next_input = parts.next().unwrap_or_default().trim();

            Some(RomajiRule {
                input: input.to_string(),
                output: output.to_string(),
                next_input: next_input.to_string(),
            })
        })
        .collect()
}

pub fn get_default_romaji_rows() -> Vec<RomajiRule> {
    default_romaji_rows()
}

pub fn zenzai_cpu_backend_supported() -> bool {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        std::is_x86_feature_detected!("avx")
    }

    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AppConfig, ConfigError, DebugConfig, GeneralConfig, NumpadInputMode, CONFIG_VERSION,
        SETTINGS_FILENAME,
    };
    use std::{
        env,
        ffi::OsString,
        fs, io,
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

    #[test]
    fn candidate_window_delay_defaults_to_off() {
        let default_config = GeneralConfig::default();
        assert!(!default_config.show_candidate_window_after_space);

        let deserialized: GeneralConfig = serde_json::from_str("{}").unwrap();
        assert!(!deserialized.show_candidate_window_after_space);
    }

    #[test]
    fn punctuation_commit_defaults_to_off() {
        let default_config = GeneralConfig::default();
        assert!(!default_config.punctuation_commit);
        assert!(default_config.punctuation_commit_punctuation);
        assert!(default_config.punctuation_commit_exclamation);
        assert!(default_config.punctuation_commit_question);

        let deserialized: GeneralConfig = serde_json::from_str("{}").unwrap();
        assert!(!deserialized.punctuation_commit);
        assert!(deserialized.punctuation_commit_punctuation);
        assert!(deserialized.punctuation_commit_exclamation);
        assert!(deserialized.punctuation_commit_question);
    }

    #[test]
    fn debug_server_log_defaults_to_off() {
        let default_config = DebugConfig::default();
        assert!(!default_config.server_log_enabled);
        assert_eq!(default_config.server_log_level, "warn");
        assert!(default_config.server_crash_trace_enabled);

        let deserialized: DebugConfig = serde_json::from_str("{}").unwrap();
        assert!(!deserialized.server_log_enabled);
        assert_eq!(deserialized.server_log_level, "warn");
        assert!(deserialized.server_crash_trace_enabled);
    }

    #[test]
    fn new_creates_default_settings_when_file_is_missing() {
        let temp = tempfile::tempdir().unwrap();
        let _appdata = AppDataGuard::set(temp.path());

        let result = AppConfig::new_with_recovery().unwrap();
        let config_path = temp.path().join("Azookey").join(SETTINGS_FILENAME);

        assert!(result.recovery.is_none());
        assert_eq!(result.config.version, CONFIG_VERSION);
        assert!(config_path.exists());

        let saved: AppConfig = serde_json::from_str(&fs::read_to_string(config_path).unwrap())
            .expect("saved default config should be valid JSON");
        assert_eq!(saved.version, CONFIG_VERSION);
    }

    #[test]
    fn corrupted_settings_are_backed_up_and_default_settings_are_written() {
        let temp = tempfile::tempdir().unwrap();
        let _appdata = AppDataGuard::set(temp.path());
        let config_root = temp.path().join("Azookey");
        fs::create_dir_all(&config_root).unwrap();
        let config_path = config_root.join(SETTINGS_FILENAME);
        fs::write(&config_path, "{not valid json").unwrap();

        let result = AppConfig::new_with_recovery().unwrap();
        let recovery = result
            .recovery
            .expect("broken settings should produce recovery metadata");

        assert_eq!(recovery.original_path, config_path);
        assert!(recovery.backup_path.exists());
        assert!(recovery
            .backup_path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("settings.json.broken-"));
        assert_eq!(
            fs::read_to_string(recovery.backup_path).unwrap(),
            "{not valid json"
        );

        let saved: AppConfig = serde_json::from_str(&fs::read_to_string(config_path).unwrap())
            .expect("recreated settings should be valid JSON");
        assert_eq!(saved.version, CONFIG_VERSION);
    }

    #[test]
    fn read_migrates_valid_legacy_config() {
        let temp = tempfile::tempdir().unwrap();
        let _appdata = AppDataGuard::set(temp.path());
        let config_root = temp.path().join("Azookey");
        fs::create_dir_all(&config_root).unwrap();
        let config_path = config_root.join(SETTINGS_FILENAME);
        let mut legacy = AppConfig::default();
        legacy.version = "0.1.1".to_string();
        legacy.general.numpad_input = NumpadInputMode::AlwaysHalf;
        fs::write(&config_path, serde_json::to_string_pretty(&legacy).unwrap()).unwrap();

        let migrated = AppConfig::read().unwrap();

        assert_eq!(migrated.version, CONFIG_VERSION);
        assert_eq!(migrated.general.numpad_input, NumpadInputMode::DirectInput);
    }

    #[test]
    fn new_with_recovery_repairs_mojibake_default_romaji_table() {
        let temp = tempfile::tempdir().unwrap();
        let _appdata = AppDataGuard::set(temp.path());
        let config_root = temp.path().join("Azookey");
        fs::create_dir_all(&config_root).unwrap();
        let config_path = config_root.join(SETTINGS_FILENAME);
        let mut config = AppConfig::default();
        for row in &mut config.romaji_table.rows {
            row.output = "繝ｼ".to_string();
        }
        fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();

        let result = AppConfig::new_with_recovery().unwrap();

        assert_eq!(
            result
                .config
                .romaji_table
                .rows
                .iter()
                .find(|row| row.input == "-")
                .unwrap()
                .output,
            "ー"
        );
        assert_eq!(
            result
                .config
                .romaji_table
                .rows
                .iter()
                .find(|row| row.input == "a")
                .unwrap()
                .output,
            "あ"
        );

        let saved: AppConfig = serde_json::from_str(&fs::read_to_string(config_path).unwrap())
            .expect("rewritten settings should be valid JSON");
        assert_eq!(
            saved
                .romaji_table
                .rows
                .iter()
                .find(|row| row.input == "a")
                .unwrap()
                .output,
            "あ"
        );
    }

    #[test]
    fn read_keeps_custom_romaji_table_output() {
        let temp = tempfile::tempdir().unwrap();
        let _appdata = AppDataGuard::set(temp.path());
        let config_root = temp.path().join("Azookey");
        fs::create_dir_all(&config_root).unwrap();
        let config_path = config_root.join(SETTINGS_FILENAME);
        let mut config = AppConfig::default();
        config.romaji_table.rows[0].output = "custom".to_string();
        fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();

        let loaded = AppConfig::read().unwrap();

        assert_eq!(loaded.romaji_table.rows[0].output, "custom");
    }

    #[test]
    fn new_with_recovery_keeps_loaded_config_when_rewrite_fails() {
        let temp = tempfile::tempdir().unwrap();
        let config_root = temp.path().join("Azookey");
        fs::create_dir_all(&config_root).unwrap();
        let config_path = config_root.join(SETTINGS_FILENAME);
        let mut config = AppConfig::default();
        config.zenzai.enable = true;
        fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();

        let result = AppConfig::new_with_recovery_from_root(&config_root, |_| {
            Err(ConfigError::WriteTemp {
                path: config_root.join("settings.json.tmp-test"),
                source: io::Error::new(io::ErrorKind::PermissionDenied, "test write failure"),
            })
        })
        .unwrap();

        assert!(result.config.zenzai.enable);
        assert!(result.recovery.is_none());
        assert!(matches!(
            result.rewrite_error,
            Some(ConfigError::WriteTemp { .. })
        ));
    }

    #[test]
    fn write_persists_valid_json_without_temp_file_leftover() {
        let temp = tempfile::tempdir().unwrap();
        let _appdata = AppDataGuard::set(temp.path());
        let mut config = AppConfig::default();
        config.zenzai.enable = true;

        config.write().unwrap();

        let config_root = temp.path().join("Azookey");
        let saved: AppConfig =
            serde_json::from_str(&fs::read_to_string(config_root.join(SETTINGS_FILENAME)).unwrap())
                .expect("written settings should be valid JSON");
        assert!(saved.zenzai.enable);
        let temp_files = fs::read_dir(config_root)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("settings.json.tmp-")
            })
            .count();
        assert_eq!(temp_files, 0);
    }

    #[test]
    fn missing_appdata_returns_config_error() {
        let _appdata = AppDataGuard::unset();

        let error = AppConfig::read().expect_err("APPDATA absence should not panic");

        assert!(matches!(error, ConfigError::MissingAppData));
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RomajiTableConfig {
    #[serde(default = "default_romaji_rows")]
    pub rows: Vec<RomajiRule>,
}

impl Default for RomajiTableConfig {
    fn default() -> Self {
        Self {
            rows: default_romaji_rows(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ZenzaiConfig {
    pub enable: bool,
    pub profile: String,
    pub backend: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ShortcutConfig {
    #[serde(default = "default_shortcut_enabled")]
    pub ctrl_space_toggle: bool,
    #[serde(default = "default_shortcut_enabled")]
    pub alt_backquote_toggle: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DebugConfig {
    #[serde(default)]
    pub server_log_enabled: bool,
    #[serde(default = "default_server_log_level")]
    pub server_log_level: String,
    #[serde(default = "default_server_crash_trace_enabled")]
    pub server_crash_trace_enabled: bool,
}

impl Default for DebugConfig {
    fn default() -> Self {
        Self {
            server_log_enabled: false,
            server_log_level: default_server_log_level(),
            server_crash_trace_enabled: default_server_crash_trace_enabled(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CharacterWidthConfig {
    #[serde(default = "default_symbol_fullwidth_map")]
    pub symbol_fullwidth: HashMap<String, bool>,
    #[serde(default)]
    pub groups: CharacterWidthGroups,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct UserDictionaryEntry {
    pub reading: String,
    pub word: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct UserDictionaryConfig {
    #[serde(default)]
    pub entries: Vec<UserDictionaryEntry>,
}

impl Default for CharacterWidthConfig {
    fn default() -> Self {
        Self {
            symbol_fullwidth: default_symbol_fullwidth_map(),
            groups: CharacterWidthGroups::default(),
        }
    }
}

impl Default for PunctuationStyle {
    fn default() -> Self {
        Self::ToutenKuten
    }
}

impl Default for SymbolStyle {
    fn default() -> Self {
        Self::CornerBracketMiddleDot
    }
}

impl Default for SpaceInputMode {
    fn default() -> Self {
        Self::AlwaysHalf
    }
}

impl Default for NumpadInputMode {
    fn default() -> Self {
        Self::DirectInput
    }
}

fn default_shortcut_enabled() -> bool {
    true
}

fn default_server_log_level() -> String {
    "warn".to_string()
}

fn default_server_crash_trace_enabled() -> bool {
    true
}

fn default_punctuation_commit_target_enabled() -> bool {
    true
}

impl Default for ShortcutConfig {
    fn default() -> Self {
        Self {
            ctrl_space_toggle: true,
            alt_backquote_toggle: true,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AppConfig {
    pub version: String,
    #[serde(default)]
    pub debug: DebugConfig,
    pub zenzai: ZenzaiConfig,
    #[serde(default)]
    pub shortcuts: ShortcutConfig,
    #[serde(default)]
    pub general: GeneralConfig,
    #[serde(default)]
    pub romaji_table: RomajiTableConfig,
    #[serde(default)]
    pub character_width: CharacterWidthConfig,
    #[serde(default)]
    pub user_dictionary: UserDictionaryConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            version: CONFIG_VERSION.to_string(),
            debug: DebugConfig::default(),
            zenzai: ZenzaiConfig {
                enable: false,
                profile: "".to_string(),
                backend: "cpu".to_string(),
            },
            shortcuts: ShortcutConfig::default(),
            general: GeneralConfig::default(),
            romaji_table: RomajiTableConfig::default(),
            character_width: CharacterWidthConfig::default(),
            user_dictionary: UserDictionaryConfig::default(),
        }
    }
}

impl AppConfig {
    pub fn settings_path() -> Result<PathBuf, ConfigError> {
        Ok(get_config_root()?.join(SETTINGS_FILENAME))
    }

    pub fn write(&self) -> Result<(), ConfigError> {
        let config_root = get_config_root()?;
        ensure_config_dir(&config_root)?;
        let config_path = config_root.join(SETTINGS_FILENAME);
        let temp_path = temporary_config_path(&config_root);
        let config_str = serde_json::to_string_pretty(self)
            .map_err(|source| ConfigError::Serialize { source })?;

        write_temp_config(&temp_path, config_str.as_bytes())?;
        replace_config_file(&temp_path, &config_path).map_err(|source| {
            let _ = fs::remove_file(&temp_path);
            ConfigError::Persist {
                from: temp_path.clone(),
                to: config_path,
                source,
            }
        })?;

        Ok(())
    }

    pub fn read() -> Result<Self, ConfigError> {
        let config_path = get_config_root()?.join(SETTINGS_FILENAME);
        if !config_path.exists() {
            return Ok(AppConfig::default());
        }
        let config_str = fs::read_to_string(&config_path).map_err(|source| ConfigError::Read {
            path: config_path.clone(),
            source,
        })?;
        let config = parse_config(&config_path, &config_str)?;

        Ok(config)
    }

    pub fn new() -> Result<Self, ConfigError> {
        Ok(Self::new_with_recovery()?.config)
    }

    pub fn new_with_recovery() -> Result<AppConfigLoadResult, ConfigError> {
        let config_root = get_config_root()?;
        Self::new_with_recovery_from_root(&config_root, |config| config.write())
    }

    fn new_with_recovery_from_root(
        config_root: &Path,
        rewrite_config: impl FnOnce(&AppConfig) -> Result<(), ConfigError>,
    ) -> Result<AppConfigLoadResult, ConfigError> {
        ensure_config_dir(config_root)?;
        let config_path = config_root.join(SETTINGS_FILENAME);

        let (config, recovery) = if !config_path.exists() {
            (AppConfig::default(), None)
        } else {
            let config_str =
                fs::read_to_string(&config_path).map_err(|source| ConfigError::Read {
                    path: config_path.clone(),
                    source,
                })?;

            match parse_config(&config_path, &config_str) {
                Ok(config) => (config, None),
                Err(ConfigError::Parse { .. }) => {
                    let backup_path = backup_corrupted_config(&config_path)?;
                    (
                        AppConfig::default(),
                        Some(ConfigRecovery {
                            original_path: config_path.clone(),
                            backup_path,
                        }),
                    )
                }
                Err(error) => return Err(error),
            }
        };

        let rewrite_error = rewrite_config(&config).err();
        Ok(AppConfigLoadResult {
            config,
            recovery,
            rewrite_error,
        })
    }
}

fn parse_config(config_path: &Path, config_str: &str) -> Result<AppConfig, ConfigError> {
    let mut config: AppConfig =
        serde_json::from_str(config_str).map_err(|source| ConfigError::Parse {
            path: config_path.to_path_buf(),
            source,
        })?;

    if config.version != CONFIG_VERSION {
        config.general.numpad_input = match config.general.numpad_input {
            // 旧仕様との互換: always_half(直接入力) -> direct_input
            NumpadInputMode::AlwaysHalf => NumpadInputMode::DirectInput,
            // 旧仕様との互換: follow_input_mode(変換待ち半角) -> always_half
            NumpadInputMode::FollowInputMode => NumpadInputMode::AlwaysHalf,
            NumpadInputMode::DirectInput => NumpadInputMode::DirectInput,
        };

        if let Ok(value) = serde_json::from_str::<serde_json::Value>(config_str) {
            let legacy = value
                .get("character_width")
                .cloned()
                .and_then(|cw| serde_json::from_value::<LegacyCharacterWidthConfig>(cw).ok())
                .unwrap_or(LegacyCharacterWidthConfig {
                    symbol_fullwidth: config.character_width.symbol_fullwidth.clone(),
                });
            let legacy_groups = legacy_groups_from_symbol_fullwidth(&legacy.symbol_fullwidth);

            if config.character_width.groups == legacy_groups {
                config.character_width.groups = CharacterWidthGroups::default();
            }
        }

        config
            .romaji_table
            .rows
            .retain(|row| !is_legacy_removed_default_row(row));
        config.version = CONFIG_VERSION.to_string();
    }

    repair_mojibake_default_romaji_table(&mut config);

    Ok(config)
}

fn repair_mojibake_default_romaji_table(config: &mut AppConfig) {
    if romaji_table_looks_like_mojibake_default(&config.romaji_table.rows) {
        config.romaji_table.rows = default_romaji_rows();
    }
}

fn romaji_table_looks_like_mojibake_default(rows: &[RomajiRule]) -> bool {
    let default_rows = default_romaji_rows();
    if rows.len() != default_rows.len() {
        return false;
    }

    let matching_keys = rows
        .iter()
        .zip(default_rows.iter())
        .filter(|(row, default_row)| {
            row.input == default_row.input && row.next_input == default_row.next_input
        })
        .count();
    if matching_keys < default_rows.len().saturating_mul(9) / 10 {
        return false;
    }

    let differing_outputs = rows
        .iter()
        .zip(default_rows.iter())
        .filter(|(row, default_row)| row.output != default_row.output)
        .count();
    let mojibake_outputs = rows
        .iter()
        .filter(|row| contains_common_utf8_mojibake_marker(&row.output))
        .count();

    differing_outputs > default_rows.len() / 2 && mojibake_outputs > 10
}

fn contains_common_utf8_mojibake_marker(value: &str) -> bool {
    const MARKERS: [&str; 12] = [
        "繝", "縺", "繧", "窶", "竊", "縲", "荳", "譁", "ã", "Ã", "Â", "â",
    ];
    MARKERS.iter().any(|marker| value.contains(marker))
}

fn ensure_config_dir(config_root: &Path) -> Result<(), ConfigError> {
    fs::create_dir_all(config_root).map_err(|source| ConfigError::CreateDir {
        path: config_root.to_path_buf(),
        source,
    })
}

fn write_temp_config(temp_path: &Path, bytes: &[u8]) -> Result<(), ConfigError> {
    let mut file = fs::File::create(temp_path).map_err(|source| ConfigError::WriteTemp {
        path: temp_path.to_path_buf(),
        source,
    })?;
    use std::io::Write as _;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|source| ConfigError::WriteTemp {
            path: temp_path.to_path_buf(),
            source,
        })
}

fn temporary_config_path(config_root: &Path) -> PathBuf {
    let timestamp = chrono::Local::now().format("%Y%m%d%H%M%S%f");
    config_root.join(format!(
        "{SETTINGS_FILENAME}.tmp-{}-{timestamp}",
        std::process::id()
    ))
}

fn backup_corrupted_config(config_path: &Path) -> Result<PathBuf, ConfigError> {
    let base_name = format!(
        "{SETTINGS_FILENAME}.broken-{}",
        chrono::Local::now().format("%Y%m%d%H%M%S")
    );
    let parent = config_path.parent().unwrap_or_else(|| Path::new("."));

    for index in 0..1000 {
        let candidate = if index == 0 {
            parent.join(&base_name)
        } else {
            parent.join(format!("{base_name}-{index}"))
        };

        if candidate.exists() {
            continue;
        }

        fs::rename(config_path, &candidate).map_err(|source| ConfigError::Backup {
            from: config_path.to_path_buf(),
            to: candidate.clone(),
            source,
        })?;
        return Ok(candidate);
    }

    let fallback = parent.join(format!("{base_name}-overflow"));
    fs::rename(config_path, &fallback).map_err(|source| ConfigError::Backup {
        from: config_path.to_path_buf(),
        to: fallback.clone(),
        source,
    })?;
    Ok(fallback)
}

#[cfg(not(windows))]
fn replace_config_file(temp_path: &Path, config_path: &Path) -> io::Result<()> {
    fs::rename(temp_path, config_path)
}

#[cfg(windows)]
fn replace_config_file(temp_path: &Path, config_path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows::{
        core::PCWSTR,
        Win32::Storage::FileSystem::{
            MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        },
    };

    let from: Vec<u16> = temp_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let to: Vec<u16> = config_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        MoveFileExW(
            PCWSTR(from.as_ptr()),
            PCWSTR(to.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(|error| io::Error::new(io::ErrorKind::Other, error.to_string()))
}
