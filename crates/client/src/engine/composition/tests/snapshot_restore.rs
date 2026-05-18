use super::*;

#[test]
fn future_clause_snapshot_uses_relative_clause_preview() {
    let snapshot = TextServiceFactory::build_future_clause_snapshot(
        "いいかげん",
        "とういつしろ",
        "kagentouitusiro",
        "かげんとういつしろ",
        "いい",
        4,
        0,
        &candidates(&["かげん"], &["とういつしろ"], "かげんとういつしろ", &[4]),
    );

    assert_eq!(snapshot.clause_preview, "かげん");
    assert_eq!(snapshot.selected_text, "かげん");
}

#[test]
fn move_clause_left_pushes_current_clause_into_future_cache() {
    let mut future = vec![
        TextServiceFactory::build_conservative_future_clause_snapshot(
            "しろ", "", "siro", "しろ", 2,
        ),
    ];

    TextServiceFactory::push_current_future_clause_snapshot(
        &mut future,
        "いいかげん",
        "とういつしろ",
        "kagentouitusiro",
        "かげんとういつしろ",
        "いい",
        4,
        0,
        false,
        false,
        false,
        None,
        &candidates(&["かげん"], &["とういつしろ"], "かげんとういつしろ", &[4]),
    );

    assert_eq!(future.len(), 2);
    assert_eq!(
        future
            .last()
            .map(|snapshot| snapshot.clause_preview.as_str()),
        Some("かげん"),
        "future={future:?}"
    );
    assert_eq!(
        TextServiceFactory::clause_texts_for_log("", "", &[], &future),
        "かげん / しろ"
    );
}

#[test]
fn move_clause_right_restores_future_clause_without_dropping_following_clauses() {
    let trailing = TextServiceFactory::build_conservative_future_clause_snapshot(
        "しろ", "", "siro", "しろ", 2,
    );
    let restored = TextServiceFactory::build_future_clause_snapshot(
        "いいかげんとういつ",
        "しろ",
        "touitusiro",
        "とういつしろ",
        "いいかげん",
        4,
        0,
        &candidates(&["とういつ"], &["しろ"], "とういつしろ", &[4]),
    );
    let mut future = vec![trailing, restored];

    let restored = future.pop().expect("restored future snapshot");
    let mut preview = String::new();
    let mut suffix = String::new();
    let mut raw_input = String::new();
    let mut raw_hiragana = String::new();
    let mut corresponding_count = 0;
    let mut selection_index = 0;
    let mut current_clause_is_split_derived = false;
    let mut current_clause_is_direct_split_remainder = false;
    let mut current_clause_has_split_left_neighbor = false;
    let mut current_clause_split_group_id = None;
    let mut restored_candidates = Candidates::default();
    TextServiceFactory::restore_future_clause_snapshot(
        &mut preview,
        &mut suffix,
        &mut raw_input,
        &mut raw_hiragana,
        &mut corresponding_count,
        &mut selection_index,
        &mut current_clause_is_split_derived,
        &mut current_clause_is_direct_split_remainder,
        &mut current_clause_has_split_left_neighbor,
        &mut current_clause_split_group_id,
        &mut restored_candidates,
        "いいかげん",
        &restored,
    );
    suffix = TextServiceFactory::sync_current_clause_future_suffix(
        &mut restored_candidates,
        selection_index,
        corresponding_count,
        &future,
    );

    assert_eq!(preview, "いいかげんとういつ");
    assert_eq!(suffix, "しろ");
    assert_eq!(
        TextServiceFactory::clause_texts_for_log(&preview, "いいかげん", &[], &future),
        "とういつ / しろ"
    );
}

struct RichNavigationAfterShrinkBackend {
    shrink_candidates: Candidates,
    navigation_candidates: Candidates,
}

impl ClauseActionBackend for RichNavigationAfterShrinkBackend {
    fn move_cursor(&mut self, offset: i32) -> anyhow::Result<Candidates> {
        if offset == 0 {
            Ok(self.navigation_candidates.clone())
        } else {
            Ok(self.shrink_candidates.clone())
        }
    }

    fn shrink_text(&mut self, _offset: i32) -> anyhow::Result<Candidates> {
        Ok(self.shrink_candidates.clone())
    }
}

