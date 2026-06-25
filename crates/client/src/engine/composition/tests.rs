use super::{
    Candidates, CapsLockKeyboardLayout, ClauseActionBackend, ClauseActionEffect,
    ClauseActionStateMut, ClauseNavigationReadyUiSync, ClauseSnapshot, ClauseState, Composition,
    CompositionReducer, CompositionState, FutureClauseSnapshot, TextServiceFactory,
};
use crate::engine::{
    client_action::{ClientAction, SetSelectionType, SetTextType},
    input_mode::InputMode,
    ipc_service::WindowRpcDelivery,
    user_action::{Function, Navigation, UserAction},
};
use shared::{get_default_romaji_rows, AppConfig, PunctuationStyle, RomajiRule, WidthMode};
use windows::Win32::Foundation::LPARAM;

pub(super) fn row(input: &str, output: &str, next_input: &str) -> RomajiRule {
    RomajiRule {
        input: input.to_string(),
        output: output.to_string(),
        next_input: next_input.to_string(),
    }
}

pub(super) fn candidates(
    texts: &[&str],
    sub_texts: &[&str],
    hiragana: &str,
    corresponding_count: &[i32],
) -> Candidates {
    Candidates {
        texts: texts.iter().map(|value| (*value).to_string()).collect(),
        sub_texts: sub_texts.iter().map(|value| (*value).to_string()).collect(),
        hiragana: hiragana.to_string(),
        corresponding_count: corresponding_count.to_vec(),
    }
}

pub(super) fn actual_future_snapshot(
    clause_preview: &str,
    suffix: &str,
    raw_input: &str,
    raw_hiragana: &str,
    corresponding_count: i32,
) -> FutureClauseSnapshot {
    TextServiceFactory::build_future_clause_snapshot(
        clause_preview,
        suffix,
        raw_input,
        raw_hiragana,
        "",
        corresponding_count,
        0,
        &candidates(
            &[clause_preview],
            &[suffix],
            raw_hiragana,
            &[corresponding_count],
        ),
    )
}

mod integration_patterns;
mod snapshot_restore;
pub(super) mod stateful_harness;
mod symbol_and_width;

#[test]
fn reducer_plans_composition_start_without_tsf_or_ipc_state() {
    let (_, actions) = CompositionReducer::plan_actions_for_user_action(
        &Composition::default(),
        &UserAction::Input('a'),
        &InputMode::Kana,
        false,
        &AppConfig::default(),
        false,
    )
    .expect("input should start composition");

    assert_eq!(
        actions,
        vec![
            ClientAction::StartComposition,
            ClientAction::AppendText("a".to_string())
        ]
    );
}

#[test]
fn append_result_indicates_server_reset_when_result_only_contains_appended_input() {
    let previous_candidates = candidates(&["感じ"], &[""], "かんじ", &[5]);
    let appended_candidates = candidates(&["k"], &[""], "k", &[1]);

    assert!(TextServiceFactory::append_result_indicates_server_reset(
        "kanji",
        &previous_candidates,
        "k",
        &appended_candidates,
    ));
}

#[test]
fn append_result_does_not_indicate_server_reset_when_result_extends_previous_input() {
    let previous_candidates = candidates(&["感じ"], &[""], "かんじ", &[5]);
    let appended_candidates = candidates(&["感じk"], &[""], "かんじk", &[6]);

    assert!(!TextServiceFactory::append_result_indicates_server_reset(
        "kanji",
        &previous_candidates,
        "k",
        &appended_candidates,
    ));
}

#[test]
fn append_result_does_not_indicate_server_reset_for_romaji_rewrite() {
    let previous_candidates = candidates(&["n"], &[""], "n", &[1]);
    let appended_candidates = candidates(&["な"], &[""], "な", &[2]);

    assert!(!TextServiceFactory::append_result_indicates_server_reset(
        "n",
        &previous_candidates,
        "a",
        &appended_candidates,
    ));
}

#[test]
fn client_composition_state_is_empty_when_all_buffers_are_empty() {
    assert!(!TextServiceFactory::has_client_composition_state(
        "",
        "",
        "",
        "",
        &Candidates::default(),
    ));
}

#[test]
fn client_composition_state_is_present_when_candidates_remain() {
    assert!(TextServiceFactory::has_client_composition_state(
        "",
        "",
        "",
        "",
        &candidates(&["漢字"], &[""], "かんじ", &[5]),
    ));
}

#[test]
fn delayed_candidate_window_does_not_show_on_composition_start() {
    let mut app_config = AppConfig::default();
    app_config.general.show_candidate_window_after_space = true;

    let (_, actions) = TextServiceFactory::plan_actions_for_user_action(
        &Composition::default(),
        &UserAction::Input('a'),
        &InputMode::Kana,
        false,
        &app_config,
        false,
    )
    .expect("input should start composition");

    assert_eq!(
        actions,
        vec![
            ClientAction::StartComposition,
            ClientAction::AppendText("a".to_string())
        ]
    );
}

#[test]
fn delayed_candidate_window_shows_when_space_opens_preview() {
    let mut app_config = AppConfig::default();
    app_config.general.show_candidate_window_after_space = true;
    let composition = Composition {
        state: CompositionState::Composing,
        raw_input: "a".to_string(),
        ..Composition::default()
    };

    let (_, actions) = TextServiceFactory::plan_actions_for_user_action(
        &composition,
        &UserAction::Space,
        &InputMode::Kana,
        false,
        &app_config,
        false,
    )
    .expect("space should enter preview");

    assert_eq!(
        actions,
        vec![
            ClientAction::ShowCandidateWindow,
            ClientAction::SetSelection(SetSelectionType::Down)
        ]
    );
}

#[test]
fn skipped_candidate_window_update_does_not_remember_visibility() {
    assert_eq!(
        TextServiceFactory::delivered_candidate_window_visibility(
            WindowRpcDelivery::SkippedUnavailable,
            Some(true),
        ),
        None
    );
    assert_eq!(
        TextServiceFactory::delivered_candidate_window_visibility(
            WindowRpcDelivery::SkippedUnavailable,
            Some(false),
        ),
        None
    );
    assert_eq!(
        TextServiceFactory::delivered_candidate_window_visibility(WindowRpcDelivery::Sent, None),
        None
    );
    assert_eq!(
        TextServiceFactory::delivered_candidate_window_visibility(
            WindowRpcDelivery::Sent,
            Some(true),
        ),
        Some(true)
    );
}

#[test]
fn input_after_space_conversion_commits_preview_before_starting_new_composition() {
    let composition = Composition {
        state: CompositionState::Previewing,
        preview: "今日は".to_string(),
        raw_input: "kyouha".to_string(),
        raw_hiragana: "きょうは".to_string(),
        corresponding_count: 6,
        ..Composition::default()
    };

    let (transition, actions) = TextServiceFactory::plan_actions_for_user_action(
        &composition,
        &UserAction::Input('a'),
        &InputMode::Kana,
        false,
        &AppConfig::default(),
        false,
    )
    .expect("input should commit preview and start new composition");

    assert_eq!(transition, CompositionState::Composing);
    assert_eq!(
        actions,
        vec![
            ClientAction::EndComposition,
            ClientAction::StartComposition,
            ClientAction::AppendText("a".to_string())
        ]
    );
}

#[test]
fn temporary_latin_after_space_conversion_starts_new_direct_composition() {
    let composition = Composition {
        state: CompositionState::Previewing,
        preview: "今日は".to_string(),
        raw_input: "kyouha".to_string(),
        raw_hiragana: "きょうは".to_string(),
        corresponding_count: 6,
        ..Composition::default()
    };

    let (transition, actions) = TextServiceFactory::plan_actions_for_user_action(
        &composition,
        &UserAction::Input('Ａ'),
        &InputMode::Kana,
        true,
        &AppConfig::default(),
        true,
    )
    .expect("temporary latin should commit preview and start direct composition");

    assert_eq!(transition, CompositionState::Composing);
    assert_eq!(
        actions,
        vec![
            ClientAction::EndComposition,
            ClientAction::StartComposition,
            ClientAction::SetTemporaryLatin(true),
            ClientAction::AppendTextDirect("A".to_string())
        ]
    );
}

