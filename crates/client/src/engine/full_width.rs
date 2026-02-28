// https://www.unicode.org/charts/nameslist/n_FF00.html
// extracted with scripts/extract_fullwidth.py

use std::{collections::HashMap, sync::LazyLock};

use shared::{
    CharacterWidthConfig, CharacterWidthGroups, GeneralConfig, PunctuationStyle, RomajiRule,
    SymbolStyle, WidthMode, CHARACTER_WIDTH_SYMBOL_DEFAULTS,
};

// in azookey, fullwidth alphabet will not be processed
static HALF_FULL_AZOOKEY: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    HashMap::from([
        ("!", "！"),
        ("\"", "”"),
        ("#", "＃"),
        ("$", "＄"),
        ("%", "％"),
        ("&", "＆"),
        ("'", "’"),
        ("(", "（"),
        (")", "）"),
        ("*", "＊"),
        ("+", "＋"),
        (",", "、"),
        ("-", "ー"),
        (".", "。"),
        ("/", "・"),
        // ("0", "０"),
        // ("1", "１"),
        // ("2", "２"),
        // ("3", "３"),
        // ("4", "４"),
        // ("5", "５"),
        // ("6", "６"),
        // ("7", "７"),
        // ("8", "８"),
        // ("9", "９"),
        (":", "："),
        (";", "；"),
        ("<", "＜"),
        ("=", "＝"),
        (">", "＞"),
        ("?", "？"),
        ("@", "＠"),
        // ("A", "Ａ"),
        // ("B", "Ｂ"),
        // ("C", "Ｃ"),
        // ("D", "Ｄ"),
        // ("E", "Ｅ"),
        // ("F", "Ｆ"),
        // ("G", "Ｇ"),
        // ("H", "Ｈ"),
        // ("I", "Ｉ"),
        // ("J", "Ｊ"),
        // ("K", "Ｋ"),
        // ("L", "Ｌ"),
        // ("M", "Ｍ"),
        // ("N", "Ｎ"),
        // ("O", "Ｏ"),
        // ("P", "Ｐ"),
        // ("Q", "Ｑ"),
        // ("R", "Ｒ"),
        // ("S", "Ｓ"),
        // ("T", "Ｔ"),
        // ("U", "Ｕ"),
        // ("V", "Ｖ"),
        // ("W", "Ｗ"),
        // ("X", "Ｘ"),
        // ("Y", "Ｙ"),
        // ("Z", "Ｚ"),
        ("[", "「"),
        ("\\", "￥"),
        ("]", "」"),
        ("^", "＾"),
        ("_", "＿"),
        ("`", "｀"),
        // ("a", "ａ"),
        // ("b", "ｂ"),
        // ("c", "ｃ"),
        // ("d", "ｄ"),
        // ("e", "ｅ"),
        // ("f", "ｆ"),
        // ("g", "ｇ"),
        // ("h", "ｈ"),
        // ("i", "ｉ"),
        // ("j", "ｊ"),
        // ("k", "ｋ"),
        // ("l", "ｌ"),
        // ("m", "ｍ"),
        // ("n", "ｎ"),
        // ("o", "ｏ"),
        // ("p", "ｐ"),
        // ("q", "ｑ"),
        // ("r", "ｒ"),
        // ("s", "ｓ"),
        // ("t", "ｔ"),
        // ("u", "ｕ"),
        // ("v", "ｖ"),
        // ("w", "ｗ"),
        // ("x", "ｘ"),
        // ("y", "ｙ"),
        // ("z", "ｚ"),
        ("{", "｛"),
        ("|", "｜"),
        ("}", "｝"),
        ("~", "～"),
    ])
});

static HALF_FULL_CONFIGURABLE: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    let mut map = HALF_FULL_AZOOKEY.clone();
    map.extend([
        ("0", "０"),
        ("1", "１"),
        ("2", "２"),
        ("3", "３"),
        ("4", "４"),
        ("5", "５"),
        ("6", "６"),
        ("7", "７"),
        ("8", "８"),
        ("9", "９"),
    ]);
    map
});

static SYMBOL_FULLWIDTH_DEFAULTS: LazyLock<HashMap<&'static str, bool>> =
    LazyLock::new(|| HashMap::from(CHARACTER_WIDTH_SYMBOL_DEFAULTS));

