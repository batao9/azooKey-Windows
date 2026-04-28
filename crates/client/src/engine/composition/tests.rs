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