#[test]
fn existing_temporary_latin_after_space_conversion_preserves_direct_composition() {
    let composition = Composition {
        state: CompositionState::Previewing,
        temporary_latin: true,
        preview: "AB".to_string(),
        raw_input: "AB".to_string(),
        raw_hiragana: "AB".to_string(),
        corresponding_count: 2,
        ..Composition::default()
    };

    let (transition, actions) = TextServiceFactory::plan_actions_for_user_action(
        &composition,
        &UserAction::Input('c'),
        &InputMode::Kana,
        false,
        &AppConfig::default(),
        false,
    )
    .expect("existing temporary latin should stay direct after committing preview");

    assert_eq!(transition, CompositionState::Composing);
    assert_eq!(
        actions,
        vec![
            ClientAction::EndComposition,
            ClientAction::StartComposition,
            ClientAction::SetTemporaryLatin(true),
            ClientAction::AppendTextDirect("c".to_string())
        ]
    );
}

#[test]
fn delete_uses_remove_text_path_while_composing() {
    let composition = Composition {
        state: CompositionState::Composing,
        raw_input: "ab".to_string(),
        ..Composition::default()
    };

    let (next_state, actions) = TextServiceFactory::plan_actions_for_user_action(
        &composition,
        &UserAction::Delete,
        &InputMode::Kana,
        false,
        &AppConfig::default(),
        false,
    )
    .expect("delete should remove composing text");

    assert_eq!(next_state, CompositionState::Composing);
    assert_eq!(actions, vec![ClientAction::RemoveText]);
}

#[test]
fn delete_ends_composition_after_last_character() {
    let composition = Composition {
        state: CompositionState::Previewing,
        raw_input: "a".to_string(),
        ..Composition::default()
    };

    let (next_state, actions) = TextServiceFactory::plan_actions_for_user_action(
        &composition,
        &UserAction::Delete,
        &InputMode::Kana,
        false,
        &AppConfig::default(),
        false,
    )
    .expect("delete should clear the final composing character");

    assert_eq!(next_state, CompositionState::None);
    assert_eq!(
        actions,
        vec![ClientAction::RemoveText, ClientAction::EndComposition]
    );
}

#[test]
fn live_conversion_reading_respects_setting_and_composition_state() {
    let mut app_config = AppConfig::default();
    let candidates = candidates(&["今位置"], &[""], "こんにちは", &[5]);

    assert_eq!(
        TextServiceFactory::live_conversion_reading(
            &app_config,
            &candidates,
            &CompositionState::Composing,
        ),
        Some("こんにちは")
    );

    app_config.general.show_live_conversion_reading = false;
    assert_eq!(
        TextServiceFactory::live_conversion_reading(
            &app_config,
            &candidates,
            &CompositionState::Composing,
        ),
        None
    );
    assert_eq!(
        TextServiceFactory::live_conversion_reading_update(
            &app_config,
            &candidates,
            &CompositionState::Composing,
        ),
        Some("")
    );

    app_config.general.show_live_conversion_reading = true;
    assert_eq!(
        TextServiceFactory::live_conversion_reading(
            &app_config,
            &candidates,
            &CompositionState::None,
        ),
        None
    );
    assert_eq!(
        TextServiceFactory::live_conversion_reading(
            &app_config,
            &Candidates::default(),
            &CompositionState::Composing,
        ),
        None
    );
}

#[test]
fn ctrl_conversion_shortcuts_are_handled_as_function_keys() {
    let cases = [
        (0x55, Function::Six, SetTextType::Hiragana),
        (0x49, Function::Seven, SetTextType::Katakana),
        (0x4F, Function::Eight, SetTextType::HalfKatakana),
        (0x50, Function::Nine, SetTextType::FullLatin),
        (0x54, Function::Ten, SetTextType::HalfLatin),
    ];

    for (key_code, function, set_text_type) in cases {
        assert_eq!(
            TextServiceFactory::ctrl_conversion_shortcut_function(key_code, true, false),
            Some(function)
        );
        assert_eq!(
            TextServiceFactory::set_text_type_for_function(function),
            set_text_type
        );
    }
}

#[test]
fn ctrl_conversion_shortcuts_do_not_capture_non_ctrl_or_alt_modified_keys() {
    assert_eq!(
        TextServiceFactory::ctrl_conversion_shortcut_function(0x55, false, false),
        None
    );
    assert_eq!(
        TextServiceFactory::ctrl_conversion_shortcut_function(0x55, true, true),
        None
    );
    assert_eq!(
        TextServiceFactory::ctrl_conversion_shortcut_function(0x41, true, false),
        None
    );
}

#[test]
fn eisu_shortcut_matches_ms_ime_capslock_rules_by_keyboard_layout() {
    assert!(TextServiceFactory::is_eisu_shortcut(
        0x14,
        LPARAM(0),
        false,
        false,
        false,
        CapsLockKeyboardLayout::Japanese
    ));
    assert!(TextServiceFactory::is_eisu_shortcut(
        0xF0,
        LPARAM(0x003A0000),
        false,
        false,
        false,
        CapsLockKeyboardLayout::Japanese
    ));
    assert!(!TextServiceFactory::is_eisu_shortcut(
        0xF0,
        LPARAM(0x003A0000),
        true,
        false,
        false,
        CapsLockKeyboardLayout::Japanese
    ));
    assert!(!TextServiceFactory::is_eisu_shortcut(
        0x14,
        LPARAM(0),
        true,
        false,
        false,
        CapsLockKeyboardLayout::Japanese
    ));
    assert!(!TextServiceFactory::is_eisu_shortcut(
        0x14,
        LPARAM(0),
        false,
        false,
        false,
        CapsLockKeyboardLayout::English
    ));
    assert!(TextServiceFactory::is_eisu_shortcut(
        0x14,
        LPARAM(0),
        true,
        false,
        false,
        CapsLockKeyboardLayout::English
    ));
    assert!(TextServiceFactory::is_eisu_shortcut(
        0xF0,
        LPARAM(0x003A0000),
        true,
        false,
        false,
        CapsLockKeyboardLayout::English
    ));
    assert!(!TextServiceFactory::is_eisu_shortcut(
        0xF0,
        LPARAM(0x002A0000),
        true,
        false,
        false,
        CapsLockKeyboardLayout::English
    ));
    assert!(!TextServiceFactory::is_eisu_shortcut(
        0xF0,
        LPARAM(0x003A0000),
        false,
        false,
        false,
        CapsLockKeyboardLayout::English
    ));
    assert!(!TextServiceFactory::is_eisu_shortcut(
        0x14,
        LPARAM(0),
        false,
        true,
        false,
        CapsLockKeyboardLayout::Japanese
    ));
    assert!(!TextServiceFactory::is_eisu_shortcut(
        0x14,
        LPARAM(0),
        false,
        false,
        true,
        CapsLockKeyboardLayout::Japanese
    ));
    assert!(!TextServiceFactory::is_eisu_shortcut(
        0x41,
        LPARAM(0),
        false,
        false,
        false,
        CapsLockKeyboardLayout::Japanese
    ));
}

#[test]
fn keyboard_layout_uses_current_keyboard_type_before_legacy_registry_override() {
    assert_eq!(
        TextServiceFactory::caps_lock_keyboard_layout_from_sources(
            Some(4),
            Some("kbd106.dll"),
            Some("PCAT_106KEY")
        ),
        CapsLockKeyboardLayout::English
    );
    assert_eq!(
        TextServiceFactory::caps_lock_keyboard_layout_from_sources(
            Some(7),
            Some("kbd101.dll"),
            Some("PCAT_101KEY")
        ),
        CapsLockKeyboardLayout::Japanese
    );
}