#[test]
fn move_clause_right_prefers_rich_navigation_candidates_after_two_clause_shrink() {
    let mut preview = "いい加減".to_string();
    let mut suffix = "統一しろ".to_string();
    let mut raw_input = "iikagentouitusiro".to_string();
    let mut raw_hiragana = "いいかげんとういつしろ".to_string();
    let mut fixed_prefix = String::new();
    let mut corresponding_count = 7;
    let mut selection_index = 0;
    let mut current_candidates =
        candidates(&["いい加減"], &["統一しろ"], "いいかげんとういつしろ", &[7]);
    let mut clause_snapshots = Vec::new();
    let mut future_clause_snapshots = Vec::new();
    let mut current_clause_is_split_derived = true;
    let mut current_clause_is_direct_split_remainder = false;
    let mut current_clause_has_split_left_neighbor = false;
    let mut current_clause_split_group_id = None;
    let mut next_split_group_id = 1;
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
    let mut backend = RichNavigationAfterShrinkBackend {
        shrink_candidates: candidates(&["統一しろ"], &[""], "とういつしろ", &[10]),
        navigation_candidates: candidates(
            &["統一しろ", "とういつしろ", "トウイツしろ"],
            &["", "", ""],
            "とういつしろ",
            &[10, 10, 10],
        ),
    };

    let effect = ClauseState::apply_move_clause(&mut state, &mut backend, 1)
        .expect("move right should succeed");

    assert!(effect.applied);
    assert_eq!(
        state.candidates.texts,
        vec!["統一しろ", "とういつしろ", "トウイツしろ"]
    );
}

#[test]
fn auto_split_future_snapshot_preserves_rich_candidates_for_exact_two_clause_suffix() {
    let mut preview = "いい加減".to_string();
    let mut suffix = "統一しろ".to_string();
    let mut raw_input = "iikagentouitusiro".to_string();
    let mut raw_hiragana = "いいかげんとういつしろ".to_string();
    let mut fixed_prefix = String::new();
    let mut corresponding_count = 7;
    let mut selection_index = 0;
    let mut current_candidates =
        candidates(&["いい加減"], &["統一しろ"], "いいかげんとういつしろ", &[7]);
    let mut clause_snapshots = Vec::new();
    let mut future_clause_snapshots = Vec::new();
    let mut current_clause_is_split_derived = true;
    let mut current_clause_is_direct_split_remainder = false;
    let mut current_clause_has_split_left_neighbor = false;
    let mut current_clause_split_group_id = Some(1);
    let mut next_split_group_id = 2;
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
    let suffix_candidates = candidates(
        &["統一しろ", "統一し路", "とういつしろ", "統一白"],
        &["", "", "", ""],
        "とういつしろ",
        &[10, 10, 10, 10],
    );
    let mut backend = RichNavigationAfterShrinkBackend {
        shrink_candidates: suffix_candidates.clone(),
        navigation_candidates: suffix_candidates,
    };

    TextServiceFactory::rebuild_future_clause_snapshots_from_backend(&mut state, &mut backend)
        .expect("future snapshots should rebuild");

    let snapshot = state
        .future_clause_snapshots
        .last()
        .expect("future suffix snapshot");
    assert_eq!(snapshot.raw_input, "touitusiro");
    assert_eq!(snapshot.raw_hiragana, "とういつしろ");
    assert_eq!(
        snapshot.candidates.texts,
        vec!["統一しろ", "統一し路", "とういつしろ", "統一白"]
    );
    assert!(!snapshot.is_conservative);
}

#[test]
fn move_clause_right_restores_split_group_from_actual_future_clause() {
    let split_group_id = 3;
    let mut restored = TextServiceFactory::build_future_clause_snapshot(
        "いいかげ",
        "んとういつしろ",
        "gentouitusiro",
        "げんとういつしろ",
        "いいか",
        2,
        0,
        &candidates(&["げ"], &["んとういつしろ"], "げんとういつしろ", &[2]),
    );
    restored.is_split_derived = true;
    restored.is_direct_split_remainder = true;
    restored.has_split_left_neighbor = true;
    restored.split_group_id = Some(split_group_id);

    let mut preview = String::new();
    let mut suffix = String::new();
    let mut raw_input = String::new();
    let mut raw_hiragana = String::new();
    let mut corresponding_count = 0;
    let mut selection_index = 0;
    let mut current_clause_is_split_derived = false;
    let mut current_clause_is_direct_split_remainder = false;
    let mut current_clause_has_split_left_neighbor = false;
    let mut current_clause_split_group_id = None;
    let mut restored_candidates = Candidates::default();
    TextServiceFactory::restore_future_clause_snapshot(
        &mut preview,
        &mut suffix,
        &mut raw_input,
        &mut raw_hiragana,
        &mut corresponding_count,
        &mut selection_index,
        &mut current_clause_is_split_derived,
        &mut current_clause_is_direct_split_remainder,
        &mut current_clause_has_split_left_neighbor,
        &mut current_clause_split_group_id,
        &mut restored_candidates,
        "いいか",
        &restored,
    );

    assert!(current_clause_is_split_derived);
    assert!(current_clause_is_direct_split_remainder);
    assert!(current_clause_has_split_left_neighbor);
    assert_eq!(current_clause_split_group_id, Some(split_group_id));
}

