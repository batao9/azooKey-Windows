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

static HALF_FULL_CONFIGURABLE: LazyLock<HashMap<&'static str, &'static str>> =
    LazyLock::new(|| {
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

static SYMBOL_FULLWIDTH_DEFAULTS_CHAR: LazyLock<HashMap<char, bool>> = LazyLock::new(|| {
    SYMBOL_FULLWIDTH_DEFAULTS
        .iter()
        .filter_map(|(key, value)| single_char(key).map(|key| (key, *value)))
        .collect()
});

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

static HALF_FULL_AZOOKEY_CHAR: LazyLock<HashMap<char, char>> =
    LazyLock::new(|| single_char_map(&HALF_FULL_AZOOKEY));

static FULL_HALF_AZOOKEY_CHAR: LazyLock<HashMap<char, char>> = LazyLock::new(|| {
    HALF_FULL_AZOOKEY_CHAR
        .iter()
        .map(|(half, full)| (*full, *half))
        .collect()
});

static HALF_FULL_CONFIGURABLE_CHAR: LazyLock<HashMap<char, char>> =
    LazyLock::new(|| single_char_map(&HALF_FULL_CONFIGURABLE));

static HALF_FULL_CHAR: LazyLock<HashMap<char, char>> =
    LazyLock::new(|| single_char_map(&HALF_FULL));

fn single_char(value: &str) -> Option<char> {
    let mut chars = value.chars();
    let ch = chars.next()?;
    chars.next().is_none().then_some(ch)
}

fn single_char_map(map: &HashMap<&'static str, &'static str>) -> HashMap<char, char> {
    map.iter()
        .filter_map(|(half, full)| Some((single_char(half)?, single_char(full)?)))
        .collect()
}

pub fn to_halfwidth(s: &str) -> String {
    s.chars()
        .map(|c| FULL_HALF_AZOOKEY_CHAR.get(&c).copied().unwrap_or(c))
        .collect()
}

pub fn to_fullwidth(s: &str, process_alphabet: bool) -> String {
    s.chars()
        .map(|c| {
            if process_alphabet {
                if let Some(&v) = HALF_FULL_CHAR.get(&c) {
                    return v;
                }
            }

            HALF_FULL_AZOOKEY_CHAR.get(&c).copied().unwrap_or(c)
        })
        .collect()
}

#[cfg(test)]
pub fn to_fullwidth_with_config(
    s: &str,
    process_alphabet: bool,
    symbol_fullwidth: &HashMap<String, bool>,
) -> String {
    s.chars()
        .map(|c| {
            if process_alphabet {
                if let Some(&v) = HALF_FULL_CHAR.get(&c) {
                    return v;
                }
            }

            if let Some(&v) = HALF_FULL_CONFIGURABLE_CHAR.get(&c) {
                if symbol_fullwidth_enabled(c, symbol_fullwidth) {
                    return v;
                }
            }

            c
        })
        .collect()
}

pub fn convert_kana_symbol(
    s: &str,
    general: &GeneralConfig,
    character_width: &CharacterWidthConfig,
    _romaji_rows: &[RomajiRule],
) -> String {
    let groups = &character_width.groups;

    s.chars()
        .map(|c| {
            let key = normalize_input_key(c);

            let base = apply_basic_setting(key, general)
                .map(str::to_string)
                .unwrap_or_else(|| {
                    legacy_fullwidth_or_half(key, &character_width.symbol_fullwidth)
                });

            apply_width_groups_with_source_key(&base, key, groups)
        })
        .collect::<Vec<_>>()
        .join("")
}

fn normalize_input_key(c: char) -> char {
    match c {
        'ˆ' | '＾' => '^',
        '〜' | '～' => '~',
        '＼' | '￥' | '¥' => '\\',
        '，' => ',',
        '．' => '.',
        '”' => '"',
        '’' => '\'',
        _ => c,
    }
}

fn apply_basic_setting(key: char, general: &GeneralConfig) -> Option<&'static str> {
    match key {
        ',' => Some(match general.punctuation_style {
            PunctuationStyle::ToutenKuten | PunctuationStyle::ToutenFullwidthPeriod => "、",
            PunctuationStyle::FullwidthCommaFullwidthPeriod
            | PunctuationStyle::FullwidthCommaKuten => "，",
        }),
        '.' => Some(match general.punctuation_style {
            PunctuationStyle::ToutenKuten | PunctuationStyle::FullwidthCommaKuten => "。",
            PunctuationStyle::FullwidthCommaFullwidthPeriod
            | PunctuationStyle::ToutenFullwidthPeriod => "．",
        }),
        '[' => Some(match general.symbol_style {
            SymbolStyle::CornerBracketMiddleDot | SymbolStyle::CornerBracketBackslash => "「",
            SymbolStyle::SquareBracketBackslash | SymbolStyle::SquareBracketMiddleDot => "［",
        }),
        ']' => Some(match general.symbol_style {
            SymbolStyle::CornerBracketMiddleDot | SymbolStyle::CornerBracketBackslash => "」",
            SymbolStyle::SquareBracketBackslash | SymbolStyle::SquareBracketMiddleDot => "］",
        }),
        '/' => Some(match general.symbol_style {
            SymbolStyle::CornerBracketMiddleDot | SymbolStyle::SquareBracketMiddleDot => "・",
            SymbolStyle::SquareBracketBackslash | SymbolStyle::CornerBracketBackslash => "／",
        }),
        _ => None,
    }
}

fn legacy_fullwidth_or_half(key: char, symbol_fullwidth: &HashMap<String, bool>) -> String {
    if let Some(&fullwidth) = HALF_FULL_CONFIGURABLE_CHAR.get(&key) {
        if symbol_fullwidth_enabled(key, symbol_fullwidth) {
            return fullwidth.to_string();
        }
    }

    key.to_string()
}

fn symbol_fullwidth_enabled(key: char, symbol_fullwidth: &HashMap<String, bool>) -> bool {
    let mut buffer = [0; 4];
    let key = key.encode_utf8(&mut buffer);
    symbol_fullwidth
        .get(key)
        .copied()
        .or_else(|| {
            single_char(key).and_then(|key| SYMBOL_FULLWIDTH_DEFAULTS_CHAR.get(&key).copied())
        })
        .unwrap_or(false)
}

fn apply_width_groups(text: &str, groups: &CharacterWidthGroups) -> String {
    text.chars()
        .map(|c| apply_width_group_char(c, groups))
        .collect()
}

fn apply_width_groups_with_source_key(
    text: &str,
    source_key: char,
    groups: &CharacterWidthGroups,
) -> String {
    if source_key == '/' {
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
    fn symbol_conversion_ignores_romaji_table_rows() {
        let mut config = default_character_width();
        config.groups.question_exclamation = WidthMode::Half;

        let general = GeneralConfig::default();
        let rows = vec![RomajiRule {
            input: "?".to_string(),
            output: "？".to_string(),
            next_input: String::new(),
        }];

        let output = convert_kana_symbol("?", &general, &config, &rows);
        assert_eq!(output, "?");
    }

    #[test]
    fn halfwidth_conversion_uses_reverse_symbol_lookup() {
        assert_eq!(to_halfwidth("！？￥「」"), r"!?\[]");
    }

    #[test]
    fn configurable_fullwidth_conversion_uses_direct_symbol_lookup() {
        let mut symbol_fullwidth = shared::default_symbol_fullwidth_map();
        symbol_fullwidth.insert("1".to_string(), true);
        symbol_fullwidth.insert("!".to_string(), true);

        assert_eq!(
            to_fullwidth_with_config("a1!", true, &symbol_fullwidth),
            "ａ１！"
        );
    }

    #[test]
    fn basic_setting_applies_before_width_groups() {
        let general = GeneralConfig {
            punctuation_style: PunctuationStyle::FullwidthCommaFullwidthPeriod,
            ..GeneralConfig::default()
        };

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

        let mut general = GeneralConfig {
            punctuation_style: PunctuationStyle::FullwidthCommaFullwidthPeriod,
            ..GeneralConfig::default()
        };
        assert_eq!(convert_kana_symbol(",.", &general, &config, &[]), ",.");

        general.punctuation_style = PunctuationStyle::ToutenFullwidthPeriod;
        assert_eq!(convert_kana_symbol(",.", &general, &config, &[]), "､.");

        general.punctuation_style = PunctuationStyle::FullwidthCommaKuten;
        assert_eq!(convert_kana_symbol(",.", &general, &config, &[]), ",｡");
    }

    #[test]
    fn symbol_style_switches_brackets_and_middle_dot() {
        let general = GeneralConfig {
            symbol_style: SymbolStyle::SquareBracketBackslash,
            ..GeneralConfig::default()
        };

        let output = convert_kana_symbol("[]\\", &general, &default_character_width(), &[]);
        assert_eq!(output, "［］\\");
    }

    #[test]
    fn slash_style_uses_fullwidth_solidus() {
        let general = GeneralConfig {
            symbol_style: SymbolStyle::SquareBracketBackslash,
            ..GeneralConfig::default()
        };

        let output = convert_kana_symbol("/", &general, &default_character_width(), &[]);
        assert_eq!(output, "／");
    }

    #[test]
    fn slash_to_middle_dot_follows_math_symbol_width_group() {
        let general = GeneralConfig::default();

        let mut config = default_character_width();
        config.groups.middle_dot_corner_bracket = WidthMode::Half;
        config.groups.math_symbol = WidthMode::Full;

        let output = convert_kana_symbol("/", &general, &config, &[]);
        assert_eq!(output, "・");
    }

    #[test]
    fn symbol_conversion_applies_width_settings_even_with_romaji_rows() {
        let mut config = default_character_width();
        config.groups.tilde = WidthMode::Half;

        let rows = vec![RomajiRule {
            input: "~".to_string(),
            output: "〜".to_string(),
            next_input: String::new(),
        }];

        assert_eq!(
            convert_kana_symbol("~", &GeneralConfig::default(), &config, &rows),
            "~"
        );
    }

    #[test]
    fn backslash_is_not_forced_to_middle_dot_by_basic_setting() {
        let general = GeneralConfig::default();

        let mut config = default_character_width();
        config.groups.hash_group = WidthMode::Half;

        assert_eq!(convert_kana_symbol("\\", &general, &config, &[]), "\\");
    }

    #[test]
    fn circumflex_variant_is_normalized() {
        let mut config = default_character_width();
        config.groups.hash_group = WidthMode::Full;

        assert_eq!(
            convert_kana_symbol("ˆ", &GeneralConfig::default(), &config, &[]),
            "＾"
        );
    }
}
