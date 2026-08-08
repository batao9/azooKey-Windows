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
fn clause_integration_fkeys_commit_all_clauses_when_clause_navigation_is_active() {
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
    assert_eq!(
        TextServiceFactory::current_clause_preview(&harness.preview, &harness.fixed_prefix),
        "準備して"
    );
    let split_group_id = harness
        .current_clause_split_group_id
        .expect("auto clause navigation should assign a split group");
    assert!(harness.current_clause_is_split_derived);
    assert!(harness
        .future_clause_snapshots
        .iter()
        .all(|snapshot| snapshot.split_group_id == Some(split_group_id)));
}

#[test]
fn clause_integration_auto_clause_navigation_moves_to_second_clause_on_next_right() {
    let extra = vec![HarnessUserAction::Right, HarnessUserAction::Right];
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
        "注意"
    );
}

#[test]
fn clause_integration_auto_clause_navigation_moves_tyu_to_second_clause_on_next_right() {
    let extra = vec![HarnessUserAction::Right, HarnessUserAction::Right];
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
fn clause_integration_auto_clause_navigation_preserves_realtime_suffix_display() {
    let extra = vec![HarnessUserAction::Right];
    let (harness, _, history) = run_from_auto_clause_preserved_suffix(&extra);

    assert_eq!(
        harness_visible_clauses(&harness),
        "ある程度 / 長い / 文節でも / 複数に分割される",
        "history: {}\nraw clauses: {}\nclause_snapshots: {}\nfuture_clause_snapshots: {}\npreview={} fixed_prefix={} suffix={}",
        history_string(&history),
        harness_raw_clauses(&harness),
        TextServiceFactory::debug_clause_snapshots(&harness.clause_snapshots),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
        harness.preview,
        harness.fixed_prefix,
        harness.suffix,
    );
    assert_eq!(harness.suffix, "長い文節でも複数に分割される");
    assert_eq!(
        TextServiceFactory::current_clause_preview(&harness.preview, &harness.fixed_prefix),
        "ある程度"
    );
}

#[test]
fn clause_integration_auto_clause_navigation_left_starts_at_last_clause() {
    let extra = vec![HarnessUserAction::Left];
    let (harness, _, history) = run_from_auto_clause_preserved_suffix(&extra);

    assert_eq!(
        harness_visible_clauses(&harness),
        "ある程度 / 長い / 文節でも / 複数に分割される",
        "history: {}\nraw clauses: {}\nclause_snapshots: {}\nfuture_clause_snapshots: {}\npreview={} fixed_prefix={} suffix={}",
        history_string(&history),
        harness_raw_clauses(&harness),
        TextServiceFactory::debug_clause_snapshots(&harness.clause_snapshots),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
        harness.preview,
        harness.fixed_prefix,
        harness.suffix,
    );
    assert!(harness.suffix.is_empty());
    assert_eq!(
        TextServiceFactory::current_clause_preview(&harness.preview, &harness.fixed_prefix),
        "複数に分割される"
    );
}

#[test]
fn clause_integration_auto_clause_navigation_routes_shift_arrows_to_reducer() {
    let right_extra = vec![HarnessUserAction::Right];
    let (right_harness, _, _) = run_from_auto_clause_preserved_suffix(&right_extra);
    assert_adjust_boundary_is_routed(&right_harness, -1);
    assert_adjust_boundary_is_routed(&right_harness, 1);

    let left_extra = vec![HarnessUserAction::Left];
    let (left_harness, _, _) = run_from_auto_clause_preserved_suffix(&left_extra);
    assert_adjust_boundary_is_routed(&left_harness, -1);
    assert_adjust_boundary_is_routed(&left_harness, 1);
}

#[test]
fn clause_integration_auto_clause_navigation_adjusts_first_clause_both_directions() {
    let left_extra = vec![HarnessUserAction::Right, HarnessUserAction::ShiftLeft];
    let (left, _, left_history) = run_from_auto_clause_ju(&left_extra);
    assert_eq!(
        harness_raw_clauses(&left),
        "じゅんびし / て / はっぴょうに / のぞむ",
        "history: {}\nfuture: {}",
        history_string(&left_history),
        TextServiceFactory::debug_future_clause_snapshots(&left.future_clause_snapshots),
    );
    assert_eq!(harness_clause_input_lengths(&left), "7 / 2 / 11 / 6");

    let right_extra = vec![HarnessUserAction::Right, HarnessUserAction::ShiftRight];
    let (right, _, right_history) = run_from_auto_clause_ju(&right_extra);
    assert_eq!(
        harness_raw_clauses(&right),
        "じゅんびしては / っぴょうに / のぞむ",
        "history: {}\nfuture: {}",
        history_string(&right_history),
        TextServiceFactory::debug_future_clause_snapshots(&right.future_clause_snapshots),
    );
    assert_eq!(harness_clause_input_lengths(&right), "11 / 9 / 6");
}

#[test]
fn clause_integration_auto_clause_navigation_adjusts_middle_clause_both_directions() {
    let left_extra = vec![
        HarnessUserAction::Right,
        HarnessUserAction::Right,
        HarnessUserAction::ShiftLeft,
    ];
    let (left, _, left_history) = run_from_auto_clause_ju(&left_extra);
    assert_eq!(
        harness_raw_clauses(&left),
        "じゅんびして / はっぴょう / に / のぞむ",
        "history: {}",
        history_string(&left_history),
    );

    let right_extra = vec![
        HarnessUserAction::Right,
        HarnessUserAction::Right,
        HarnessUserAction::ShiftRight,
    ];
    let (right, _, right_history) = run_from_auto_clause_ju(&right_extra);
    assert_eq!(
        harness_raw_clauses(&right),
        "じゅんびして / はっぴょうにの / ぞむ",
        "history: {}",
        history_string(&right_history),
    );
}

#[test]
fn clause_integration_round_trips_from_both_auto_navigation_directions_keep_shifts_live() {
    let cases = [
        vec![
            HarnessUserAction::Right,
            HarnessUserAction::Right,
            HarnessUserAction::Right,
            HarnessUserAction::Left,
            HarnessUserAction::Left,
            HarnessUserAction::ShiftLeft,
        ],
        vec![
            HarnessUserAction::Right,
            HarnessUserAction::Right,
            HarnessUserAction::Right,
            HarnessUserAction::Left,
            HarnessUserAction::Left,
            HarnessUserAction::ShiftRight,
        ],
        vec![
            HarnessUserAction::Right,
            HarnessUserAction::Right,
            HarnessUserAction::Right,
            HarnessUserAction::Left,
            HarnessUserAction::Left,
            HarnessUserAction::Right,
            HarnessUserAction::ShiftLeft,
        ],
        vec![
            HarnessUserAction::Right,
            HarnessUserAction::Right,
            HarnessUserAction::Right,
            HarnessUserAction::Left,
            HarnessUserAction::Left,
            HarnessUserAction::Right,
            HarnessUserAction::ShiftRight,
        ],
        vec![
            HarnessUserAction::Left,
            HarnessUserAction::Left,
            HarnessUserAction::Left,
            HarnessUserAction::ShiftLeft,
        ],
        vec![
            HarnessUserAction::Left,
            HarnessUserAction::Left,
            HarnessUserAction::Left,
            HarnessUserAction::ShiftRight,
        ],
        vec![
            HarnessUserAction::Left,
            HarnessUserAction::Left,
            HarnessUserAction::Left,
            HarnessUserAction::Right,
            HarnessUserAction::ShiftLeft,
        ],
        vec![
            HarnessUserAction::Left,
            HarnessUserAction::Left,
            HarnessUserAction::Left,
            HarnessUserAction::Right,
            HarnessUserAction::ShiftRight,
        ],
        vec![
            HarnessUserAction::Left,
            HarnessUserAction::Left,
            HarnessUserAction::Left,
            HarnessUserAction::Right,
            HarnessUserAction::Right,
            HarnessUserAction::ShiftLeft,
        ],
    ];

    for actions in cases {
        let _ = run_from_auto_clause_ju(&actions);
    }
}

#[test]
fn clause_integration_auto_clause_navigation_keeps_adjusted_boundary_after_movement() {
    let extra = vec![
        HarnessUserAction::Right,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Right,
        HarnessUserAction::Left,
    ];
    let (harness, _, history) = run_from_auto_clause_ju(&extra);
    assert_eq!(
        harness_raw_clauses(&harness),
        "じゅんびし / て / はっぴょうに / のぞむ",
        "history: {}",
        history_string(&history),
    );
    assert_eq!(harness_clause_input_lengths(&harness), "7 / 2 / 11 / 6");
}

#[test]
fn clause_integration_adjustment_round_trip_survives_navigation() {
    let extra = vec![
        HarnessUserAction::Right,
        HarnessUserAction::Right,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Right,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_auto_clause_ju(&extra);
    assert_eq!(
        harness_raw_clauses(&harness),
        "じゅんびして / はっぴょうに / のぞむ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
    assert_eq!(
        harness_visible_clauses(&harness),
        "準備して / 発表に / 臨む",
        "history: {}",
        history_string(&history),
    );
    assert_eq!(
        format!("{}{}", harness.preview, harness.suffix),
        "準備して発表に臨む",
        "history: {}",
        history_string(&history),
    );
}

#[test]
fn clause_integration_independent_boundary_restores_do_not_merge() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Right,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
    ];

    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_raw_clauses(&harness),
        "い / い / か / げんと / ういつ / しろ",
        "history: {}",
        history_string(&history),
    );
}