#[test]
fn keyboard_layout_falls_back_to_hardware_registry_for_unknown_keyboard_type() {
    assert_eq!(
        TextServiceFactory::caps_lock_keyboard_layout_from_sources(None, None, None),
        CapsLockKeyboardLayout::Japanese
    );
    assert_eq!(
        TextServiceFactory::caps_lock_keyboard_layout_from_sources(
            Some(0x51),
            Some("kbd106.dll"),
            Some("PCAT_106KEY")
        ),
        CapsLockKeyboardLayout::Japanese
    );
    assert_eq!(
        TextServiceFactory::caps_lock_keyboard_layout_from_sources(
            Some(0x51),
            Some("kbd101.dll"),
            Some("PCAT_101KEY")
        ),
        CapsLockKeyboardLayout::English
    );
    assert_eq!(
        TextServiceFactory::caps_lock_keyboard_layout_from_sources(
            Some(0x51),
            None,
            Some("PCAT_101KEY")
        ),
        CapsLockKeyboardLayout::English
    );
}

#[test]
fn kana_input_lowercases_ascii_uppercase_from_capslock() {
    assert_eq!(
        TextServiceFactory::input_text_for_mode('A', &InputMode::Kana),
        "a"
    );
    assert_eq!(
        TextServiceFactory::input_text_for_mode('A', &InputMode::Latin),
        "A"
    );
    assert_eq!(
        TextServiceFactory::input_text_for_mode('Ａ', &InputMode::Kana),
        "Ａ"
    );
}

#[test]
fn right_arrow_prepares_clause_navigation_without_initial_move() {
    let composition = Composition {
        state: CompositionState::Composing,
        preview: "いい加減統一しろ".to_string(),
        raw_input: "iikagentouitusiro".to_string(),
        raw_hiragana: "いいかげんとういつしろ".to_string(),
        corresponding_count: 17,
        ..Composition::default()
    };

    let (_, actions) = TextServiceFactory::plan_actions_for_user_action(
        &composition,
        &UserAction::Navigation(Navigation::Right),
        &InputMode::Kana,
        false,
        &AppConfig::default(),
        false,
    )
    .expect("right arrow should prepare clause navigation");

    assert_eq!(actions, vec![ClientAction::EnsureClauseNavigationReady]);
}

#[test]
fn initial_left_arrow_defers_clause_navigation_ready_ui_sync_until_last_clause() {
    let actions = vec![
        ClientAction::EnsureClauseNavigationReady,
        ClientAction::MoveClause(TextServiceFactory::MOVE_CLAUSE_TO_LAST),
    ];

    assert!(TextServiceFactory::should_defer_clause_navigation_ready_sync(&actions, 0));
    assert!(!TextServiceFactory::should_defer_clause_navigation_ready_sync(&actions, 1));
}

#[test]
fn applied_clause_navigation_ready_sync_shows_candidate_window() {
    assert_eq!(
        TextServiceFactory::clause_navigation_ready_ui_sync(ClauseActionEffect::applied(true)),
        Some(ClauseNavigationReadyUiSync {
            update_pos: true,
            visible: Some(true),
        })
    );
    assert_eq!(
        TextServiceFactory::clause_navigation_ready_ui_sync(ClauseActionEffect::skipped()),
        None
    );
}

#[test]
fn deferred_clause_navigation_ready_sync_preserves_candidate_window_show_request() {
    let deferred_sync = Some(ClauseNavigationReadyUiSync {
        update_pos: true,
        visible: Some(true),
    });

    assert_eq!(
        TextServiceFactory::deferred_clause_navigation_ready_ui_sync_after_move(
            deferred_sync,
            ClauseActionEffect::skipped()
        ),
        deferred_sync
    );
    assert_eq!(
        TextServiceFactory::deferred_clause_navigation_ready_ui_sync_after_move(
            deferred_sync,
            ClauseActionEffect::applied(false)
        ),
        Some(ClauseNavigationReadyUiSync {
            update_pos: false,
            visible: Some(true),
        })
    );
}

#[test]
fn enter_commits_all_when_clause_navigation_is_active() {
    let composition = Composition {
        state: CompositionState::Composing,
        preview: "加減".to_string(),
        raw_input: "kagentouitu".to_string(),
        raw_hiragana: "かげんとういつ".to_string(),
        corresponding_count: 5,
        future_clause_snapshots: vec![actual_future_snapshot("統一", "", "touitu", "とういつ", 6)],
        ..Composition::default()
    };

    let (transition, actions) = TextServiceFactory::plan_actions_for_user_action(
        &composition,
        &UserAction::Enter,
        &InputMode::Kana,
        false,
        &AppConfig::default(),
        false,
    )
    .expect("enter should commit active clause navigation");

    assert_eq!(transition, CompositionState::None);
    assert_eq!(actions, vec![ClientAction::EndComposition]);
}

#[test]
fn ctrl_down_keeps_current_clause_commit_in_clause_navigation() {
    let composition = Composition {
        state: CompositionState::Composing,
        preview: "加減".to_string(),
        suffix: "統一".to_string(),
        raw_input: "kagentouitu".to_string(),
        raw_hiragana: "かげんとういつ".to_string(),
        corresponding_count: 5,
        future_clause_snapshots: vec![actual_future_snapshot("統一", "", "touitu", "とういつ", 6)],
        ..Composition::default()
    };

    let (transition, actions) = TextServiceFactory::plan_actions_for_user_action(
        &composition,
        &UserAction::CommitAndNextClause,
        &InputMode::Kana,
        false,
        &AppConfig::default(),
        false,
    )
    .expect("ctrl+down should commit current clause");

    assert_eq!(transition, CompositionState::Composing);
    assert_eq!(actions, vec![ClientAction::ShrinkText("".to_string())]);
}

struct NonProgressMoveBackend {
    shrink_calls: usize,
}

impl ClauseActionBackend for NonProgressMoveBackend {
    fn move_cursor(&mut self, _offset: i32) -> anyhow::Result<Candidates> {
        Ok(Candidates::default())
    }

    fn shrink_text(&mut self, _offset: i32) -> anyhow::Result<Candidates> {
        self.shrink_calls += 1;
        assert!(
            self.shrink_calls <= 1,
            "MOVE_CLAUSE_TO_LAST retried a non-progressing right move"
        );
        Ok(Candidates::default())
    }
}

#[test]
fn move_clause_to_last_stops_when_right_move_makes_no_progress() {
    let mut preview = "いい加減".to_string();
    let mut suffix = "統一".to_string();
    let mut raw_input = "iikagentouitu".to_string();
    let mut raw_hiragana = "いいかげんとういつ".to_string();
    let mut fixed_prefix = String::new();
    let mut corresponding_count = 7;
    let mut selection_index = 0;
    let mut candidates = candidates(&["いい加減"], &["統一"], "いいかげんとういつ", &[7]);
    let mut clause_snapshots = Vec::new();
    let mut future_clause_snapshots = Vec::new();
    let mut current_clause_is_split_derived = true;
    let mut current_clause_is_direct_split_remainder = false;
    let mut current_clause_has_split_left_neighbor = false;
    let mut current_clause_split_group_id = None;
    let mut next_split_group_id = 1;
    let mut backend = NonProgressMoveBackend { shrink_calls: 0 };

    let mut state = ClauseActionStateMut {
        preview: &mut preview,
        suffix: &mut suffix,
        raw_input: &mut raw_input,
        raw_hiragana: &mut raw_hiragana,
        fixed_prefix: &mut fixed_prefix,
        corresponding_count: &mut corresponding_count,
        selection_index: &mut selection_index,
        candidates: &mut candidates,
        clause_snapshots: &mut clause_snapshots,
        future_clause_snapshots: &mut future_clause_snapshots,
        current_clause_is_split_derived: &mut current_clause_is_split_derived,
        current_clause_is_direct_split_remainder: &mut current_clause_is_direct_split_remainder,
        current_clause_has_split_left_neighbor: &mut current_clause_has_split_left_neighbor,
        current_clause_split_group_id: &mut current_clause_split_group_id,
        next_split_group_id: &mut next_split_group_id,
    };

    let effect = TextServiceFactory::apply_move_clause(
        &mut state,
        &mut backend,
        TextServiceFactory::MOVE_CLAUSE_TO_LAST,
    )
    .expect("move to last should return");

    assert!(!effect.applied);
    assert_eq!(backend.shrink_calls, 1);
}

