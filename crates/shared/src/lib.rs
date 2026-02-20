use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::PathBuf};

pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/azookey.rs"));
    include!(concat!(env!("OUT_DIR"), "/window.rs"));
    pub const FILE_DESCRIPTOR_SET: &[u8] =
        tonic::include_file_descriptor_set!("azookey_service_descriptor");
}

fn get_config_root() -> PathBuf {
    let appdata = PathBuf::from(std::env::var("APPDATA").unwrap());
    appdata.join("Azookey")
}

const SETTINGS_FILENAME: &str = "settings.json";

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
    ("#", true),
    ("$", true),
    ("%", true),
    ("&", true),
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
    ("@", true),
    ("[", true),
    ("\\", true),
    ("]", true),
    ("^", true),
    ("_", true),
    ("`", true),
    ("{", true),
    ("|", true),
    ("}", true),
    ("~", true),
];

pub fn default_symbol_fullwidth_map() -> HashMap<String, bool> {
    CHARACTER_WIDTH_SYMBOL_DEFAULTS
        .into_iter()
        .map(|(symbol, is_fullwidth)| (symbol.to_string(), is_fullwidth))
        .collect()
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
pub struct CharacterWidthConfig {
    #[serde(default = "default_symbol_fullwidth_map")]
    pub symbol_fullwidth: HashMap<String, bool>,
}

impl Default for CharacterWidthConfig {
    fn default() -> Self {
        Self {
            symbol_fullwidth: default_symbol_fullwidth_map(),
        }
    }
}

fn default_shortcut_enabled() -> bool {
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
    pub zenzai: ZenzaiConfig,
    #[serde(default)]
    pub shortcuts: ShortcutConfig,
    #[serde(default)]
    pub character_width: CharacterWidthConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            version: "0.1.0".to_string(),
            zenzai: ZenzaiConfig {
                enable: false,
                profile: "".to_string(),
                backend: "cpu".to_string(),
            },
            shortcuts: ShortcutConfig::default(),
            character_width: CharacterWidthConfig::default(),
        }
    }
}

impl AppConfig {
    pub fn write(&self) {
        let config_path = get_config_root().join(SETTINGS_FILENAME);
        let config_str = serde_json::to_string_pretty(self).unwrap();
        std::fs::write(config_path, config_str).unwrap();
    }

    pub fn read() -> Self {
        let config_path = get_config_root().join(SETTINGS_FILENAME);
        if !config_path.exists() {
            return AppConfig::default();
        }
        let config_str = std::fs::read_to_string(config_path).unwrap();
        serde_json::from_str(&config_str).unwrap()
    }

    pub fn new() -> Self {
        let config_path = get_config_root();
        if !config_path.exists() {
            std::fs::create_dir_all(&config_path).unwrap();
        }
        let config = AppConfig::read();
        config.write();
        config
    }
}