static HALF_FULL: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    HashMap::from([
        ("a", "ａ"),
        ("b", "ｂ"),
        ("c", "ｃ"),
        ("d", "ｄ"),
        ("e", "ｅ"),
        ("f", "ｆ"),
        ("g", "ｇ"),
        ("h", "ｈ"),
        ("i", "ｉ"),
        ("j", "ｊ"),
        ("k", "ｋ"),
        ("l", "ｌ"),
        ("m", "ｍ"),
        ("n", "ｎ"),
        ("o", "ｏ"),
        ("p", "ｐ"),
        ("q", "ｑ"),
        ("r", "ｒ"),
        ("s", "ｓ"),
        ("t", "ｔ"),
        ("u", "ｕ"),
        ("v", "ｖ"),
        ("w", "ｗ"),
        ("x", "ｘ"),
        ("y", "ｙ"),
        ("z", "ｚ"),
    ])
});

const ROMAJI_PRIORITY_KEYS: [&str; 32] = [
    "!", "\"", "#", "$", "%", "&", "'", "(", ")", "*", "+", ",", "-", ".", "/", ":", ";", "<",
    "=", ">", "?", "@", "[", "\\", "]", "^", "_", "`", "{", "|", "}", "~",
];

pub fn to_halfwidth(s: &str) -> String {
    s.chars()
        .map(|c| {
            let key = c.to_string();
            if let Some((&k, _)) = HALF_FULL_AZOOKEY.iter().find(|(_, &v)| v == key) {
                k.to_string()
            } else {
                c.to_string()
            }
        })
        .collect()
}

pub fn to_fullwidth(s: &str, process_alphabet: bool) -> String {
    s.chars()
        .map(|c| {
            let key = c.to_string();

            if process_alphabet {
                if let Some(&v) = HALF_FULL.get(key.as_str()) {
                    return v.to_string();
                }
            }

            if let Some(&v) = HALF_FULL_AZOOKEY.get(key.as_str()) {
                v.to_string()
            } else {
                c.to_string()
            }
        })
        .collect()
}

pub fn to_fullwidth_with_config(
    s: &str,
    process_alphabet: bool,
    symbol_fullwidth: &HashMap<String, bool>,
) -> String {
    s.chars()
        .map(|c| {
            let key = c.to_string();

            if process_alphabet {
                if let Some(&v) = HALF_FULL.get(key.as_str()) {
                    return v.to_string();
                }
            }

            if let Some(&v) = HALF_FULL_CONFIGURABLE.get(key.as_str()) {
                let is_fullwidth = symbol_fullwidth
                    .get(key.as_str())
                    .copied()
                    .or_else(|| SYMBOL_FULLWIDTH_DEFAULTS.get(key.as_str()).copied())
                    .unwrap_or(false);
                if is_fullwidth {
                    return v.to_string();
                }
            }

            c.to_string()
        })
        .collect()
}

pub fn convert_kana_symbol(
    s: &str,
    general: &GeneralConfig,
    character_width: &CharacterWidthConfig,
    romaji_rows: &[RomajiRule],
) -> String {
    let groups = &character_width.groups;

    s.chars()
        .map(|c| {
            let key = normalize_input_key(c);

            if let Some(romaji_output) = find_romaji_priority_output(&key, romaji_rows) {
                return romaji_output;
            }

            let base = apply_basic_setting(&key, general)
                .map(str::to_string)
                .unwrap_or_else(|| legacy_fullwidth_or_half(&key, &character_width.symbol_fullwidth));

            apply_width_groups_with_source_key(&base, &key, groups)
        })
        .collect::<Vec<_>>()
        .join("")
}

fn normalize_input_key(c: char) -> String {
    match c {
        'ˆ' | '＾' => "^".to_string(),
        '〜' | '～' => "~".to_string(),
        '＼' | '￥' | '¥' => "\\".to_string(),
        '，' => ",".to_string(),
        '．' => ".".to_string(),
        '”' => "\"".to_string(),
        '’' => "'".to_string(),
        _ => c.to_string(),
    }
}

fn find_romaji_priority_output(key: &str, romaji_rows: &[RomajiRule]) -> Option<String> {
    if !ROMAJI_PRIORITY_KEYS.contains(&key) {
        return None;
    }

    romaji_rows
        .iter()
        .find(|row| row.input == key && row.input.chars().count() == 1 && !row.output.is_empty())
        .map(|row| row.output.clone())
}