#[test]
fn adjust_boundary_split_keeps_future_clause_sequence() {
    let mut future = vec![
        actual_future_snapshot("しろ", "", "siro", "しろ", 2),
        actual_future_snapshot("とういつ", "しろ", "touitusiro", "とういつしろ", 4),
    ];

    TextServiceFactory::maybe_push_split_future_clause_snapshot(
        &mut future,
        "kagentouitusiro",
        "かげんとういつしろ",
        4,
        "んとういつしろ",
        true,
        None,
    );

    assert_eq!(
        future
            .last()
            .map(|snapshot| snapshot.clause_preview.as_str()),
        Some("ん")
    );
    assert_eq!(
        future.last().map(|snapshot| snapshot.raw_input.as_str()),
        Some("ntouitusiro")
    );
    assert_eq!(
        TextServiceFactory::clause_raw_texts_for_log("", 0, &[], &future),
        "ん / とういつ / しろ"
    );
}

#[test]
fn adjust_boundary_without_existing_future_cache_does_not_capture_initial_split_clause() {
    let mut future = Vec::new();

    TextServiceFactory::maybe_push_split_future_clause_snapshot(
        &mut future,
        "iikagentouitusiro",
        "いいかげんとういつしろ",
        1,
        "いかげんとういつしろ",
        false,
        None,
    );

    assert!(future.is_empty());
}

#[test]
fn auto_split_bootstrap_uses_raw_suffix_hint_when_input_count_is_not_kana_count() {
    let mut future = Vec::new();

    TextServiceFactory::maybe_push_split_future_clause_snapshot(
        &mut future,
        "iikagentouitusiro",
        "いいかげんとういつしろ",
        7,
        "とういつしろ",
        true,
        Some(1),
    );

    let snapshot = future
        .last()
        .expect("auto split should cache the remaining clause");
    assert_eq!(snapshot.raw_input, "touitusiro");
    assert_eq!(snapshot.raw_hiragana, "とういつしろ");
    assert_eq!(snapshot.clause_preview, "とういつしろ");
}

#[test]
fn auto_split_bootstrap_recovers_single_n_boundary_consumed_consonant() {
    let mut future = Vec::new();

    TextServiceFactory::maybe_push_split_future_clause_snapshot(
        &mut future,
        "iikagentouitusiro",
        "いいかげんとういつしろ",
        8,
        "とういつしろ",
        true,
        Some(1),
    );

    let snapshot = future
        .last()
        .expect("single-n auto split should recover the consonant after n");
    assert_eq!(snapshot.raw_input, "touitusiro");
    assert_eq!(snapshot.raw_hiragana, "とういつしろ");
    assert_eq!(snapshot.clause_preview, "とういつしろ");
}

#[test]
fn exact_suffix_does_not_trigger_single_n_recovery() {
    assert_eq!(
        TextServiceFactory::recover_single_n_raw_hiragana_suffix(
            "いいかげんとういつしろ",
            "とういつしろ",
        ),
        None
    );
}

#[test]
fn auto_split_bootstrap_keeps_double_n_suffix_boundary() {
    let mut future = Vec::new();

    TextServiceFactory::maybe_push_split_future_clause_snapshot(
        &mut future,
        "iikagenntouitusiro",
        "いいかげんとういつしろ",
        8,
        "とういつしろ",
        true,
        Some(1),
    );

    let snapshot = future
        .last()
        .expect("double-n auto split should cache the same remaining clause");
    assert_eq!(snapshot.raw_input, "touitusiro");
    assert_eq!(snapshot.raw_hiragana, "とういつしろ");
    assert_eq!(snapshot.clause_preview, "とういつしろ");
}