struct NonProgressEnsureBackend {
    move_cursor_zero_calls: usize,
    shrink_calls: usize,
}

impl ClauseActionBackend for NonProgressEnsureBackend {
    fn move_cursor(&mut self, offset: i32) -> anyhow::Result<Candidates> {
        if offset == 0 {
            self.move_cursor_zero_calls += 1;
            if self.move_cursor_zero_calls == 1 {
                return Ok(candidates(
                    &["いい加減"],
                    &["統一"],
                    "いいかげんとういつ",
                    &[7],
                ));
            }
        }
        Ok(Candidates::default())
    }

    fn shrink_text(&mut self, _offset: i32) -> anyhow::Result<Candidates> {
        self.shrink_calls += 1;
        assert!(
            self.shrink_calls <= 1,
            "future snapshot rebuild retried a non-progressing right move"
        );
        Ok(Candidates::default())
    }
}

#[test]
fn ensure_clause_navigation_stops_rebuilding_future_on_non_progress_move() {
    let mut preview = "いい加減統一".to_string();
    let mut suffix = String::new();
    let mut raw_input = "iikagentouitu".to_string();
    let mut raw_hiragana = "いいかげんとういつ".to_string();
    let mut fixed_prefix = String::new();
    let mut corresponding_count = 13;
    let mut selection_index = 0;
    let mut candidates = candidates(&["いい加減統一"], &[""], "いいかげんとういつ", &[13]);
    let mut clause_snapshots = Vec::new();
    let mut future_clause_snapshots = Vec::new();
    let mut current_clause_is_split_derived = false;
    let mut current_clause_is_direct_split_remainder = false;
    let mut current_clause_has_split_left_neighbor = false;
    let mut current_clause_split_group_id = None;
    let mut next_split_group_id = 1;
    let mut backend = NonProgressEnsureBackend {
        move_cursor_zero_calls: 0,
        shrink_calls: 0,
    };

    let mut state = ClauseActionStateMut {
        preview: &mut preview,
        suffix: &mut suffix,
        raw_input: &mut raw_input,
        raw_hiragana: &mut raw_hiragana,
        fixed_prefix: &mut fixed_prefix,
        corresponding_count: &mut corresponding_count,
        selection_index: &mut selection_index,
        candidates: &mut candidates,
        clause_snapshots: &mut clause_snapshots,
        future_clause_snapshots: &mut future_clause_snapshots,
        current_clause_is_split_derived: &mut current_clause_is_split_derived,
        current_clause_is_direct_split_remainder: &mut current_clause_is_direct_split_remainder,
        current_clause_has_split_left_neighbor: &mut current_clause_has_split_left_neighbor,
        current_clause_split_group_id: &mut current_clause_split_group_id,
        next_split_group_id: &mut next_split_group_id,
    };

    let effect = TextServiceFactory::ensure_clause_navigation_ready(&mut state, &mut backend)
        .expect("ensure clause navigation should return");

    assert!(effect.applied);
    assert_eq!(backend.shrink_calls, 1);
    assert!(future_clause_snapshots.is_empty());
}

struct PreserveSelectionEnsureBackend;

impl ClauseActionBackend for PreserveSelectionEnsureBackend {
    fn move_cursor(&mut self, offset: i32) -> anyhow::Result<Candidates> {
        if offset == 0 {
            return Ok(candidates(
                &["いい加減", "良い加減"],
                &["統一", "統一"],
                "いいかげんとういつ",
                &[7, 7],
            ));
        }
        Ok(Candidates::default())
    }

    fn shrink_text(&mut self, _offset: i32) -> anyhow::Result<Candidates> {
        Ok(candidates(&["統一"], &[""], "とういつ", &[6]))
    }
}

struct ReorderedNavigationEnsureBackend;

impl ClauseActionBackend for ReorderedNavigationEnsureBackend {
    fn move_cursor(&mut self, offset: i32) -> anyhow::Result<Candidates> {
        if offset == 0 {
            return Ok(candidates(
                &["いい加減", "良い加減", "程良い加減"],
                &["統一", "統一", "統一"],
                "いいかげんとういつ",
                &[7, 7, 7],
            ));
        }
        Ok(Candidates::default())
    }

    fn shrink_text(&mut self, _offset: i32) -> anyhow::Result<Candidates> {
        Ok(candidates(&["統一"], &[""], "とういつ", &[6]))
    }
}

struct FKeyDisplayEnsureBackend {
    shrunk: bool,
}

impl ClauseActionBackend for FKeyDisplayEnsureBackend {
    fn move_cursor(&mut self, offset: i32) -> anyhow::Result<Candidates> {
        if offset == 0 && !self.shrunk {
            return Ok(candidates(
                &["加減", "下限", "かげん"],
                &["統一", "統一", "統一"],
                "かげんとういつ",
                &[5, 5, 5],
            ));
        }
        Ok(Candidates::default())
    }

    fn shrink_text(&mut self, _offset: i32) -> anyhow::Result<Candidates> {
        self.shrunk = true;
        Ok(candidates(&["統一"], &[""], "とういつ", &[6]))
    }
}

#[test]
fn ensure_clause_navigation_preserves_current_candidate_selection() {
    let mut preview = "良い加減統一".to_string();
    let mut suffix = String::new();
    let mut raw_input = "iikagentouitu".to_string();
    let mut raw_hiragana = "いいかげんとういつ".to_string();
    let mut fixed_prefix = String::new();
    let mut corresponding_count = 13;
    let mut selection_index = 1;
    let mut current_candidates = candidates(
        &["いい加減統一", "良い加減統一"],
        &["", ""],
        "いいかげんとういつ",
        &[13, 13],
    );
    let mut clause_snapshots = Vec::new();
    let mut future_clause_snapshots = Vec::new();
    let mut current_clause_is_split_derived = false;
    let mut current_clause_is_direct_split_remainder = false;
    let mut current_clause_has_split_left_neighbor = false;
    let mut current_clause_split_group_id = None;
    let mut next_split_group_id = 1;
    let mut backend = PreserveSelectionEnsureBackend;

    let mut state = ClauseActionStateMut {
        preview: &mut preview,
        suffix: &mut suffix,
        raw_input: &mut raw_input,
        raw_hiragana: &mut raw_hiragana,
        fixed_prefix: &mut fixed_prefix,
        corresponding_count: &mut corresponding_count,
        selection_index: &mut selection_index,
        candidates: &mut current_candidates,
        clause_snapshots: &mut clause_snapshots,
        future_clause_snapshots: &mut future_clause_snapshots,
        current_clause_is_split_derived: &mut current_clause_is_split_derived,
        current_clause_is_direct_split_remainder: &mut current_clause_is_direct_split_remainder,
        current_clause_has_split_left_neighbor: &mut current_clause_has_split_left_neighbor,
        current_clause_split_group_id: &mut current_clause_split_group_id,
        next_split_group_id: &mut next_split_group_id,
    };

    let effect = TextServiceFactory::ensure_clause_navigation_ready(&mut state, &mut backend)
        .expect("ensure clause navigation should return");

    assert!(effect.applied);
    assert_eq!(preview, "良い加減");
    assert_eq!(selection_index, 1);
    assert_eq!(corresponding_count, 7);
    assert_eq!(suffix, "統一");
}