fn apply_basic_setting(key: &str, general: &GeneralConfig) -> Option<&'static str> {
    match key {
        "," => Some(match general.punctuation_style {
            PunctuationStyle::ToutenKuten | PunctuationStyle::ToutenFullwidthPeriod => "、",
            PunctuationStyle::FullwidthCommaFullwidthPeriod
            | PunctuationStyle::FullwidthCommaKuten => "，",
        }),
        "." => Some(match general.punctuation_style {
            PunctuationStyle::ToutenKuten | PunctuationStyle::FullwidthCommaKuten => "。",
            PunctuationStyle::FullwidthCommaFullwidthPeriod
            | PunctuationStyle::ToutenFullwidthPeriod => "．",
        }),
        "[" => Some(match general.symbol_style {
            SymbolStyle::CornerBracketMiddleDot | SymbolStyle::CornerBracketBackslash => "「",
            SymbolStyle::SquareBracketBackslash | SymbolStyle::SquareBracketMiddleDot => "［",
        }),
        "]" => Some(match general.symbol_style {
            SymbolStyle::CornerBracketMiddleDot | SymbolStyle::CornerBracketBackslash => "」",
            SymbolStyle::SquareBracketBackslash | SymbolStyle::SquareBracketMiddleDot => "］",
        }),
        "/" => Some(match general.symbol_style {
            SymbolStyle::CornerBracketMiddleDot | SymbolStyle::SquareBracketMiddleDot => "・",
            SymbolStyle::SquareBracketBackslash | SymbolStyle::CornerBracketBackslash => "／",
        }),
        _ => None,
    }
}

fn legacy_fullwidth_or_half(key: &str, symbol_fullwidth: &HashMap<String, bool>) -> String {
    if let Some(&fullwidth) = HALF_FULL_CONFIGURABLE.get(key) {
        let is_fullwidth = symbol_fullwidth
            .get(key)
            .copied()
            .or_else(|| SYMBOL_FULLWIDTH_DEFAULTS.get(key).copied())
            .unwrap_or(false);
        if is_fullwidth {
            return fullwidth.to_string();
        }
    }

    key.to_string()
}

fn apply_width_groups(text: &str, groups: &CharacterWidthGroups) -> String {
    text.chars().map(|c| apply_width_group_char(c, groups)).collect()
}

fn apply_width_groups_with_source_key(
    text: &str,
    source_key: &str,
    groups: &CharacterWidthGroups,
) -> String {
    if source_key == "/" {
        return text
            .chars()
            .map(|c| match c {
                '・' | '･' => match groups.math_symbol {
                    WidthMode::Half => '･',
                    WidthMode::Full => '・',
                },
                '/' | '／' => match groups.math_symbol {
                    WidthMode::Half => '/',
                    WidthMode::Full => '／',
                },
                _ => apply_width_group_char(c, groups),
            })
            .collect();
    }

    apply_width_groups(text, groups)
}