#[test]
fn auto_split_bootstrap_does_not_cache_untrusted_display_suffix() {
    let mut future = Vec::new();

    TextServiceFactory::maybe_push_split_future_clause_snapshot(
        &mut future,
        "iikagentouitusiro",
        "いいかげんとういつしろ",
        7,
        "横溢しろ",
        true,
        Some(1),
    );

    assert!(future.is_empty());
}

#[test]
fn adjust_boundary_bootstraps_last_clause_split_without_existing_future_cache() {
    let mut future = Vec::new();

    TextServiceFactory::maybe_push_split_future_clause_snapshot(
        &mut future,
        "siro",
        "しろ",
        2,
        "ろ",
        true,
        None,
    );

    assert_eq!(future.len(), 1);
    assert_eq!(
        future
            .last()
            .map(|snapshot| snapshot.clause_preview.as_str()),
        Some("ろ")
    );
    assert_eq!(
        future.last().map(|snapshot| snapshot.raw_input.as_str()),
        Some("ro")
    );
    assert_eq!(
        future.last().map(|snapshot| snapshot.raw_hiragana.as_str()),
        Some("ろ")
    );
}

#[test]
fn adjust_boundary_replaces_existing_conservative_future_clause() {
    let mut future = vec![
        actual_future_snapshot("しろ", "", "siro", "しろ", 2),
        TextServiceFactory::build_conservative_future_clause_snapshot(
            "つ",
            "しろ",
            "tusiro",
            "つしろ",
            2,
        ),
    ];

    TextServiceFactory::maybe_push_split_future_clause_snapshot(
        &mut future,
        "touitusiro",
        "とういつしろ",
        3,
        "いつしろ",
        true,
        None,
    );

    assert_eq!(
        future
            .last()
            .map(|snapshot| snapshot.clause_preview.as_str()),
        Some("いつ")
    );
    assert_eq!(
        future.last().map(|snapshot| snapshot.suffix.as_str()),
        Some("しろ")
    );
    assert_eq!(
        future.last().map(|snapshot| snapshot.raw_input.as_str()),
        Some("itusiro")
    );
    assert_eq!(
        future.last().map(|snapshot| snapshot.raw_hiragana.as_str()),
        Some("いつしろ")
    );
}

#[test]
fn adjust_boundary_keeps_terminal_actual_direct_remainder_as_separate_clause() {
    let split_group_id = 11;
    let mut terminal_snapshot = actual_future_snapshot("しろ", "", "siro", "しろ", 2);
    terminal_snapshot.is_split_derived = true;
    terminal_snapshot.is_direct_split_remainder = true;
    terminal_snapshot.has_split_left_neighbor = true;
    terminal_snapshot.split_group_id = Some(split_group_id);
    let mut future = vec![terminal_snapshot];

    TextServiceFactory::maybe_push_split_future_clause_snapshot(
        &mut future,
        "touitusiro",
        "とういつしろ",
        3,
        "いつしろ",
        true,
        Some(split_group_id),
    );

    assert_eq!(
        future
            .iter()
            .rev()
            .map(|snapshot| snapshot.clause_preview.as_str())
            .collect::<Vec<_>>(),
        vec!["いつ", "しろ"]
    );
    assert_eq!(
        future.last().map(|snapshot| snapshot.raw_hiragana.as_str()),
        Some("いつしろ")
    );
    assert_eq!(
        future
            .first()
            .map(|snapshot| snapshot.raw_hiragana.as_str()),
        Some("しろ")
    );
}

#[test]
fn adjust_boundary_keeps_actual_split_derived_future_clause_and_inserts_new_split() {
    let split_group_id = 5;
    let mut future = vec![
        actual_future_snapshot("しろ", "", "siro", "しろ", 2),
        actual_future_snapshot("ん", "とういつしろ", "ntouitusiro", "んとういつしろ", 1),
        actual_future_snapshot(
            "かげ",
            "んとういつしろ",
            "kagentouitusiro",
            "かげんとういつしろ",
            2,
        ),
    ];
    future[1].is_split_derived = true;
    future[1].has_split_left_neighbor = true;
    future[1].split_group_id = Some(split_group_id);
    future[2].is_split_derived = true;
    future[2].has_split_left_neighbor = false;
    future[2].split_group_id = Some(split_group_id);

    TextServiceFactory::maybe_push_split_future_clause_snapshot(
        &mut future,
        "iikagentouitusiro",
        "いいかげんとういつしろ",
        1,
        "いかげんとういつしろ",
        false,
        Some(9),
    );

    assert_eq!(future.len(), 4);
    assert_eq!(
        future
            .iter()
            .rev()
            .map(|snapshot| snapshot.clause_preview.as_str())
            .collect::<Vec<_>>(),
        vec!["い", "かげ", "ん", "しろ"]
    );
    assert_eq!(
        future.last().map(|snapshot| snapshot.split_group_id),
        Some(Some(9))
    );
    assert_eq!(
        future
            .iter()
            .rev()
            .nth(1)
            .and_then(|snapshot| snapshot.split_group_id),
        Some(split_group_id)
    );
}

