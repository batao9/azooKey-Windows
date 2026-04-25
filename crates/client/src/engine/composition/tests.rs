use super::{
    Candidates, ClauseActionBackend, ClauseActionStateMut, ClauseSnapshot, Composition,
    CompositionState, FutureClauseSnapshot, TextServiceFactory,
};
use crate::engine::{
    client_action::{ClientAction, SetSelectionType, SetTextType},
    input_mode::InputMode,
    user_action::{Function, Navigation, UserAction},
};
use shared::{get_default_romaji_rows, AppConfig, RomajiRule, WidthMode};

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