fn apply_width_group_char(c: char, groups: &CharacterWidthGroups) -> char {
    match c {
        '0' | '０' => toggle_with_mode(c, groups.number, '0', '０'),
        '1' | '１' => toggle_with_mode(c, groups.number, '1', '１'),
        '2' | '２' => toggle_with_mode(c, groups.number, '2', '２'),
        '3' | '３' => toggle_with_mode(c, groups.number, '3', '３'),
        '4' | '４' => toggle_with_mode(c, groups.number, '4', '４'),
        '5' | '５' => toggle_with_mode(c, groups.number, '5', '５'),
        '6' | '６' => toggle_with_mode(c, groups.number, '6', '６'),
        '7' | '７' => toggle_with_mode(c, groups.number, '7', '７'),
        '8' | '８' => toggle_with_mode(c, groups.number, '8', '８'),
        '9' | '９' => toggle_with_mode(c, groups.number, '9', '９'),

        '(' | '（' => toggle_with_mode(c, groups.bracket, '(', '（'),
        ')' | '）' => toggle_with_mode(c, groups.bracket, ')', '）'),
        '{' | '｛' => toggle_with_mode(c, groups.bracket, '{', '｛'),
        '}' | '｝' => toggle_with_mode(c, groups.bracket, '}', '｝'),
        '[' | '［' => toggle_with_mode(c, groups.bracket, '[', '［'),
        ']' | '］' => toggle_with_mode(c, groups.bracket, ']', '］'),

        ',' | '、' | '､' => match groups.comma_period {
            WidthMode::Half => '､',
            WidthMode::Full => '、',
        },
        '，' => match groups.comma_period {
            WidthMode::Half => ',',
            WidthMode::Full => '，',
        },
        '.' | '。' | '｡' => match groups.comma_period {
            WidthMode::Half => '｡',
            WidthMode::Full => '。',
        },
        '．' => match groups.comma_period {
            WidthMode::Half => '.',
            WidthMode::Full => '．',
        },

        '･' | '・' => toggle_with_mode(c, groups.middle_dot_corner_bracket, '･', '・'),
        '｢' | '「' => toggle_with_mode(c, groups.middle_dot_corner_bracket, '｢', '「'),
        '｣' | '」' => toggle_with_mode(c, groups.middle_dot_corner_bracket, '｣', '」'),

        '"' | '”' => toggle_with_mode(c, groups.quote, '"', '”'),
        '\'' | '’' => toggle_with_mode(c, groups.quote, '\'', '’'),

        ':' | '：' => toggle_with_mode(c, groups.colon_semicolon, ':', '：'),
        ';' | '；' => toggle_with_mode(c, groups.colon_semicolon, ';', '；'),

        '#' | '＃' => toggle_with_mode(c, groups.hash_group, '#', '＃'),
        '$' | '＄' => toggle_with_mode(c, groups.hash_group, '$', '＄'),
        '%' | '％' => toggle_with_mode(c, groups.hash_group, '%', '％'),
        '&' | '＆' => toggle_with_mode(c, groups.hash_group, '&', '＆'),
        '@' | '＠' => toggle_with_mode(c, groups.hash_group, '@', '＠'),
        '^' | '＾' | 'ˆ' => toggle_with_mode(c, groups.hash_group, '^', '＾'),
        '_' | '＿' => toggle_with_mode(c, groups.hash_group, '_', '＿'),
        '|' | '｜' => toggle_with_mode(c, groups.hash_group, '|', '｜'),
        '`' | '｀' => toggle_with_mode(c, groups.hash_group, '`', '｀'),
        '\\' | '￥' | '＼' | '¥' => match groups.hash_group {
            WidthMode::Half => '\\',
            WidthMode::Full => '＼',
        },

        '~' | '～' | '〜' => match groups.tilde {
            WidthMode::Half => '~',
            WidthMode::Full => match c {
                '〜' => '〜',
                _ => '～',
            },
        },

        '<' | '＜' => toggle_with_mode(c, groups.math_symbol, '<', '＜'),
        '>' | '＞' => toggle_with_mode(c, groups.math_symbol, '>', '＞'),
        '=' | '＝' => toggle_with_mode(c, groups.math_symbol, '=', '＝'),
        '+' | '＋' => toggle_with_mode(c, groups.math_symbol, '+', '＋'),
        '-' | 'ー' | '－' => match groups.math_symbol {
            WidthMode::Half => '-',
            WidthMode::Full => match c {
                '－' => '－',
                _ => 'ー',
            },
        },
        '/' | '／' => toggle_with_mode(c, groups.math_symbol, '/', '／'),
        '*' | '＊' => toggle_with_mode(c, groups.math_symbol, '*', '＊'),

        '?' | '？' => toggle_with_mode(c, groups.question_exclamation, '?', '？'),
        '!' | '！' => toggle_with_mode(c, groups.question_exclamation, '!', '！'),

        _ => c,
    }
}

