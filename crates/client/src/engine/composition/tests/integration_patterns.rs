use super::{stateful_harness::*, *};

#[test]
fn clause_integration_baseline_fixture_matches_logged_state() {
    let (harness, backend, _) = run_to_baseline();

    assert_eq!(backend.spec, baseline_spec_state());
    assert_eq!(
        harness_visible_clauses(&harness),
        "いい / 加減 / 統一 / しろ"
    );
    assert_eq!(harness_clause_input_lengths(&harness), "2 / 5 / 6 / 4");
}

#[test]
fn clause_integration_matches_spec_for_histories_up_to_depth_eight() {
    assert_histories_match_up_to_depth_eight();
}

#[test]
fn clause_integration_pattern_a_keeps_exact_raw_clauses() {
    let extra = vec![
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Right,
        HarnessUserAction::Right,
        HarnessUserAction::Right,
        HarnessUserAction::Right,
    ];
    let (harness, _, _) = run_from_baseline(&extra);

    assert_eq!(
        harness_visible_clauses(&harness),
        "い / い / 加減 / 統一 / しろ"
    );
    assert_eq!(harness_clause_input_lengths(&harness), "1 / 1 / 5 / 6 / 4");
}

#[test]
fn clause_integration_pattern_a_presplit_keeps_future_cache() {
    let extra = vec![
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::Left,
    ];
    let (harness, _, _) = run_from_baseline(&extra);

    assert_eq!(
        TextServiceFactory::clause_texts_for_log("", "", &[], &harness.future_clause_snapshots),
        "加減 / 統一 / しろ"
    );
    assert_eq!(
        TextServiceFactory::clause_input_lengths_for_log(0, &[], &harness.future_clause_snapshots),
        "5 / 6 / 4"
    );
}

#[test]
fn clause_integration_pattern_b_keeps_exact_raw_clauses() {
    let extra = vec![
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Right,
        HarnessUserAction::Right,
        HarnessUserAction::Right,
    ];
    let (harness, _, _) = run_from_baseline(&extra);

    assert_eq!(
        harness_visible_clauses(&harness),
        "いい / かげ / ん / 統一 / しろ"
    );
    assert_eq!(harness_clause_input_lengths(&harness), "2 / 4 / 1 / 6 / 4");
}

#[test]
fn clause_integration_pattern_b_presplit_keeps_future_cache() {
    let extra = vec![HarnessUserAction::Left, HarnessUserAction::Left];
    let (harness, _, _) = run_from_baseline(&extra);

    assert_eq!(
        TextServiceFactory::clause_texts_for_log("", "", &[], &harness.future_clause_snapshots),
        "統一 / しろ"
    );
    assert_eq!(
        TextServiceFactory::clause_raw_texts_for_log("", 0, &[], &harness.future_clause_snapshots),
        "とういつ / しろ"
    );
}

#[test]
fn clause_integration_pattern_c_keeps_exact_raw_clauses() {
    let extra = vec![
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Right,
        HarnessUserAction::Right,
    ];
    let (harness, _, _) = run_from_baseline(&extra);

    assert_eq!(
        harness_visible_clauses(&harness),
        "いい / 加減 / とう / いつ / しろ"
    );
    assert_eq!(harness_clause_input_lengths(&harness), "2 / 5 / 3 / 3 / 4");
}

#[test]
fn clause_integration_pattern_d1_preserves_selection_and_raw_clauses() {
    let extra = vec![
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::Space,
        HarnessUserAction::Right,
        HarnessUserAction::Right,
    ];
    let (harness, _, _) = run_from_baseline(&extra);

    assert_eq!(
        harness_visible_clauses(&harness),
        "いい / 下限 / 統一 / しろ"
    );
    assert_eq!(harness_clause_input_lengths(&harness), "2 / 5 / 6 / 4");
}