#[test]
fn clause_integration_right_expanded_restore_keeps_origin_boundary() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Right,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
    ];

    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_raw_clauses(&harness),
        "いい / かげ / ん / と / ういつ / しろ",
        "history: {}",
        history_string(&history),
    );
}

#[test]
fn clause_integration_navigation_keeps_right_expanded_restore_boundary() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Right,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
    ];

    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_raw_clauses(&harness),
        "いい / か / げんと / ういつ / しろ",
        "history: {}",
        history_string(&history),
    );
}

#[test]
fn clause_integration_right_expanded_mixed_origin_remainder_stays_separate() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
    ];

    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_raw_clauses(&harness),
        "いいか / げ / んと / うい / つ / しろ",
        "history: {}",
        history_string(&history),
    );
}

#[test]
fn clause_integration_pending_remainder_rejoins_across_split_groups() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Right,
        HarnessUserAction::Right,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
    ];

    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_raw_clauses(&harness),
        "いい / かげんと / う / いつ / しろ",
        "history: {}",
        history_string(&history),
    );
}

#[test]
fn clause_integration_restore_provenance_keeps_different_origin_boundary() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Right,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftLeft,
    ];

    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_raw_clauses(&harness),
        "いい / かげ / ん / とうい / つ / しろ",
        "history: {}",
        history_string(&history),
    );
}