fn toggle_with_mode(current: char, mode: WidthMode, half: char, full: char) -> char {
    match mode {
        WidthMode::Half => half,
        WidthMode::Full => {
            if current == half || current == full {
                full
            } else {
                current
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shared::{
        CharacterWidthConfig, CharacterWidthGroups, GeneralConfig, PunctuationStyle, SymbolStyle,
        WidthMode,
    };

    fn default_character_width() -> CharacterWidthConfig {
        CharacterWidthConfig {
            symbol_fullwidth: shared::default_symbol_fullwidth_map(),
            groups: CharacterWidthGroups::default(),
        }
    }

    #[test]
    fn romaji_rule_has_highest_precedence() {
        let mut config = default_character_width();
        config.groups.question_exclamation = WidthMode::Half;

        let general = GeneralConfig::default();
        let rows = vec![RomajiRule {
            input: "?".to_string(),
            output: "？".to_string(),
            next_input: String::new(),
        }];

        let output = convert_kana_symbol("?", &general, &config, &rows);
        assert_eq!(output, "？");
    }

    #[test]
    fn basic_setting_applies_before_width_groups() {
        let mut general = GeneralConfig::default();
        general.punctuation_style = PunctuationStyle::FullwidthCommaFullwidthPeriod;

        let mut config = default_character_width();
        config.groups.comma_period = WidthMode::Full;

        let output = convert_kana_symbol(",.", &general, &config, &[]);
        assert_eq!(output, "，．");
    }

    #[test]
    fn width_group_can_force_halfwidth_japanese_punctuation() {
        let mut config = default_character_width();
        config.groups.comma_period = WidthMode::Half;

        let output = convert_kana_symbol(",.", &GeneralConfig::default(), &config, &[]);
        assert_eq!(output, "､｡");
    }

    #[test]
    fn punctuation_half_mode_keeps_style_specific_ascii_forms() {
        let mut config = default_character_width();
        config.groups.comma_period = WidthMode::Half;

        let mut general = GeneralConfig::default();
        general.punctuation_style = PunctuationStyle::FullwidthCommaFullwidthPeriod;
        assert_eq!(convert_kana_symbol(",.", &general, &config, &[]), ",.");

        general.punctuation_style = PunctuationStyle::ToutenFullwidthPeriod;
        assert_eq!(convert_kana_symbol(",.", &general, &config, &[]), "､.");

        general.punctuation_style = PunctuationStyle::FullwidthCommaKuten;
        assert_eq!(convert_kana_symbol(",.", &general, &config, &[]), ",｡");
    }

    #[test]
    fn symbol_style_switches_brackets_and_middle_dot() {
        let mut general = GeneralConfig::default();
        general.symbol_style = SymbolStyle::SquareBracketBackslash;

        let output = convert_kana_symbol("[]\\", &general, &default_character_width(), &[]);
        assert_eq!(output, "［］\\");
    }

    #[test]
    fn slash_style_uses_fullwidth_solidus() {
        let mut general = GeneralConfig::default();
        general.symbol_style = SymbolStyle::SquareBracketBackslash;

        let output = convert_kana_symbol("/", &general, &default_character_width(), &[]);
        assert_eq!(output, "／");
    }

    #[test]
    fn slash_to_middle_dot_follows_math_symbol_width_group() {
        let mut general = GeneralConfig::default();
        general.symbol_style = SymbolStyle::CornerBracketMiddleDot;

        let mut config = default_character_width();
        config.groups.middle_dot_corner_bracket = WidthMode::Half;
        config.groups.math_symbol = WidthMode::Full;

        let output = convert_kana_symbol("/", &general, &config, &[]);
        assert_eq!(output, "・");
    }

    #[test]
    fn romaji_rule_bypasses_width_settings() {
        let mut config = default_character_width();
        config.groups.tilde = WidthMode::Half;

        let rows = vec![RomajiRule {
            input: "~".to_string(),
            output: "〜".to_string(),
            next_input: String::new(),
        }];

        assert_eq!(
            convert_kana_symbol("~", &GeneralConfig::default(), &config, &rows),
            "〜"
        );
    }

    #[test]
    fn backslash_is_not_forced_to_middle_dot_by_basic_setting() {
        let mut general = GeneralConfig::default();
        general.symbol_style = SymbolStyle::CornerBracketMiddleDot;

        let mut config = default_character_width();
        config.groups.hash_group = WidthMode::Half;

        assert_eq!(convert_kana_symbol("\\", &general, &config, &[]), "\\");
    }

    #[test]
    fn circumflex_variant_is_normalized() {
        let mut config = default_character_width();
        config.groups.hash_group = WidthMode::Full;

        assert_eq!(convert_kana_symbol("ˆ", &GeneralConfig::default(), &config, &[]), "＾");
    }
}