#[test]
fn clause_integration_pattern_d2_preserves_selection_and_raw_clauses() {
    let extra = vec![
        HarnessUserAction::Left,
        HarnessUserAction::Space,
        HarnessUserAction::Right,
    ];
    let (harness, _, _) = run_from_baseline(&extra);

    assert_eq!(
        harness_visible_clauses(&harness),
        "いい / 加減 / とういつ / しろ"
    );
    assert_eq!(harness_clause_input_lengths(&harness), "2 / 5 / 6 / 4");
}

#[test]
fn clause_integration_pattern_e1_keeps_exact_raw_clauses() {
    let extra = vec![
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Right,
        HarnessUserAction::Right,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Right,
        HarnessUserAction::Right,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Right,
        HarnessUserAction::Right,
    ];
    let (harness, _, _) = run_from_baseline(&extra);

    assert_eq!(
        harness_visible_clauses(&harness),
        "い / い / かげ / ん / とう / いつ / しろ"
    );
    assert_eq!(
        harness_clause_input_lengths(&harness),
        "1 / 1 / 4 / 1 / 3 / 3 / 4"
    );
}

#[test]
fn clause_integration_logged_baseline_pattern_c_keeps_terminal_clause() {
    let extra = vec![
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Right,
        HarnessUserAction::Right,
    ];
    let (harness, _, history) = run_from_logged_baseline(&extra);

    assert_eq!(
        harness_visible_clauses(&harness),
        "いい / 加減 / とう / いつ / しろ",
        "history: {}\nraw clauses: {}\nfuture_clause_snapshots: {}",
        history_string(&history),
        harness_raw_clauses(&harness),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_logged_baseline_pattern_e1_keeps_terminal_clause() {
    let extra = vec![
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Right,
        HarnessUserAction::Right,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Right,
        HarnessUserAction::Right,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Right,
        HarnessUserAction::Right,
    ];
    let (harness, _, history) = run_from_logged_baseline(&extra);

    assert_eq!(
        harness_visible_clauses(&harness),
        "い / い / かげ / ん / とう / いつ / しろ",
        "history: {}\nraw clauses: {}\nfuture_clause_snapshots: {}",
        history_string(&history),
        harness_raw_clauses(&harness),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_fkeys_change_only_current_clause_display() {
    for (set_type, converted_clause) in fkey_cases() {
        let extra = vec![HarnessUserAction::SetTextType(set_type)];
        let (harness, _, history) = run_from_fkey_baseline(&extra);

        assert_eq!(
            harness_visible_clauses(&harness),
            format!("{converted_clause} / 統一"),
            "history: {}\nraw clauses: {}\nfuture_clause_snapshots: {}",
            history_string(&history),
            harness_raw_clauses(&harness),
            TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
        );
        assert_eq!(harness.state, CompositionState::Previewing);
        assert_eq!(harness_raw_clauses(&harness), "かげん / とういつ");
    }
}

#[test]
fn clause_integration_fkeys_preserve_display_when_moving_right() {
    for (set_type, converted_clause) in fkey_cases() {
        let extra = vec![
            HarnessUserAction::SetTextType(set_type),
            HarnessUserAction::Right,
        ];
        let (harness, _, history) = run_from_fkey_baseline(&extra);

        assert_eq!(
            harness_visible_clauses(&harness),
            format!("{converted_clause} / 統一"),
            "history: {}\nraw clauses: {}\nclause_snapshots: {}\nfuture_clause_snapshots: {}",
            history_string(&history),
            harness_raw_clauses(&harness),
            TextServiceFactory::debug_clause_snapshots(&harness.clause_snapshots),
            TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
        );
        assert_eq!(harness_raw_clauses(&harness), "かげん / とういつ");
        assert_eq!(harness_clause_input_lengths(&harness), "5 / 6");
        assert_eq!(
            TextServiceFactory::current_clause_preview(&harness.preview, &harness.fixed_prefix),
            "統一"
        );
    }
}

#[test]
fn clause_integration_fkeys_preserve_display_when_committing_current_clause() {
    for (set_type, converted_clause) in fkey_cases() {
        let extra = vec![
            HarnessUserAction::SetTextType(set_type),
            HarnessUserAction::Enter,
        ];
        let (harness, _, history) = run_from_fkey_baseline(&extra);

        assert_eq!(
            harness_visible_clauses(&harness),
            format!("{converted_clause} / 統一"),
            "history: {}\nraw clauses: {}\nclause_snapshots: {}\nfuture_clause_snapshots: {}",
            history_string(&history),
            harness_raw_clauses(&harness),
            TextServiceFactory::debug_clause_snapshots(&harness.clause_snapshots),
            TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
        );
        assert_eq!(harness_raw_clauses(&harness), "かげん / とういつ");
        assert_eq!(harness_clause_input_lengths(&harness), "5 / 6");
        assert_eq!(harness.committed_clauses.len(), 2);
        assert!(harness.preview.is_empty());
        assert!(harness.future_clause_snapshots.is_empty());
        assert_eq!(harness.state, CompositionState::None);
    }
}

#[test]
fn clause_integration_auto_clause_navigation_uses_first_clause_for_ju_sequence() {
    let extra = vec![HarnessUserAction::Right];
    let (harness, _, history) = run_from_auto_clause_ju(&extra);

    assert_eq!(
        harness_visible_clauses(&harness),
        "準備して / 発表に / 臨む",
        "history: {}\nraw clauses: {}\nclause_snapshots: {}\nfuture_clause_snapshots: {}",
        history_string(&history),
        harness_raw_clauses(&harness),
        TextServiceFactory::debug_clause_snapshots(&harness.clause_snapshots),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
    assert_eq!(harness_clause_input_lengths(&harness), "9 / 11 / 6");
    assert_eq!(harness.raw_input, "haxtupyouninozomu");
    assert_eq!(harness.raw_hiragana, "はっぴょうにのぞむ");
    assert_eq!(
        TextServiceFactory::current_clause_preview(&harness.preview, &harness.fixed_prefix),
        "発表に"
    );
}

#[test]
fn clause_integration_auto_clause_navigation_preserves_tyu_sequence_boundary() {
    let extra = vec![HarnessUserAction::Right];
    let (harness, _, history) = run_from_auto_clause_tyu(&extra);

    assert_eq!(
        harness_visible_clauses(&harness),
        "注意 / して",
        "history: {}\nraw clauses: {}\nclause_snapshots: {}\nfuture_clause_snapshots: {}",
        history_string(&history),
        harness_raw_clauses(&harness),
        TextServiceFactory::debug_clause_snapshots(&harness.clause_snapshots),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
    assert_eq!(harness_raw_clauses(&harness), "ちゅうい / して");
    assert_eq!(harness_clause_input_lengths(&harness), "5 / 4");
    assert_eq!(
        TextServiceFactory::current_clause_preview(&harness.preview, &harness.fixed_prefix),
        "して"
    );
}

#[test]
fn clause_integration_logged_baseline_f7_keeps_future_display_when_moving_left() {
    let extra = vec![
        HarnessUserAction::Left,
        HarnessUserAction::SetTextType(SetTextType::Katakana),
        HarnessUserAction::Left,
    ];
    let (harness, _, history) = run_from_logged_baseline(&extra);

    assert_eq!(
        harness_visible_clauses(&harness),
        "いい / 加減 / トウイツ / しろ",
        "history: {}\nraw clauses: {}\nclause_snapshots: {}\nfuture_clause_snapshots: {}\npreview={} fixed_prefix={} suffix={}",
        history_string(&history),
        harness_raw_clauses(&harness),
        TextServiceFactory::debug_clause_snapshots(&harness.clause_snapshots),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
        harness.preview,
        harness.fixed_prefix,
        harness.suffix,
    );
    assert_eq!(
        TextServiceFactory::current_clause_preview(&harness.preview, &harness.fixed_prefix),
        "加減"
    );
    assert_eq!(harness.suffix, "トウイツしろ");
}
