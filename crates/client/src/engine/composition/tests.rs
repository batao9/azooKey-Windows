use super::{
    Candidates, ClauseActionBackend, ClauseActionStateMut, ClauseSnapshot, Composition,
    CompositionState, FutureClauseSnapshot, TextServiceFactory,
};
use crate::engine::{
    client_action::{ClientAction, SetSelectionType, SetTextType},
    input_mode::InputMode,
    user_action::{Function, Navigation, UserAction},
};
use shared::{get_default_romaji_rows, AppConfig, PunctuationStyle, RomajiRule, WidthMode};

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
fn arrow_navigation_prepares_clause_navigation_before_move() {
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
    .expect("right arrow should navigate clauses");

    assert_eq!(
        actions,
        vec![
            ClientAction::EnsureClauseNavigationReady,
            ClientAction::MoveClause(1)
        ]
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