#[test]
fn clause_integration_boundary_round_trip_resets_candidate_selection() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::Space,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
    ];
    let (candidate, _, candidate_history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&candidate),
        "いい / 加減 / 統一 / しろ",
        "history: {}",
        history_string(&candidate_history),
    );
}

#[test]
fn clause_integration_boundary_round_trip_drops_consumed_remainder_restore() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Right,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "いい / 加減 / 統一 / しろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_nested_restore_chain_restores_converted_origin() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Right,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "いい / 加減 / 統一 / しろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_repeated_left_adjustment_restores_converted_next_clause() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Right,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "いい / 加減 / 統一 / しろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_restore_does_not_cross_adjusted_inner_boundary() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Right,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "い / い / かげ / ん / 統一 / しろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_released_prefix_keeps_adjusted_left_boundary() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "い / い / 加減 / 統一 / しろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_nested_full_fragment_restores_inner_converted_clause() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Right,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "いい / 加減 / 統一 / しろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_partial_future_restore_keeps_converted_display() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Right,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "いいか / げん / 統一 / しろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_released_n_keeps_following_adjusted_boundary() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "いいかげ / ん / とうい / つ / しろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_newer_split_boundary_overrides_converted_origin() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Right,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "いい / 加減 / とう / いつ / しろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_released_u_keeps_following_adjusted_boundary_near_end() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Right,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "いい / かげんと / う / いつし / ろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_pending_ge_rejoins_following_n() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Right,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "い / いか / げん / 統一 / しろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_released_ge_rejoins_split_derived_n() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Right,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "いいか / げん / 統一 / しろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_pending_ka_rejoins_ge_but_keeps_n_boundary() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Right,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "いい / かげ / ん / 統一 / しろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_released_ge_rejoins_actual_n_remainder() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Right,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "いいか / げん / 統一 / しろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_terminal_pending_remainder_keeps_different_origin_boundary() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Right,
        HarnessUserAction::Right,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "いい / かげ / ん / とうい / つ / しろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_released_i_rejoins_uniform_t_remainder() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Right,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "いい / かげんとう / いつ / しろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_released_u_rejoins_negative_displacement_i() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Right,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "いい / かげ / んと / うい / つ / しろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_released_u_rejoins_restored_i_remainder() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Right,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "いい / かげんと / うい / つ / しろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_released_converted_clause_keeps_selected_display() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Left,
        HarnessUserAction::Space,
        HarnessUserAction::Space,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "いい / 加減 / とうい / つ / しろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_released_t_rejoins_terminal_unadjusted_clause() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Right,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "いい / 加減 / とうい / つしろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_initial_clause_split_keeps_next_clause_boundary() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "い / い / 加減 / 統一 / しろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_initial_clause_round_trip_restores_next_converted_clause() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Right,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "いい / 加減 / 統一 / しろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_released_n_keeps_following_converted_clause_boundary() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Right,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "いいか / げ / ん / 統一 / しろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_round_trip_keeps_newer_following_split_boundary() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "いい / 加減 / と / ういつし / ろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_released_t_keeps_terminal_unadjusted_clause_boundary() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Right,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "いい / 加減 / とうい / つ / しろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_repeated_terminal_contraction_keeps_pending_remainder_joined() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "いい / 加減 / とうい / つしろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_released_n_keeps_restored_converted_clause_boundary() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Right,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "いいか / げ / ん / 統一 / しろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_released_t_keeps_following_mixed_origin_boundary() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "いい / 加減 / と / ういつしろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_released_u_rejoins_following_same_origin_fragment() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Right,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "いいか / げんと / ういつ / しろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_released_t_rejoins_restored_pending_terminal_clause() {
    let actions = [
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Right,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "いい / かげんと / うい / つしろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_repeated_adjustment_joins_adjacent_pending_terminal_remainders() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "いい / 加減 / とうい / つしろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
    assert_eq!(harness_clause_input_lengths(&harness), "2 / 5 / 4 / 6");
}

