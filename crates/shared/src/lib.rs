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
const CONFIG_VERSION: &str = "0.1.1";

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

fn legacy_groups_from_symbol_fullwidth(symbol_fullwidth: &HashMap<String, bool>) -> CharacterWidthGroups {
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
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            punctuation_style: PunctuationStyle::ToutenKuten,
            symbol_style: SymbolStyle::CornerBracketMiddleDot,
            space_input: SpaceInputMode::AlwaysHalf,
            numpad_input: NumpadInputMode::AlwaysHalf,
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
        (row.input.as_str(), row.output.as_str(), row.next_input.as_str()),
        ("~", "〜", "")
            | (".", "。", "")
            | (",", "、", "")
            | ("[", "「", "")
            | ("]", "」", "")
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
pub struct CharacterWidthConfig {
    #[serde(default = "default_symbol_fullwidth_map")]
    pub symbol_fullwidth: HashMap<String, bool>,
    #[serde(default)]
    pub groups: CharacterWidthGroups,
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
        Self::AlwaysHalf
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
    pub general: GeneralConfig,
    #[serde(default)]
    pub romaji_table: RomajiTableConfig,
    #[serde(default)]
    pub character_width: CharacterWidthConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            version: CONFIG_VERSION.to_string(),
            zenzai: ZenzaiConfig {
                enable: false,
                profile: "".to_string(),
                backend: "cpu".to_string(),
            },
            shortcuts: ShortcutConfig::default(),
            general: GeneralConfig::default(),
            romaji_table: RomajiTableConfig::default(),
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
        let mut config: AppConfig = serde_json::from_str(&config_str).unwrap();

        if config.version != CONFIG_VERSION {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&config_str) {
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

        config
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
