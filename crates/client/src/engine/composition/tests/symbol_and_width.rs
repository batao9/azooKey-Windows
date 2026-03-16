use super::*;

#[test]
fn symbol_fallback_is_disabled_in_romaji_context() {
    let rows = vec![row("z/", "・", "")];
    let should_apply = TextServiceFactory::should_apply_symbol_fallback("z", "/", &rows);
    assert!(!should_apply);
}

#[test]
fn symbol_fallback_is_enabled_for_standalone_symbol() {
    let rows = vec![row("z/", "・", "")];
    let should_apply = TextServiceFactory::should_apply_symbol_fallback("abc", "/", &rows);
    assert!(should_apply);
}

#[test]
fn symbol_fallback_is_disabled_for_non_symbol_input() {
    let rows = vec![row("ka", "か", "")];
    let should_apply = TextServiceFactory::should_apply_symbol_fallback("k", "a", &rows);
    assert!(!should_apply);
}

#[test]
fn symbol_fallback_is_enabled_for_non_ascii_symbol_variant() {
    let rows = vec![row("ka", "か", "")];
    let should_apply = TextServiceFactory::should_apply_symbol_fallback("", "￥", &rows);
    assert!(should_apply);
}

#[test]
fn symbol_fallback_is_disabled_for_non_ascii_symbol_in_romaji_context() {
    let rows = vec![row("n\\", "んー", "")];
    let should_apply = TextServiceFactory::should_apply_symbol_fallback("n", "￥", &rows);
    assert!(!should_apply);
}

#[test]
fn single_symbol_romaji_output_matches_exact_symbol_rule() {
    let rows = vec![row("-", "ー", "")];
    let output = TextServiceFactory::single_symbol_romaji_output("-", &rows);
    assert_eq!(output, Some("ー".to_string()));
}

#[test]
fn single_symbol_romaji_output_ignores_multi_character_rule() {
    let rows = vec![row("z/", "・", "")];
    let output = TextServiceFactory::single_symbol_romaji_output("/", &rows);
    assert_eq!(output, None);
}

#[test]
fn zenzai_symbol_input_prefers_explicit_single_symbol_rule() {
    let mut app_config = AppConfig::default();
    app_config.zenzai.enable = true;
    app_config.zenzai.backend = "vulkan".to_string();
    app_config.character_width.groups.math_symbol = WidthMode::Half;
    app_config.romaji_table.rows = vec![row("-", "ー", "")];

    let output = TextServiceFactory::resolve_symbol_input_text("", "-", &app_config);
    assert_eq!(output, Some("ー".to_string()));
}

#[test]
fn zenzai_symbol_input_falls_back_to_width_setting_without_symbol_rule() {
    let mut app_config = AppConfig::default();
    app_config.zenzai.enable = true;
    app_config.zenzai.backend = "vulkan".to_string();
    app_config.character_width.groups.math_symbol = WidthMode::Half;
    app_config.romaji_table.rows = vec![];

    let output = TextServiceFactory::resolve_symbol_input_text("", "-", &app_config);
    assert_eq!(output, Some("-".to_string()));
}

#[test]
fn zenzai_symbol_input_keeps_default_multi_character_dash_sequence() {
    let mut app_config = AppConfig::default();
    app_config.zenzai.enable = true;
    app_config.zenzai.backend = "vulkan".to_string();
    app_config.romaji_table.rows = get_default_romaji_rows();

    let output = TextServiceFactory::resolve_symbol_input_text("z", "-", &app_config);
    assert_eq!(output, None);
}

#[test]
fn zenzai_symbol_input_keeps_default_multi_character_symbol_sequence() {
    let mut app_config = AppConfig::default();
    app_config.zenzai.enable = true;
    app_config.zenzai.backend = "vulkan".to_string();
    app_config.romaji_table.rows = get_default_romaji_rows();

    let output = TextServiceFactory::resolve_symbol_input_text("z", "/", &app_config);
    assert_eq!(output, None);
}

#[test]
fn zenzai_symbol_input_keeps_default_n_apostrophe_sequence() {
    let mut app_config = AppConfig::default();
    app_config.zenzai.enable = true;
    app_config.zenzai.backend = "vulkan".to_string();
    app_config.romaji_table.rows = get_default_romaji_rows();

    let output = TextServiceFactory::resolve_symbol_input_text("n", "'", &app_config);
    assert_eq!(output, None);
}

#[test]
fn zenzai_symbol_input_still_applies_standalone_symbol_rule_without_multi_character_context() {
    let mut app_config = AppConfig::default();
    app_config.zenzai.enable = true;
    app_config.zenzai.backend = "vulkan".to_string();
    app_config.romaji_table.rows = get_default_romaji_rows();

    let output = TextServiceFactory::resolve_symbol_input_text("a", "-", &app_config);
    assert_eq!(output, Some("ー".to_string()));
}

#[test]
fn top_row_digit_input_uses_number_width_setting_when_zenzai_is_disabled() {
    let mut app_config = AppConfig::default();
    app_config.character_width.groups.number = WidthMode::Full;

    let output = TextServiceFactory::resolve_symbol_input_text("", "1", &app_config);
    assert_eq!(output, Some("１".to_string()));
}

#[test]
fn top_row_digit_input_uses_number_width_setting_with_existing_raw_input() {
    let mut app_config = AppConfig::default();
    app_config.character_width.groups.number = WidthMode::Full;

    let output = TextServiceFactory::resolve_symbol_input_text("a", "1", &app_config);
    assert_eq!(output, Some("１".to_string()));
}

#[test]
fn top_row_digit_input_uses_number_width_setting_when_zenzai_is_enabled() {
    let mut app_config = AppConfig::default();
    app_config.zenzai.enable = true;
    app_config.zenzai.backend = "vulkan".to_string();
    app_config.character_width.groups.number = WidthMode::Full;

    let output = TextServiceFactory::resolve_symbol_input_text("", "1", &app_config);
    assert_eq!(output, Some("１".to_string()));
}

#[test]
fn top_row_digit_input_preserves_single_digit_romaji_rule_when_zenzai_is_disabled() {
    let mut app_config = AppConfig::default();
    app_config.character_width.groups.number = WidthMode::Full;
    app_config.romaji_table.rows = vec![row("1", "一", "")];

    let output = TextServiceFactory::resolve_symbol_input_text("", "1", &app_config);
    assert_eq!(output, None);
}

#[test]
fn top_row_digit_input_preserves_single_digit_romaji_rule_when_zenzai_is_enabled() {
    let mut app_config = AppConfig::default();
    app_config.zenzai.enable = true;
    app_config.zenzai.backend = "vulkan".to_string();
    app_config.character_width.groups.number = WidthMode::Full;
    app_config.romaji_table.rows = vec![row("1", "一", "")];

    let output = TextServiceFactory::resolve_symbol_input_text("", "1", &app_config);
    assert_eq!(output, Some("一".to_string()));
}

#[test]
fn top_row_digit_input_preserves_multi_character_romaji_context_when_zenzai_is_enabled() {
    let mut app_config = AppConfig::default();
    app_config.zenzai.enable = true;
    app_config.zenzai.backend = "vulkan".to_string();
    app_config.character_width.groups.number = WidthMode::Full;
    app_config.romaji_table.rows = vec![row("z1", "座布団", "")];

    let output = TextServiceFactory::resolve_symbol_input_text("z", "1", &app_config);
    assert_eq!(output, None);
}