#[test]
fn clause_integration_repeated_adjustment_keeps_different_origin_terminal_boundary() {
    let actions = [
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "いい / 加減 / とうい / つ / しろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
    assert_eq!(harness_clause_input_lengths(&harness), "2 / 5 / 4 / 2 / 4");
}

#[test]
fn clause_integration_adjacent_pending_fragments_restore_whole_terminal_origin() {
    let actions = [
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "いい / 加減 / 統一 / しろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
    assert_eq!(harness_clause_input_lengths(&harness), "2 / 5 / 6 / 4");
}

#[test]
fn clause_integration_repeated_right_adjustment_restores_converted_clause_display() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Right,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "いい / 加減 / 統一 / しろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
    assert_eq!(harness_clause_input_lengths(&harness), "2 / 5 / 6 / 4");
}

#[test]
fn clause_integration_released_n_keeps_restored_inner_clause_boundary() {
    let actions = [
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "いい / かげ / ん / 統一 / しろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_released_i_rejoins_pending_mixed_remainder() {
    let actions = [
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "いい / 加減 / とう / いつしろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_navigation_keeps_pending_mixed_remainder_boundary() {
    let actions = [
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Right,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "いい / 加減 / とう / い / つしろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
    assert_eq!(harness_clause_input_lengths(&harness), "2 / 5 / 3 / 1 / 6");
}

#[test]
fn clause_integration_released_s_rejoins_same_origin_terminal_fragment() {
    let actions = [
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Right,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "いい / 加減 / 統一 / しろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_released_u_keeps_following_mixed_origin_fragment_boundary() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Right,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "いい / かげんと / う / いつし / ろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_released_s_rejoins_selected_same_origin_fragment() {
    let actions = [
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Right,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Left,
        HarnessUserAction::Space,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_visible_clauses(&harness),
        "いい / 加減 / 統一 / しろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_keeps_remainders_from_different_origins_separate() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Right,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_raw_clauses(&harness),
        "いい / かげ / ん / と / ういつし / ろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_pending_remainder_absorbs_next_released_prefix() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_raw_clauses(&harness),
        "い / いか / げんと / ういつ / しろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_mixed_origin_clause_rejects_same_origin_prefix_merge() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Right,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_raw_clauses(&harness),
        "いい / か / げんと / ういつ / しろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
}

#[test]
fn clause_integration_fully_consumed_contracted_clause_restores_origin() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_raw_clauses(&harness),
        "いい / かげん / とういつ / しろ",
        "history: {}",
        history_string(&history),
    );
    assert_eq!(
        harness_visible_clauses(&harness),
        "いい / 加減 / 統一 / しろ",
        "history: {}",
        history_string(&history),
    );
}

#[test]
fn clause_integration_restored_contracted_origin_rejoins_previous_clause() {
    let actions = [
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&actions);
    assert_eq!(
        harness_raw_clauses(&harness),
        "い / いかげん / とういつ / しろ",
        "history: {}",
        history_string(&history),
    );
    assert_eq!(
        harness_visible_clauses(&harness),
        "い / いかげん / 統一 / しろ",
        "history: {}",
        history_string(&history),
    );
}

#[test]
fn clause_integration_fully_consumed_clause_restores_converted_display() {
    let mut extra = vec![HarnessUserAction::Right];
    extra.extend(std::iter::repeat_n(HarnessUserAction::ShiftRight, 5));
    extra.extend(std::iter::repeat_n(HarnessUserAction::ShiftLeft, 5));
    let (harness, _, history) = run_from_auto_clause_ju(&extra);
    assert_eq!(
        harness_raw_clauses(&harness),
        "じゅんびして / はっぴょうに / のぞむ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
    assert_eq!(
        harness_visible_clauses(&harness),
        "準備して / 発表に / 臨む",
        "history: {}",
        history_string(&history),
    );
    assert_eq!(
        format!("{}{}", harness.preview, harness.suffix),
        "準備して発表に臨む",
        "history: {}",
        history_string(&history),
    );
}

#[test]
fn clause_integration_fully_consumed_terminal_clause_restores_converted_display() {
    let mut extra = vec![HarnessUserAction::Right];
    extra.extend(std::iter::repeat_n(HarnessUserAction::ShiftRight, 3));
    extra.extend(std::iter::repeat_n(HarnessUserAction::ShiftLeft, 3));
    let (harness, _, history) = run_from_auto_clause_terminal_restore(&extra);
    assert_eq!(
        harness_raw_clauses(&harness),
        "じゅんびして / のぞむ",
        "history: {}\nfuture: {}\nrestore: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
        TextServiceFactory::debug_consumed_prefix_restore(
            harness.current_clause_consumed_prefix_restore.as_ref()
        ),
    );
    assert_eq!(
        harness_visible_clauses(&harness),
        "準備して / 臨む",
        "history: {}",
        history_string(&history),
    );
    assert_eq!(
        format!("{}{}", harness.preview, harness.suffix),
        "準備して臨む",
        "history: {}",
        history_string(&history),
    );
}

#[test]
fn clause_integration_restore_rebases_modified_downstream_boundary() {
    let extra = vec![
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::Left,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Right,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::SetTextType(SetTextType::Katakana),
        HarnessUserAction::Left,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::ShiftLeft,
    ];
    let (harness, _, history) = run_from_baseline(&extra);
    let visible = harness_visible_clauses(&harness);
    assert_eq!(
        harness_raw_clauses(&harness),
        "いい / かげん / とういつし / ろ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
    assert_eq!(
        format!("{}{}", harness.preview, harness.suffix),
        visible.replace(" / ", ""),
        "history: {history:?}; visible={visible}",
    );
}

#[test]
fn clause_integration_auto_clause_navigation_commits_all_after_adjustment() {
    let extra = vec![
        HarnessUserAction::Right,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Enter,
    ];
    let (harness, _, history) = run_from_auto_clause_ju(&extra);
    assert_eq!(
        harness_raw_clauses(&harness).replace(" / ", ""),
        "じゅんびしてはっぴょうにのぞむ",
        "history: {}",
        history_string(&history),
    );
    assert!(harness.preview.is_empty());
    assert!(harness.future_clause_snapshots.is_empty());
    assert_eq!(harness.state, CompositionState::None);
}

#[test]
fn clause_integration_auto_clause_navigation_handles_last_clause_edges() {
    let left_extra = vec![HarnessUserAction::Left, HarnessUserAction::ShiftLeft];
    let (left, _, left_history) = run_from_auto_clause_preserved_suffix(&left_extra);
    assert_eq!(
        harness_raw_clauses(&left),
        "あるていど / ながい / ぶんせつでも / ふくすうにぶんかつされ / る",
        "history: {}",
        history_string(&left_history),
    );

    let right_extra = vec![HarnessUserAction::Left, HarnessUserAction::ShiftRight];
    let (right, _, right_history) = run_from_auto_clause_preserved_suffix(&right_extra);
    assert_eq!(
        harness_raw_clauses(&right),
        "あるていど / ながい / ぶんせつでも / ふくすうにぶんかつされる",
        "history: {}",
        history_string(&right_history),
    );
    assert_eq!(harness_clause_input_lengths(&right), "8 / 5 / 11 / 22");
}

#[test]
fn clause_integration_auto_clause_navigation_adjusts_preserved_first_clause_both_directions() {
    let left_extra = vec![HarnessUserAction::Right, HarnessUserAction::ShiftLeft];
    let (left, _, left_history) = run_from_auto_clause_preserved_suffix(&left_extra);
    assert_eq!(
        harness_raw_clauses(&left),
        "あるてい / ど / ながい / ぶんせつでも / ふくすうにぶんかつされる",
        "history: {}",
        history_string(&left_history),
    );
    assert_eq!(harness_clause_input_lengths(&left), "6 / 2 / 5 / 11 / 22");

    let right_extra = vec![HarnessUserAction::Right, HarnessUserAction::ShiftRight];
    let (right, _, right_history) = run_from_auto_clause_preserved_suffix(&right_extra);
    assert_eq!(
        harness_raw_clauses(&right),
        "あるていどな / がい / ぶんせつでも / ふくすうにぶんかつされる",
        "history: {}",
        history_string(&right_history),
    );
    assert_eq!(harness_clause_input_lengths(&right), "10 / 3 / 11 / 22");
}

#[test]
fn clause_integration_auto_clause_navigation_removes_consumed_next_clause() {
    let extra = vec![
        HarnessUserAction::Right,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::ShiftRight,
        HarnessUserAction::Right,
    ];
    let (harness, _, history) = run_from_auto_clause_ju(&extra);
    assert_eq!(
        harness_raw_clauses(&harness),
        "じゅんびしてはっぴょうに / のぞむ",
        "history: {}\nfuture: {}",
        history_string(&history),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
    );
    assert_eq!(harness_clause_input_lengths(&harness), "20 / 6");
    assert_eq!(
        TextServiceFactory::current_clause_preview(&harness.preview, &harness.fixed_prefix),
        "臨む"
    );
}

#[test]
fn clause_integration_clause_adjustment_keeps_alternate_romaji_digraphs_atomic() {
    for (digraph, raw_digraph) in [
        ("ちゃ", "tya"),
        ("ちゃ", "cha"),
        ("ちゅ", "tyu"),
        ("ちゅ", "chu"),
    ] {
        let extra = vec![
            HarnessUserAction::Right,
            HarnessUserAction::ShiftLeft,
            HarnessUserAction::ShiftLeft,
            HarnessUserAction::ShiftLeft,
        ];
        let (harness, _, history) = run_from_auto_clause_digraph(digraph, raw_digraph, &extra);
        assert_eq!(
            harness_raw_clauses(&harness),
            format!("{digraph} / うい / して"),
            "{raw_digraph}, history: {}",
            history_string(&history),
        );
        assert_eq!(
            harness_clause_input_lengths(&harness),
            "3 / 2 / 4",
            "{raw_digraph}, history: {}",
            history_string(&history),
        );
        assert_eq!(
            harness_raw_clauses(&harness).replace(" / ", ""),
            format!("{digraph}ういして"),
        );
    }
}

#[test]
fn clause_integration_clause_adjustment_restores_alternate_romaji_digraphs_with_shift_right() {
    for (digraph, raw_digraph) in [
        ("ちゃ", "tya"),
        ("ちゃ", "cha"),
        ("ちゅ", "tyu"),
        ("ちゅ", "chu"),
    ] {
        let extra = vec![
            HarnessUserAction::Right,
            HarnessUserAction::ShiftLeft,
            HarnessUserAction::ShiftLeft,
            HarnessUserAction::ShiftRight,
            HarnessUserAction::ShiftRight,
        ];
        let (harness, _, history) = run_from_auto_clause_digraph(digraph, raw_digraph, &extra);
        assert_eq!(
            harness_raw_clauses(&harness),
            format!("{digraph}うい / して"),
            "{raw_digraph}, history: {}",
            history_string(&history),
        );
        assert_eq!(
            harness_clause_input_lengths(&harness),
            "5 / 4",
            "{raw_digraph}, history: {}",
            history_string(&history),
        );
        assert_eq!(
            harness_raw_clauses(&harness).replace(" / ", ""),
            format!("{digraph}ういして"),
        );
    }
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