#[test]
fn ensure_clause_navigation_matches_current_preview_before_reusing_index() {
    let mut preview = "程良い加減統一".to_string();
    let mut suffix = String::new();
    let mut raw_input = "iikagentouitu".to_string();
    let mut raw_hiragana = "いいかげんとういつ".to_string();
    let mut fixed_prefix = String::new();
    let mut corresponding_count = 13;
    let mut selection_index = 1;
    let mut current_candidates = candidates(
        &["いい加減統一", "程良い加減統一"],
        &["", ""],
        "いいかげんとういつ",
        &[13, 13],
    );
    let mut clause_snapshots = Vec::new();
    let mut future_clause_snapshots = Vec::new();
    let mut current_clause_is_split_derived = false;
    let mut current_clause_is_direct_split_remainder = false;
    let mut current_clause_has_split_left_neighbor = false;
    let mut current_clause_split_group_id = None;
    let mut next_split_group_id = 1;
    let mut backend = ReorderedNavigationEnsureBackend;

    let mut state = ClauseActionStateMut {
        preview: &mut preview,
        suffix: &mut suffix,
        raw_input: &mut raw_input,
        raw_hiragana: &mut raw_hiragana,
        fixed_prefix: &mut fixed_prefix,
        corresponding_count: &mut corresponding_count,
        selection_index: &mut selection_index,
        candidates: &mut current_candidates,
        clause_snapshots: &mut clause_snapshots,
        future_clause_snapshots: &mut future_clause_snapshots,
        current_clause_is_split_derived: &mut current_clause_is_split_derived,
        current_clause_is_direct_split_remainder: &mut current_clause_is_direct_split_remainder,
        current_clause_has_split_left_neighbor: &mut current_clause_has_split_left_neighbor,
        current_clause_split_group_id: &mut current_clause_split_group_id,
        next_split_group_id: &mut next_split_group_id,
    };

    let effect = TextServiceFactory::ensure_clause_navigation_ready(&mut state, &mut backend)
        .expect("ensure clause navigation should return");

    assert!(effect.applied);
    assert_eq!(preview, "程良い加減");
    assert_eq!(selection_index, 2);
    assert_eq!(corresponding_count, 7);
    assert_eq!(suffix, "統一");
}

#[test]
fn ensure_clause_navigation_preserves_fkey_display_preview() {
    let mut preview = "カゲントウイツ".to_string();
    let mut suffix = String::new();
    let mut raw_input = "kagentouitu".to_string();
    let mut raw_hiragana = "かげんとういつ".to_string();
    let mut fixed_prefix = String::new();
    let mut corresponding_count = 11;
    let mut selection_index = 0;
    let mut current_candidates = candidates(
        &["加減統一", "下限統一", "かげん統一"],
        &["", "", ""],
        "かげんとういつ",
        &[11, 11, 11],
    );
    let mut clause_snapshots = Vec::new();
    let mut future_clause_snapshots = Vec::new();
    let mut current_clause_is_split_derived = false;
    let mut current_clause_is_direct_split_remainder = false;
    let mut current_clause_has_split_left_neighbor = false;
    let mut current_clause_split_group_id = None;
    let mut next_split_group_id = 1;
    let mut backend = FKeyDisplayEnsureBackend { shrunk: false };

    let mut state = ClauseActionStateMut {
        preview: &mut preview,
        suffix: &mut suffix,
        raw_input: &mut raw_input,
        raw_hiragana: &mut raw_hiragana,
        fixed_prefix: &mut fixed_prefix,
        corresponding_count: &mut corresponding_count,
        selection_index: &mut selection_index,
        candidates: &mut current_candidates,
        clause_snapshots: &mut clause_snapshots,
        future_clause_snapshots: &mut future_clause_snapshots,
        current_clause_is_split_derived: &mut current_clause_is_split_derived,
        current_clause_is_direct_split_remainder: &mut current_clause_is_direct_split_remainder,
        current_clause_has_split_left_neighbor: &mut current_clause_has_split_left_neighbor,
        current_clause_split_group_id: &mut current_clause_split_group_id,
        next_split_group_id: &mut next_split_group_id,
    };

    let effect = TextServiceFactory::ensure_clause_navigation_ready(&mut state, &mut backend)
        .expect("ensure clause navigation should return");

    assert!(effect.applied);
    assert_eq!(preview, "カゲン");
    assert_eq!(suffix, "トウイツ");
    assert_eq!(selection_index, 0);
    assert_eq!(corresponding_count, 5);
    assert_eq!(current_candidates.texts[0], "カゲン");
    assert_eq!(current_candidates.sub_texts[0], "トウイツ");
}

#[test]
fn ensure_clause_navigation_clamps_out_of_range_selection_index() {
    let mut preview = "程良い加減統一".to_string();
    let mut suffix = String::new();
    let mut raw_input = "iikagentouitu".to_string();
    let mut raw_hiragana = "いいかげんとういつ".to_string();
    let mut fixed_prefix = String::new();
    let mut corresponding_count = 13;
    let mut selection_index = 3;
    let mut current_candidates = candidates(
        &[
            "いい加減統一",
            "良い加減統一",
            "好い加減統一",
            "程良い加減統一",
        ],
        &["", "", "", ""],
        "いいかげんとういつ",
        &[13, 13, 13, 13],
    );
    let mut clause_snapshots = Vec::new();
    let mut future_clause_snapshots = Vec::new();
    let mut current_clause_is_split_derived = false;
    let mut current_clause_is_direct_split_remainder = false;
    let mut current_clause_has_split_left_neighbor = false;
    let mut current_clause_split_group_id = None;
    let mut next_split_group_id = 1;
    let mut backend = PreserveSelectionEnsureBackend;

    let mut state = ClauseActionStateMut {
        preview: &mut preview,
        suffix: &mut suffix,
        raw_input: &mut raw_input,
        raw_hiragana: &mut raw_hiragana,
        fixed_prefix: &mut fixed_prefix,
        corresponding_count: &mut corresponding_count,
        selection_index: &mut selection_index,
        candidates: &mut current_candidates,
        clause_snapshots: &mut clause_snapshots,
        future_clause_snapshots: &mut future_clause_snapshots,
        current_clause_is_split_derived: &mut current_clause_is_split_derived,
        current_clause_is_direct_split_remainder: &mut current_clause_is_direct_split_remainder,
        current_clause_has_split_left_neighbor: &mut current_clause_has_split_left_neighbor,
        current_clause_split_group_id: &mut current_clause_split_group_id,
        next_split_group_id: &mut next_split_group_id,
    };

    let effect = TextServiceFactory::ensure_clause_navigation_ready(&mut state, &mut backend)
        .expect("ensure clause navigation should return");

    assert!(effect.applied);
    assert_eq!(preview, "良い加減");
    assert_eq!(selection_index, 1);
    assert_eq!(corresponding_count, 7);
    assert_eq!(suffix, "統一");
}

struct MoveRightCollapsedRemainderBackend {
    moved_left: bool,
}

impl ClauseActionBackend for MoveRightCollapsedRemainderBackend {
    fn move_cursor(&mut self, offset: i32) -> anyhow::Result<Candidates> {
        if offset < 0 {
            self.moved_left = true;
        }

        if self.moved_left {
            return Ok(candidates(
                &["長い", "永い", "ながい"],
                &["文節でも複数に分割される"; 3],
                "ながいぶんせつでもふくすうにぶんかつされる",
                &[5, 5, 5],
            ));
        }

        Ok(Candidates::default())
    }

    fn shrink_text(&mut self, _offset: i32) -> anyhow::Result<Candidates> {
        Ok(candidates(
            &["長い文節でも複数に分割される"],
            &[""],
            "ながいぶんせつでもふくすうにぶんかつされる",
            &[26],
        ))
    }
}