#[test]
fn adjust_boundary_trim_drops_stale_split_snapshot_before_rejoin() {
    let mut future = vec![
        actual_future_snapshot("しろ", "", "siro", "しろ", 2),
        actual_future_snapshot(
            "かげん",
            "とういつしろ",
            "kagentouitusiro",
            "かげんとういつしろ",
            5,
        ),
    ];

    TextServiceFactory::maybe_push_split_future_clause_snapshot(
        &mut future,
        "iikagentouitusiro",
        "いかげんとういつしろ",
        1,
        "いかげんとういつしろ",
        false,
        None,
    );
    TextServiceFactory::maybe_push_split_future_clause_snapshot(
        &mut future,
        "iikagentouitusiro",
        "かげんとういつしろ",
        2,
        "かげんとういつしろ",
        false,
        None,
    );

    assert_eq!(future.len(), 2);
    assert_eq!(
        future
            .last()
            .map(|snapshot| snapshot.clause_preview.as_str()),
        Some("かげん")
    );
}

#[test]
fn adjust_boundary_rejoin_removes_active_split_group_from_future_cache() {
    let split_group_id = 7;
    let mut future = vec![
        actual_future_snapshot("しろ", "", "siro", "しろ", 2),
        actual_future_snapshot("とういつ", "しろ", "touitusiro", "とういつしろ", 4),
    ];
    let mut split = TextServiceFactory::build_conservative_future_clause_snapshot(
        "ん",
        "とういつしろ",
        "ntouitusiro",
        "んとういつしろ",
        1,
    );
    split.is_split_derived = true;
    split.split_group_id = Some(split_group_id);
    future.push(split);

    TextServiceFactory::maybe_push_split_future_clause_snapshot(
        &mut future,
        "kagentouitusiro",
        "かげんとういつしろ",
        5,
        "とういつしろ",
        true,
        Some(split_group_id),
    );

    assert_eq!(future.len(), 2);
    assert!(
        future
            .iter()
            .all(|snapshot| snapshot.split_group_id != Some(split_group_id)),
        "future={future:?}"
    );
    assert_eq!(
        future
            .last()
            .map(|snapshot| snapshot.clause_preview.as_str()),
        Some("とういつ"),
        "future={future:?}"
    );
}

#[test]
fn restore_selection_prefers_exact_match_then_fallback() {
    let restored_candidates = candidates(
        &["候補A", "候補B", "候補C"],
        &["残り", "残り", "別"],
        "こうほ",
        &[2, 2, 1],
    );

    assert_eq!(
        TextServiceFactory::resolve_selection_index(&restored_candidates, "候補B", "残り", 2, 0,),
        1
    );
    assert_eq!(
        TextServiceFactory::resolve_selection_index(&restored_candidates, "候補X", "残り", 2, 2,),
        2
    );
}

#[test]
fn adjust_boundary_prefers_hint_over_corresponding_count_suffix() {
    let mut future = vec![
        actual_future_snapshot("しろ", "", "siro", "しろ", 2),
        actual_future_snapshot("とういつ", "しろ", "touitusiro", "とういつしろ", 4),
    ];

    TextServiceFactory::maybe_push_split_future_clause_snapshot(
        &mut future,
        "iikagentouitusiro",
        "いいかげんとういつしろ",
        6,
        "んとういつしろ",
        true,
        None,
    );

    assert_eq!(
        future
            .last()
            .map(|snapshot| snapshot.clause_preview.as_str()),
        Some("ん")
    );
    assert_eq!(
        future.last().map(|snapshot| snapshot.raw_hiragana.as_str()),
        Some("んとういつしろ")
    );
}