#[test]
fn move_clause_right_preserves_future_clause_when_server_returns_collapsed_remainder() {
    let mut preview = "ある程度".to_string();
    let mut suffix = "長い文節でも複数に分割される".to_string();
    let mut raw_input = "aruteidonagaibunsetudemohukusuunibunkatusareru".to_string();
    let mut raw_hiragana = "あるていどながいぶんせつでもふくすうにぶんかつされる".to_string();
    let mut fixed_prefix = String::new();
    let mut corresponding_count = 9;
    let mut selection_index = 0;
    let mut current_candidates = candidates(
        &["ある程度"],
        &["長い文節でも複数に分割される"],
        "あるていどながいぶんせつでもふくすうにぶんかつされる",
        &[9],
    );
    let mut clause_snapshots = Vec::new();
    let mut future_clause_snapshots = vec![
        actual_future_snapshot(
            "文節でも",
            "複数に分割される",
            "bunsetudemohukusuunibunkatusareru",
            "ぶんせつでもふくすうにぶんかつされる",
            11,
        ),
        TextServiceFactory::build_future_clause_snapshot(
            "長い",
            "文節でも複数に分割される",
            "nagaibunsetudemohukusuunibunkatusareru",
            "ながいぶんせつでもふくすうにぶんかつされる",
            "",
            5,
            0,
            &candidates(
                &["長い", "永い", "ながい"],
                &["文節でも複数に分割される"; 3],
                "ながいぶんせつでもふくすうにぶんかつされる",
                &[5, 5, 5],
            ),
        ),
    ];
    let mut current_clause_is_split_derived = false;
    let mut current_clause_is_direct_split_remainder = false;
    let mut current_clause_has_split_left_neighbor = false;
    let mut current_clause_split_group_id = None;
    let mut next_split_group_id = 1;
    let mut backend = MoveRightCollapsedRemainderBackend { moved_left: false };

    let mut state = ClauseActionStateMut {
        preview: &mut preview,
        suffix: &mut suffix,
        raw_input: &mut raw_input,
        raw_hiragana: &mut raw_hiragana,
        fixed_prefix: &mut fixed_prefix,
        corresponding_count: &mut corresponding_count,
        selection_index: &mut selection_index,
        candidates: &mut current_candidates,
        clause_snapshots: &mut clause_snapshots,
        future_clause_snapshots: &mut future_clause_snapshots,
        current_clause_is_split_derived: &mut current_clause_is_split_derived,
        current_clause_is_direct_split_remainder: &mut current_clause_is_direct_split_remainder,
        current_clause_has_split_left_neighbor: &mut current_clause_has_split_left_neighbor,
        current_clause_split_group_id: &mut current_clause_split_group_id,
        next_split_group_id: &mut next_split_group_id,
    };

    let effect = TextServiceFactory::apply_move_clause(&mut state, &mut backend, 1)
        .expect("right move should return");

    assert!(effect.applied);
    assert!(backend.moved_left);
    assert_eq!(
        TextServiceFactory::current_clause_preview(&preview, &fixed_prefix),
        "長い"
    );
    assert_eq!(suffix, "文節でも複数に分割される");
    assert_eq!(
        TextServiceFactory::clause_texts_for_log(
            &preview,
            &fixed_prefix,
            &[],
            &future_clause_snapshots
        ),
        "長い / 文節でも"
    );
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SingleNBoundaryServerState {
    Full,
    BadSuffix,
}

struct InitialSplitSingleNBoundaryBackend {
    server: SingleNBoundaryServerState,
    snapshots: Vec<SingleNBoundaryServerState>,
}

impl InitialSplitSingleNBoundaryBackend {
    fn current_candidates(&self) -> Candidates {
        match self.server {
            SingleNBoundaryServerState::Full => {
                candidates(&["いい加減"], &["横溢しろ"], "いいかげんとういつしろ", &[7])
            }
            SingleNBoundaryServerState::BadSuffix => {
                candidates(&["横溢しろ"], &[""], "おういつしろ", &[9])
            }
        }
    }
}

impl ClauseActionBackend for InitialSplitSingleNBoundaryBackend {
    fn move_cursor(&mut self, offset: i32) -> anyhow::Result<Candidates> {
        match offset {
            value if value == TextServiceFactory::MOVE_CURSOR_PUSH_CLAUSE_SNAPSHOT => {
                self.snapshots.push(self.server);
            }
            value if value == TextServiceFactory::MOVE_CURSOR_POP_CLAUSE_SNAPSHOT => {
                if let Some(restored) = self.snapshots.pop() {
                    self.server = restored;
                }
            }
            _ => {}
        }
        Ok(self.current_candidates())
    }

    fn shrink_text(&mut self, offset: i32) -> anyhow::Result<Candidates> {
        assert_eq!(offset, 7);
        self.server = SingleNBoundaryServerState::BadSuffix;
        Ok(self.current_candidates())
    }
}

#[test]
fn initial_auto_split_recovers_single_n_suffix_without_reinput() {
    let mut preview = "いい加減統一しろ".to_string();
    let mut suffix = String::new();
    let mut raw_input = "iikagentouitusiro".to_string();
    let mut raw_hiragana = "いいかげんとういつしろ".to_string();
    let mut fixed_prefix = String::new();
    let mut corresponding_count = 17;
    let mut selection_index = 0;
    let mut current_candidates = candidates(
        &["いい加減統一しろ"],
        &[""],
        "いいかげんとういつしろ",
        &[17],
    );
    let mut clause_snapshots = Vec::new();
    let mut future_clause_snapshots = Vec::new();
    let mut current_clause_is_split_derived = false;
    let mut current_clause_is_direct_split_remainder = false;
    let mut current_clause_has_split_left_neighbor = false;
    let mut current_clause_split_group_id = None;
    let mut next_split_group_id = 1;
    let mut backend = InitialSplitSingleNBoundaryBackend {
        server: SingleNBoundaryServerState::Full,
        snapshots: Vec::new(),
    };

    {
        let mut state = ClauseActionStateMut {
            preview: &mut preview,
            suffix: &mut suffix,
            raw_input: &mut raw_input,
            raw_hiragana: &mut raw_hiragana,
            fixed_prefix: &mut fixed_prefix,
            corresponding_count: &mut corresponding_count,
            selection_index: &mut selection_index,
            candidates: &mut current_candidates,
            clause_snapshots: &mut clause_snapshots,
            future_clause_snapshots: &mut future_clause_snapshots,
            current_clause_is_split_derived: &mut current_clause_is_split_derived,
            current_clause_is_direct_split_remainder: &mut current_clause_is_direct_split_remainder,
            current_clause_has_split_left_neighbor: &mut current_clause_has_split_left_neighbor,
            current_clause_split_group_id: &mut current_clause_split_group_id,
            next_split_group_id: &mut next_split_group_id,
        };
        let effect = TextServiceFactory::ensure_clause_navigation_ready(&mut state, &mut backend)
            .expect("clause navigation should prepare");
        assert!(effect.applied);
    }

    assert_eq!(backend.server, SingleNBoundaryServerState::Full);
    assert_eq!(backend.snapshots.len(), 0);
    assert_eq!(suffix, "統一しろ");
    assert_eq!(
        future_clause_snapshots
            .last()
            .map(|snapshot| snapshot.selected_text.as_str()),
        Some("統一しろ")
    );
    assert_eq!(
        future_clause_snapshots
            .last()
            .map(|snapshot| snapshot.raw_input.as_str()),
        Some("touitusiro")
    );
    assert_eq!(
        future_clause_snapshots
            .last()
            .map(|snapshot| snapshot.raw_hiragana.as_str()),
        Some("とういつしろ")
    );

    {
        let mut state = ClauseActionStateMut {
            preview: &mut preview,
            suffix: &mut suffix,
            raw_input: &mut raw_input,
            raw_hiragana: &mut raw_hiragana,
            fixed_prefix: &mut fixed_prefix,
            corresponding_count: &mut corresponding_count,
            selection_index: &mut selection_index,
            candidates: &mut current_candidates,
            clause_snapshots: &mut clause_snapshots,
            future_clause_snapshots: &mut future_clause_snapshots,
            current_clause_is_split_derived: &mut current_clause_is_split_derived,
            current_clause_is_direct_split_remainder: &mut current_clause_is_direct_split_remainder,
            current_clause_has_split_left_neighbor: &mut current_clause_has_split_left_neighbor,
            current_clause_split_group_id: &mut current_clause_split_group_id,
            next_split_group_id: &mut next_split_group_id,
        };
        let effect = TextServiceFactory::apply_move_clause(&mut state, &mut backend, 1)
            .expect("right move should restore the cached suffix");
        assert!(effect.applied);
    }

    assert_eq!(suffix, "");
    assert_eq!(raw_input, "touitusiro");
    assert_eq!(raw_hiragana, "とういつしろ");
    assert_eq!(
        TextServiceFactory::current_clause_preview(&preview, &fixed_prefix),
        "統一しろ"
    );
    assert_eq!(
        current_candidates.texts.first().map(String::as_str),
        Some("統一しろ")
    );
}

#[test]
fn punctuation_commit_defaults_off_and_keeps_existing_append_path() {
    let composition = Composition {
        state: CompositionState::Composing,
        preview: "今日は".to_string(),
        raw_input: "kyouha".to_string(),
        raw_hiragana: "きょうは".to_string(),
        corresponding_count: 6,
        ..Composition::default()
    };

    let (transition, actions) = TextServiceFactory::plan_actions_for_user_action(
        &composition,
        &UserAction::Input(','),
        &InputMode::Kana,
        false,
        &AppConfig::default(),
        false,
    )
    .expect("comma should keep composing by default");

    assert_eq!(transition, CompositionState::Composing);
    assert_eq!(actions, vec![ClientAction::AppendText(",".to_string())]);
}

#[test]
fn punctuation_commit_commits_current_composition_then_punctuation() {
    let mut app_config = AppConfig::default();
    app_config.general.punctuation_commit = true;
    let composition = Composition {
        state: CompositionState::Composing,
        preview: "今日は".to_string(),
        raw_input: "kyouha".to_string(),
        raw_hiragana: "きょうは".to_string(),
        corresponding_count: 6,
        ..Composition::default()
    };

    let (transition, actions) = TextServiceFactory::plan_actions_for_user_action(
        &composition,
        &UserAction::Input(','),
        &InputMode::Kana,
        false,
        &app_config,
        false,
    )
    .expect("comma should commit punctuation");

    assert_eq!(transition, CompositionState::None);
    assert_eq!(
        actions,
        vec![
            ClientAction::EndComposition,
            ClientAction::CommitTextDirect("、".to_string())
        ]
    );
}

#[test]
fn punctuation_commit_respects_punctuation_style_and_width_for_output() {
    let mut app_config = AppConfig::default();
    app_config.general.punctuation_commit = true;
    app_config.general.punctuation_style = PunctuationStyle::FullwidthCommaFullwidthPeriod;
    app_config.character_width.groups.comma_period = WidthMode::Half;
    let composition = Composition {
        state: CompositionState::Composing,
        preview: "今日は".to_string(),
        raw_input: "kyouha".to_string(),
        raw_hiragana: "きょうは".to_string(),
        corresponding_count: 6,
        ..Composition::default()
    };

    let (_, comma_actions) = TextServiceFactory::plan_actions_for_user_action(
        &composition,
        &UserAction::Input(','),
        &InputMode::Kana,
        false,
        &app_config,
        false,
    )
    .expect("comma should commit punctuation");
    let (_, period_actions) = TextServiceFactory::plan_actions_for_user_action(
        &composition,
        &UserAction::Input('.'),
        &InputMode::Kana,
        false,
        &app_config,
        false,
    )
    .expect("period should commit punctuation");

    assert_eq!(
        comma_actions,
        vec![
            ClientAction::EndComposition,
            ClientAction::CommitTextDirect(",".to_string())
        ]
    );
    assert_eq!(
        period_actions,
        vec![
            ClientAction::EndComposition,
            ClientAction::CommitTextDirect(".".to_string())
        ]
    );
}

#[test]
fn punctuation_commit_preserves_multi_character_romaji_punctuation_sequences() {
    let mut app_config = AppConfig::default();
    app_config.general.punctuation_commit = true;
    app_config.romaji_table.rows = get_default_romaji_rows();
    let composition = Composition {
        state: CompositionState::Composing,
        preview: "z".to_string(),
        raw_input: "z".to_string(),
        raw_hiragana: "z".to_string(),
        corresponding_count: 1,
        ..Composition::default()
    };

    let (period_transition, period_actions) = TextServiceFactory::plan_actions_for_user_action(
        &composition,
        &UserAction::Input('.'),
        &InputMode::Kana,
        false,
        &app_config,
        false,
    )
    .expect("z. should stay on the romaji input path");
    let (comma_transition, comma_actions) = TextServiceFactory::plan_actions_for_user_action(
        &composition,
        &UserAction::Input(','),
        &InputMode::Kana,
        false,
        &app_config,
        false,
    )
    .expect("z, should stay on the romaji input path");

    assert_eq!(period_transition, CompositionState::Composing);
    assert_eq!(
        period_actions,
        vec![ClientAction::AppendText(".".to_string())]
    );
    assert_eq!(comma_transition, CompositionState::Composing);
    assert_eq!(
        comma_actions,
        vec![ClientAction::AppendText(",".to_string())]
    );
}

#[test]
fn punctuation_commit_preserves_numpad_multi_character_romaji_punctuation_sequences() {
    let mut app_config = AppConfig::default();
    app_config.general.punctuation_commit = true;
    app_config.romaji_table.rows = get_default_romaji_rows();
    let composition = Composition {
        state: CompositionState::Composing,
        preview: "z".to_string(),
        raw_input: "z".to_string(),
        raw_hiragana: "z".to_string(),
        corresponding_count: 1,
        ..Composition::default()
    };

    let (period_transition, period_actions) = TextServiceFactory::plan_actions_for_user_action(
        &composition,
        &UserAction::NumpadSymbol('.'),
        &InputMode::Kana,
        false,
        &app_config,
        false,
    )
    .expect("numpad z. should stay on the romaji input path");
    let (comma_transition, comma_actions) = TextServiceFactory::plan_actions_for_user_action(
        &composition,
        &UserAction::NumpadSymbol(','),
        &InputMode::Kana,
        false,
        &app_config,
        false,
    )
    .expect("numpad z, should stay on the romaji input path");

    assert_eq!(period_transition, CompositionState::Composing);
    assert_eq!(
        period_actions,
        vec![ClientAction::AppendTextRaw(".".to_string())]
    );
    assert_eq!(comma_transition, CompositionState::Composing);
    assert_eq!(
        comma_actions,
        vec![ClientAction::AppendTextRaw(",".to_string())]
    );
}

#[test]
fn punctuation_commit_preserves_zenzai_single_symbol_romaji_mapping() {
    let mut app_config = AppConfig::default();
    app_config.general.punctuation_commit = true;
    app_config.zenzai.enable = true;
    app_config.zenzai.backend = "vulkan".to_string();
    app_config.romaji_table.rows = vec![row("?", "QUESTION", "")];
    let composition = Composition {
        state: CompositionState::Composing,
        preview: "今日は".to_string(),
        raw_input: "kyouha".to_string(),
        raw_hiragana: "きょうは".to_string(),
        corresponding_count: 6,
        ..Composition::default()
    };

    let (transition, actions) = TextServiceFactory::plan_actions_for_user_action(
        &composition,
        &UserAction::Input('?'),
        &InputMode::Kana,
        false,
        &app_config,
        false,
    )
    .expect("single-symbol romaji mapping should commit mapped punctuation");

    assert_eq!(transition, CompositionState::None);
    assert_eq!(
        actions,
        vec![
            ClientAction::EndComposition,
            ClientAction::CommitTextDirect("QUESTION".to_string())
        ]
    );
}

#[test]
fn punctuation_commit_also_applies_while_previewing() {
    let mut app_config = AppConfig::default();
    app_config.general.punctuation_commit = true;
    let composition = Composition {
        state: CompositionState::Previewing,
        preview: "今日は".to_string(),
        raw_input: "kyouha".to_string(),
        raw_hiragana: "きょうは".to_string(),
        corresponding_count: 6,
        ..Composition::default()
    };

    let (transition, actions) = TextServiceFactory::plan_actions_for_user_action(
        &composition,
        &UserAction::Input('。'),
        &InputMode::Kana,
        false,
        &app_config,
        false,
    )
    .expect("kuten should commit punctuation while previewing");

    assert_eq!(transition, CompositionState::None);
    assert_eq!(
        actions,
        vec![
            ClientAction::EndComposition,
            ClientAction::CommitTextDirect("。".to_string())
        ]
    );
}

#[test]
fn punctuation_commit_can_disable_punctuation_target_only() {
    let mut app_config = AppConfig::default();
    app_config.general.punctuation_commit = true;
    app_config.general.punctuation_commit_punctuation = false;
    let composition = Composition {
        state: CompositionState::Composing,
        preview: "今日は".to_string(),
        raw_input: "kyouha".to_string(),
        raw_hiragana: "きょうは".to_string(),
        corresponding_count: 6,
        ..Composition::default()
    };

    let (transition, actions) = TextServiceFactory::plan_actions_for_user_action(
        &composition,
        &UserAction::Input(','),
        &InputMode::Kana,
        false,
        &app_config,
        false,
    )
    .expect("comma should keep composing when punctuation target is disabled");

    assert_eq!(transition, CompositionState::Composing);
    assert_eq!(actions, vec![ClientAction::AppendText(",".to_string())]);
}

#[test]
fn punctuation_commit_supports_exclamation_and_question_with_fullwidth_setting() {
    let mut app_config = AppConfig::default();
    app_config.general.punctuation_commit = true;
    app_config.character_width.groups.question_exclamation = WidthMode::Full;
    let composition = Composition {
        state: CompositionState::Composing,
        preview: "今日は".to_string(),
        raw_input: "kyouha".to_string(),
        raw_hiragana: "きょうは".to_string(),
        corresponding_count: 6,
        ..Composition::default()
    };

    let (_, exclamation_actions) = TextServiceFactory::plan_actions_for_user_action(
        &composition,
        &UserAction::Input('!'),
        &InputMode::Kana,
        false,
        &app_config,
        false,
    )
    .expect("exclamation should commit");
    let (_, question_actions) = TextServiceFactory::plan_actions_for_user_action(
        &composition,
        &UserAction::Input('?'),
        &InputMode::Kana,
        false,
        &app_config,
        false,
    )
    .expect("question should commit");

    assert_eq!(
        exclamation_actions,
        vec![
            ClientAction::EndComposition,
            ClientAction::CommitTextDirect("！".to_string())
        ]
    );
    assert_eq!(
        question_actions,
        vec![
            ClientAction::EndComposition,
            ClientAction::CommitTextDirect("？".to_string())
        ]
    );
}

#[test]
fn punctuation_commit_supports_fullwidth_exclamation_and_question_with_halfwidth_setting() {
    let mut app_config = AppConfig::default();
    app_config.general.punctuation_commit = true;
    app_config.character_width.groups.question_exclamation = WidthMode::Half;
    let composition = Composition {
        state: CompositionState::Composing,
        preview: "今日は".to_string(),
        raw_input: "kyouha".to_string(),
        raw_hiragana: "きょうは".to_string(),
        corresponding_count: 6,
        ..Composition::default()
    };

    let (_, exclamation_actions) = TextServiceFactory::plan_actions_for_user_action(
        &composition,
        &UserAction::Input('！'),
        &InputMode::Kana,
        false,
        &app_config,
        false,
    )
    .expect("fullwidth exclamation should commit");
    let (_, question_actions) = TextServiceFactory::plan_actions_for_user_action(
        &composition,
        &UserAction::Input('？'),
        &InputMode::Kana,
        false,
        &app_config,
        false,
    )
    .expect("fullwidth question should commit");

    assert_eq!(
        exclamation_actions,
        vec![
            ClientAction::EndComposition,
            ClientAction::CommitTextDirect("!".to_string())
        ]
    );
    assert_eq!(
        question_actions,
        vec![
            ClientAction::EndComposition,
            ClientAction::CommitTextDirect("?".to_string())
        ]
    );
}

#[test]
fn punctuation_commit_can_disable_exclamation_and_question_individually() {
    let mut app_config = AppConfig::default();
    app_config.general.punctuation_commit = true;
    app_config.general.punctuation_commit_exclamation = false;
    app_config.general.punctuation_commit_question = false;
    let composition = Composition {
        state: CompositionState::Composing,
        preview: "今日は".to_string(),
        raw_input: "kyouha".to_string(),
        raw_hiragana: "きょうは".to_string(),
        corresponding_count: 6,
        ..Composition::default()
    };

    let (_, exclamation_actions) = TextServiceFactory::plan_actions_for_user_action(
        &composition,
        &UserAction::Input('!'),
        &InputMode::Kana,
        false,
        &app_config,
        false,
    )
    .expect("disabled exclamation should keep composing");
    let (_, question_actions) = TextServiceFactory::plan_actions_for_user_action(
        &composition,
        &UserAction::Input('?'),
        &InputMode::Kana,
        false,
        &app_config,
        false,
    )
    .expect("disabled question should keep composing");

    assert_eq!(
        exclamation_actions,
        vec![ClientAction::AppendText("!".to_string())]
    );
    assert_eq!(
        question_actions,
        vec![ClientAction::AppendText("?".to_string())]
    );
}

#[test]
fn fkeys_use_finalized_terminal_n_hiragana() {
    assert_eq!(
        TextServiceFactory::converted_clause_preview_text(
            &SetTextType::Hiragana,
            "kagen",
            "かげん",
        ),
        "かげん"
    );
    assert_eq!(
        TextServiceFactory::converted_clause_preview_text(
            &SetTextType::Katakana,
            "kagen",
            "かげん",
        ),
        "カゲン"
    );
}

#[test]
fn temporary_latin_after_finalized_terminal_n_starts_direct_remainder() {
    let composition = Composition {
        state: CompositionState::Composing,
        preview: "加減".to_string(),
        raw_input: "kagen".to_string(),
        raw_hiragana: "かげん".to_string(),
        corresponding_count: 5,
        ..Composition::default()
    };

    let (transition, actions) = TextServiceFactory::plan_actions_for_user_action(
        &composition,
        &UserAction::Input('Ａ'),
        &InputMode::Kana,
        true,
        &AppConfig::default(),
        true,
    )
    .expect("temporary latin should append direct text");

    assert_eq!(transition, CompositionState::Composing);
    assert_eq!(
        actions,
        vec![
            ClientAction::SetTemporaryLatin(true),
            ClientAction::ShrinkTextDirect("A".to_string()),
        ]
    );
}

#[test]
fn temporary_latin_keeps_direct_append_without_finalized_terminal_n() {
    let composition = Composition {
        state: CompositionState::Composing,
        preview: "かげn".to_string(),
        raw_input: "kagen".to_string(),
        raw_hiragana: "かげn".to_string(),
        corresponding_count: 5,
        ..Composition::default()
    };

    let (_, actions) = TextServiceFactory::plan_actions_for_user_action(
        &composition,
        &UserAction::Input('Ａ'),
        &InputMode::Kana,
        true,
        &AppConfig::default(),
        true,
    )
    .expect("temporary latin should append direct text");

    assert_eq!(
        actions,
        vec![
            ClientAction::SetTemporaryLatin(true),
            ClientAction::AppendTextDirect("A".to_string()),
        ]
    );
}

#[test]
fn temporary_latin_keeps_direct_append_when_raw_input_suffix_remains() {
    let composition = Composition {
        state: CompositionState::Composing,
        preview: "かん".to_string(),
        raw_input: "kann".to_string(),
        raw_hiragana: "かんん".to_string(),
        corresponding_count: 3,
        ..Composition::default()
    };

    let (_, actions) = TextServiceFactory::plan_actions_for_user_action(
        &composition,
        &UserAction::Input('Ａ'),
        &InputMode::Kana,
        true,
        &AppConfig::default(),
        true,
    )
    .expect("temporary latin should keep direct append when suffix remains");

    assert_eq!(
        actions,
        vec![
            ClientAction::SetTemporaryLatin(true),
            ClientAction::AppendTextDirect("A".to_string()),
        ]
    );
}
