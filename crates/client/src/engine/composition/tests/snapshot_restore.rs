use super::*;
use std::sync::Arc;

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
        false,
        0,
        None,
        None,
        None,
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
    let mut current_clause_is_pending_remainder = false;
    let mut current_clause_has_split_left_neighbor = false;
    let mut current_clause_right_boundary_displacement = 0;
    let mut current_clause_right_boundary_origin = None;
    let mut current_clause_split_group_id = None;
    let mut current_clause_consumed_prefix_restore = None;
    let mut current_clause_remainder_origin = None;
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
        &mut current_clause_is_pending_remainder,
        &mut current_clause_has_split_left_neighbor,
        &mut current_clause_right_boundary_displacement,
        &mut current_clause_right_boundary_origin,
        &mut current_clause_split_group_id,
        &mut current_clause_consumed_prefix_restore,
        &mut current_clause_remainder_origin,
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
    raw_input: ClauseAdvanceRawInput,
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

    fn advance_clause(
        &mut self,
        _offset: i32,
        _previous_candidates: &Candidates,
    ) -> anyhow::Result<ClauseAdvance> {
        Ok(ClauseAdvance {
            shrunk: self.shrink_candidates.clone(),
            navigation: self.navigation_candidates.clone(),
            raw_input: self.raw_input.clone(),
        })
    }
}

struct DesynchronizedFutureRestoreBackend {
    shrink_candidates: Candidates,
    navigation_candidates: Candidates,
    snapshot_depth: usize,
    pop_count: usize,
    cursor_move_attempted: bool,
}

impl ClauseActionBackend for DesynchronizedFutureRestoreBackend {
    fn move_cursor(&mut self, offset: i32) -> anyhow::Result<Candidates> {
        if offset != 0 {
            self.cursor_move_attempted = true;
        }
        Ok(if self.cursor_move_attempted {
            Candidates::default()
        } else {
            self.navigation_candidates.clone()
        })
    }

    fn shrink_text(&mut self, _offset: i32) -> anyhow::Result<Candidates> {
        Ok(self.shrink_candidates.clone())
    }

    fn advance_clause(
        &mut self,
        _offset: i32,
        _previous_candidates: &Candidates,
    ) -> anyhow::Result<ClauseAdvance> {
        self.snapshot_depth += 1;
        Ok(ClauseAdvance {
            shrunk: self.shrink_candidates.clone(),
            navigation: self.navigation_candidates.clone(),
            raw_input: ClauseAdvanceRawInput::Verified("kusuuni".to_string()),
        })
    }

    fn update_composition_snapshot(
        &mut self,
        operation: ClauseSnapshotOperation,
        _previous_candidates: &Candidates,
    ) -> anyhow::Result<()> {
        if operation == ClauseSnapshotOperation::Pop {
            assert!(self.snapshot_depth > 0, "server snapshot stack underflow");
            self.snapshot_depth -= 1;
            self.pop_count += 1;
        }
        Ok(())
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
    let mut current_clause_consumed_prefix_restore = None;
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
        current_clause_is_pending_remainder: &mut false,
        current_clause_has_split_left_neighbor: &mut current_clause_has_split_left_neighbor,
        current_clause_right_boundary_displacement: &mut 0,
        current_clause_right_boundary_origin: &mut None,
        current_clause_split_group_id: &mut current_clause_split_group_id,
        current_clause_consumed_prefix_restore: &mut current_clause_consumed_prefix_restore,
        current_clause_remainder_origin: &mut None,
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
        raw_input: ClauseAdvanceRawInput::Unverified,
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
fn move_clause_right_does_not_replace_materialized_candidates_with_conservative_snapshot() {
    let mut preview = "ある程度な買い文章でもふ".to_string();
    let mut suffix = "くすうに".to_string();
    let mut raw_input = "bunsyoudemofukusuuni".to_string();
    let mut raw_hiragana = "ぶんしょうでもふくすうに".to_string();
    let mut fixed_prefix = "ある程度な買い".to_string();
    let mut corresponding_count = 13;
    let mut selection_index = 0;
    let mut current_candidates = candidates(
        &["文章でもふ"],
        &["くすうに"],
        "ぶんしょうでもふくすうに",
        &[13],
    );
    let mut clause_snapshots = Vec::new();
    let mut future_clause_snapshots = vec![
        TextServiceFactory::build_conservative_future_clause_snapshot(
            "くすうに",
            "",
            "kusuuni",
            "くすうに",
            7,
        ),
    ];
    let mut current_clause_is_split_derived = true;
    let mut current_clause_is_direct_split_remainder = false;
    let mut current_clause_is_pending_remainder = false;
    let mut current_clause_has_split_left_neighbor = true;
    let mut current_clause_right_boundary_displacement = 1;
    let mut current_clause_right_boundary_origin = None;
    let mut current_clause_split_group_id = Some(1);
    let mut current_clause_consumed_prefix_restore = None;
    let mut current_clause_remainder_origin = None;
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
        current_clause_is_pending_remainder: &mut current_clause_is_pending_remainder,
        current_clause_has_split_left_neighbor: &mut current_clause_has_split_left_neighbor,
        current_clause_right_boundary_displacement: &mut current_clause_right_boundary_displacement,
        current_clause_right_boundary_origin: &mut current_clause_right_boundary_origin,
        current_clause_split_group_id: &mut current_clause_split_group_id,
        current_clause_consumed_prefix_restore: &mut current_clause_consumed_prefix_restore,
        current_clause_remainder_origin: &mut current_clause_remainder_origin,
        next_split_group_id: &mut next_split_group_id,
    };
    let mut backend = RichNavigationAfterShrinkBackend {
        shrink_candidates: Candidates {
            hiragana: "くすうに".to_string(),
            ..Candidates::default()
        },
        navigation_candidates: candidates(
            &["複数に", "服数に", "ふくすうに"],
            &["", "", ""],
            "くすうに",
            &[7, 7, 7],
        ),
        raw_input: ClauseAdvanceRawInput::Verified("kusuuni".to_string()),
    };

    let effect = ClauseState::apply_move_clause(&mut state, &mut backend, 1)
        .expect("move right should materialize the adjusted following clause");

    assert!(effect.applied);
    assert_eq!(
        TextServiceFactory::current_clause_preview(state.preview, state.fixed_prefix),
        "くすうに"
    );
    assert_eq!(
        state.candidates.texts,
        vec!["くすうに", "複数に", "服数に", "ふくすうに"],
        "the conservative cache may preserve the boundary, but must not discard live candidates"
    );
    assert_eq!(*state.selection_index, 0);
    assert_eq!(
        TextServiceFactory::select_candidate(state.candidates, *state.selection_index)
            .map(|candidate| candidate.text),
        Some("くすうに".to_string()),
        "rehydration must keep the displayed conservative candidate selected"
    );
}

#[test]
fn move_clause_right_rolls_back_both_snapshots_when_future_sync_desynchronizes() {
    let mut preview = "ある程度な買い文章でもふ".to_string();
    let mut suffix = "くすうに".to_string();
    let mut raw_input = "bunsyoudemofukusuuni".to_string();
    let mut raw_hiragana = "ぶんしょうでもふくすうに".to_string();
    let mut fixed_prefix = "ある程度な買い".to_string();
    let mut corresponding_count = 13;
    let mut selection_index = 0;
    let original_candidates = candidates(
        &["文章でもふ"],
        &["くすうに"],
        "ぶんしょうでもふくすうに",
        &[13],
    );
    let mut current_candidates = original_candidates.clone();
    let mut clause_snapshots = Vec::new();
    let future_snapshot = TextServiceFactory::build_conservative_future_clause_snapshot(
        "くすうに",
        "",
        "kusuuni",
        "くすうに",
        7,
    );
    let mut future_clause_snapshots = vec![future_snapshot.clone()];
    let mut current_clause_is_split_derived = true;
    let mut current_clause_is_direct_split_remainder = false;
    let mut current_clause_is_pending_remainder = false;
    let mut current_clause_has_split_left_neighbor = true;
    let mut current_clause_right_boundary_displacement = 1;
    let mut current_clause_right_boundary_origin = None;
    let mut current_clause_split_group_id = Some(1);
    let mut current_clause_consumed_prefix_restore = None;
    let mut current_clause_remainder_origin = None;
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
        current_clause_is_pending_remainder: &mut current_clause_is_pending_remainder,
        current_clause_has_split_left_neighbor: &mut current_clause_has_split_left_neighbor,
        current_clause_right_boundary_displacement: &mut current_clause_right_boundary_displacement,
        current_clause_right_boundary_origin: &mut current_clause_right_boundary_origin,
        current_clause_split_group_id: &mut current_clause_split_group_id,
        current_clause_consumed_prefix_restore: &mut current_clause_consumed_prefix_restore,
        current_clause_remainder_origin: &mut current_clause_remainder_origin,
        next_split_group_id: &mut next_split_group_id,
    };
    let mut backend = DesynchronizedFutureRestoreBackend {
        shrink_candidates: Candidates {
            hiragana: "くすうに".to_string(),
            ..Candidates::default()
        },
        navigation_candidates: candidates(&["複数"], &["に"], "くすうに", &[5]),
        snapshot_depth: 0,
        pop_count: 0,
        cursor_move_attempted: false,
    };

    let effect = ClauseState::apply_move_clause(&mut state, &mut backend, 1)
        .expect("a desynchronized future restore should roll back transactionally");

    assert!(!effect.applied);
    assert!(!effect.server_reset);
    assert_eq!(state.preview, "ある程度な買い文章でもふ");
    assert_eq!(state.suffix, "くすうに");
    assert_eq!(state.raw_input, "bunsyoudemofukusuuni");
    assert_eq!(state.raw_hiragana, "ぶんしょうでもふくすうに");
    assert_eq!(state.fixed_prefix, "ある程度な買い");
    assert_eq!(*state.corresponding_count, 13);
    assert_eq!(*state.selection_index, 0);
    assert_eq!(state.candidates, &original_candidates);
    assert!(state.clause_snapshots.is_empty());
    assert_eq!(state.future_clause_snapshots, &[future_snapshot]);
    assert_eq!(backend.snapshot_depth, 0);
    assert_eq!(backend.pop_count, 1);
}

#[test]
fn future_snapshot_rehydration_rejects_candidates_from_a_different_boundary() {
    let snapshot = TextServiceFactory::build_conservative_future_clause_snapshot(
        "くすうに",
        "",
        "kusuuni",
        "くすうに",
        7,
    );
    let live_candidates = candidates(&["複数", "服数"], &["に", "に"], "くすうに", &[5, 5]);

    let hydrated = TextServiceFactory::rehydrate_future_clause_snapshot_candidates(
        &snapshot,
        &live_candidates,
    );

    assert_eq!(hydrated.candidates.texts, vec!["くすうに"]);
    assert_eq!(hydrated.candidates.corresponding_count, vec![7]);
}

#[test]
fn future_snapshot_rehydration_rejects_candidates_from_a_different_composition() {
    let snapshot = TextServiceFactory::build_conservative_future_clause_snapshot(
        "くすうに",
        "",
        "kusuuni",
        "くすうに",
        7,
    );
    let live_candidates = candidates(&["複数に"], &[""], "べつのぶん", &[7]);

    let hydrated = TextServiceFactory::rehydrate_future_clause_snapshot_candidates(
        &snapshot,
        &live_candidates,
    );

    assert_eq!(hydrated.candidates.texts, vec!["くすうに"]);
}

#[test]
fn production_future_snapshot_match_requires_server_raw_input_identity() {
    let snapshots = vec![
        TextServiceFactory::build_conservative_future_clause_snapshot(
            "くすうに",
            "",
            "kusuuni",
            "くすうに",
            7,
        ),
    ];
    let server_candidates = candidates(&["複数に"], &[""], "くすうに", &[7]);

    assert!(TextServiceFactory::future_snapshot_matches_server(
        &snapshots,
        Some("kusuuni"),
        &server_candidates
    ));
    assert!(!TextServiceFactory::future_snapshot_matches_server(
        &snapshots,
        Some("different"),
        &server_candidates
    ));
    assert!(
        TextServiceFactory::future_snapshot_matches_server(&snapshots, None, &server_candidates),
        "non-production backends without a raw-input contract retain legacy matching"
    );
}

struct BoundaryCursorBackend {
    positions: Vec<Candidates>,
    index: usize,
    offsets: Vec<i32>,
}

impl ClauseActionBackend for BoundaryCursorBackend {
    fn move_cursor(&mut self, offset: i32) -> anyhow::Result<Candidates> {
        self.offsets.push(offset);
        if offset < 0 {
            self.index = self.index.saturating_sub(1);
        } else if offset > 0 {
            self.index = (self.index + 1).min(self.positions.len().saturating_sub(1));
        }
        Ok(self.positions[self.index].clone())
    }

    fn shrink_text(&mut self, _offset: i32) -> anyhow::Result<Candidates> {
        unreachable!("the synchronization test does not shrink the composition")
    }
}

#[test]
fn future_snapshot_sync_does_not_move_an_already_exact_server_boundary() {
    let target = candidates(&["複数に"], &[""], "くすうに", &[7]);
    let mut backend = BoundaryCursorBackend {
        positions: vec![target.clone()],
        index: 0,
        offsets: Vec::new(),
    };
    let mut current = target;

    let synchronized = TextServiceFactory::sync_backend_current_clause_to_target(
        &mut backend,
        &mut current,
        "くすうに",
        "",
        7,
    )
    .expect("an exact boundary should need no synchronization");

    assert_eq!(synchronized, ClauseBoundarySync::Synchronized);
    assert!(backend.offsets.is_empty());
}

#[test]
fn future_snapshot_sync_moves_a_shorter_server_boundary_to_the_exact_target() {
    let shorter = candidates(&["複数"], &["に"], "くすうに", &[5]);
    let target = candidates(
        &["複数に", "服数に", "ふくすうに"],
        &["", "", ""],
        "くすうに",
        &[7, 7, 7],
    );
    let mut backend = BoundaryCursorBackend {
        positions: vec![shorter.clone(), target.clone()],
        index: 0,
        offsets: Vec::new(),
    };
    let mut current = shorter;

    let synchronized = TextServiceFactory::sync_backend_current_clause_to_target(
        &mut backend,
        &mut current,
        "くすうに",
        "",
        7,
    )
    .expect("synchronization should complete");

    assert_eq!(synchronized, ClauseBoundarySync::Synchronized);
    assert_eq!(current, target);
    assert_eq!(
        backend.offsets,
        vec![1, 0],
        "a shorter server boundary must advance instead of being accepted as synchronized"
    );
}

#[test]
fn future_snapshot_sync_moves_a_longer_server_boundary_to_the_exact_target() {
    let target = candidates(&["複数"], &["に"], "くすうに", &[5]);
    let longer = candidates(&["複数に"], &[""], "くすうに", &[7]);
    let mut backend = BoundaryCursorBackend {
        positions: vec![target.clone(), longer.clone()],
        index: 1,
        offsets: Vec::new(),
    };
    let mut current = longer;

    let synchronized = TextServiceFactory::sync_backend_current_clause_to_target(
        &mut backend,
        &mut current,
        "くすうに",
        "に",
        5,
    )
    .expect("synchronization should complete");

    assert_eq!(synchronized, ClauseBoundarySync::Synchronized);
    assert_eq!(current, target);
    assert_eq!(backend.offsets, vec![-1, 0]);
}

#[test]
fn future_snapshot_sync_rolls_back_when_the_target_boundary_is_unreachable() {
    let shorter = candidates(&["複数"], &["に"], "くすうに", &[5]);
    let longer = candidates(&["複数に"], &[""], "くすうに", &[7]);
    let mut backend = BoundaryCursorBackend {
        positions: vec![shorter.clone(), longer],
        index: 0,
        offsets: Vec::new(),
    };
    let mut current = shorter.clone();

    let synchronized = TextServiceFactory::sync_backend_current_clause_to_target(
        &mut backend,
        &mut current,
        "くすうに",
        "",
        6,
    )
    .expect("failed synchronization should roll back cleanly");

    assert_eq!(synchronized, ClauseBoundarySync::Unavailable);
    assert_eq!(current, shorter);
    assert_eq!(backend.index, 0);
    assert!(
        backend.offsets.len() <= 8,
        "cycle detection and rollback must stay bounded: {:?}",
        backend.offsets
    );
}

struct EmptyAfterMoveBackend {
    initial: Candidates,
    move_attempted: bool,
}

impl ClauseActionBackend for EmptyAfterMoveBackend {
    fn move_cursor(&mut self, offset: i32) -> anyhow::Result<Candidates> {
        if offset != 0 {
            self.move_attempted = true;
        }
        Ok(if self.move_attempted {
            Candidates::default()
        } else {
            self.initial.clone()
        })
    }

    fn shrink_text(&mut self, _offset: i32) -> anyhow::Result<Candidates> {
        unreachable!("the synchronization test does not shrink the composition")
    }
}

#[test]
fn future_snapshot_sync_marks_an_empty_post_move_boundary_as_desynchronized() {
    let initial = candidates(&["複数"], &["に"], "くすうに", &[5]);
    let mut backend = EmptyAfterMoveBackend {
        initial: initial.clone(),
        move_attempted: false,
    };
    let mut current = initial;

    let synchronized = TextServiceFactory::sync_backend_current_clause_to_target(
        &mut backend,
        &mut current,
        "くすうに",
        "",
        7,
    )
    .expect("the logical desynchronization should be reported as an outcome");

    assert_eq!(synchronized, ClauseBoundarySync::BackendDesynchronized);
}

#[test]
fn auto_split_prepared_snapshot_preserves_rich_candidates_for_exact_two_clause_suffix() {
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
    let mut current_clause_consumed_prefix_restore = None;
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
        current_clause_is_pending_remainder: &mut false,
        current_clause_has_split_left_neighbor: &mut current_clause_has_split_left_neighbor,
        current_clause_right_boundary_displacement: &mut 0,
        current_clause_right_boundary_origin: &mut None,
        current_clause_split_group_id: &mut current_clause_split_group_id,
        current_clause_consumed_prefix_restore: &mut current_clause_consumed_prefix_restore,
        current_clause_remainder_origin: &mut None,
        next_split_group_id: &mut next_split_group_id,
    };
    let suffix_candidates = candidates(
        &["統一しろ", "統一し路", "とういつしろ", "統一白"],
        &["", "", "", ""],
        "とういつしろ",
        &[10, 10, 10, 10],
    );

    TextServiceFactory::rebuild_future_clause_snapshots_from_prepared(
        &mut state,
        vec![ClauseAdvance {
            shrunk: suffix_candidates.clone(),
            navigation: suffix_candidates,
            raw_input: ClauseAdvanceRawInput::Unverified,
        }],
    )
    .expect("future snapshots should rebuild from the batch response");

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
    let mut current_clause_is_pending_remainder = false;
    let mut current_clause_has_split_left_neighbor = false;
    let mut current_clause_right_boundary_displacement = 0;
    let mut current_clause_right_boundary_origin = None;
    let mut current_clause_split_group_id = None;
    let mut current_clause_consumed_prefix_restore = None;
    let mut current_clause_remainder_origin = None;
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
        &mut current_clause_is_pending_remainder,
        &mut current_clause_has_split_left_neighbor,
        &mut current_clause_right_boundary_displacement,
        &mut current_clause_right_boundary_origin,
        &mut current_clause_split_group_id,
        &mut current_clause_consumed_prefix_restore,
        &mut current_clause_remainder_origin,
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
    let origin: Arc<str> = Arc::from("touitusiro");
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
    future
        .last_mut()
        .expect("pending remainder")
        .remainder_origin = Some(origin.clone());
    let mut current_restore = None;

    TextServiceFactory::maybe_push_split_future_clause_snapshot_with_restore(
        &mut future,
        "touitusiro",
        "とういつしろ",
        3,
        "いつしろ",
        true,
        None,
        &mut current_restore,
        Some(origin),
        true,
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
fn adjust_boundary_round_trip_restores_consumed_converted_future_clause() {
    let split_group_id = 11;
    let mut future = vec![
        actual_future_snapshot("しろ", "", "siro", "しろ", 4),
        actual_future_snapshot("統一", "しろ", "touitusiro", "とういつしろ", 6),
    ];
    future[1].is_split_derived = true;
    future[1].has_split_left_neighbor = true;
    future[1].split_group_id = Some(split_group_id);

    TextServiceFactory::maybe_push_split_future_clause_snapshot(
        &mut future,
        "kagentouitusiro",
        "かげんとういつしろ",
        7,
        "ういつしろ",
        false,
        Some(split_group_id),
    );

    assert_eq!(future.len(), 2);
    assert_eq!(
        future
            .last()
            .map(|snapshot| snapshot.clause_preview.as_str()),
        Some("ういつ")
    );
    assert!(future
        .last()
        .is_some_and(|snapshot| snapshot.consumed_prefix_restore.is_some()));

    TextServiceFactory::maybe_push_split_future_clause_snapshot(
        &mut future,
        "kagentouitusiro",
        "かげんとういつしろ",
        5,
        "とういつしろ",
        false,
        Some(split_group_id),
    );

    assert_eq!(future.len(), 2);
    assert_eq!(
        future
            .last()
            .map(|snapshot| snapshot.clause_preview.as_str()),
        Some("統一")
    );
    assert!(future
        .last()
        .is_some_and(|snapshot| snapshot.consumed_prefix_restore.is_none()));
}

#[test]
fn adjust_boundary_full_consumption_keeps_restore_chain() {
    let split_group_id = 12;
    let mut future = vec![
        actual_future_snapshot("しろ", "", "siro", "しろ", 4),
        actual_future_snapshot("統一", "しろ", "touitusiro", "とういつしろ", 6),
    ];
    let mut current_restore = None;

    TextServiceFactory::maybe_push_split_future_clause_snapshot_with_restore(
        &mut future,
        "kagentouitusiro",
        "かげんとういつしろ",
        11,
        "しろ",
        false,
        Some(split_group_id),
        &mut current_restore,
        None,
        true,
    );

    assert_eq!(future.len(), 1);
    assert_eq!(
        current_restore
            .as_deref()
            .map(|snapshot| snapshot.clause_preview.as_str()),
        Some("統一")
    );

    TextServiceFactory::maybe_push_split_future_clause_snapshot_with_restore(
        &mut future,
        "kagentouitusiro",
        "かげんとういつしろ",
        7,
        "いつしろ",
        false,
        Some(split_group_id),
        &mut current_restore,
        None,
        true,
    );
    assert_eq!(
        future
            .last()
            .map(|snapshot| snapshot.clause_preview.as_str()),
        Some("いつ")
    );
    assert!(current_restore.is_none());

    TextServiceFactory::maybe_push_split_future_clause_snapshot_with_restore(
        &mut future,
        "kagentouitusiro",
        "かげんとういつしろ",
        5,
        "とういつしろ",
        false,
        Some(split_group_id),
        &mut current_restore,
        None,
        true,
    );
    assert_eq!(
        future
            .last()
            .map(|snapshot| snapshot.clause_preview.as_str()),
        Some("統一")
    );
    assert!(current_restore.is_none());
}

#[test]
fn restored_future_rebase_preserves_candidate_specific_prefixes() {
    let mut restored = TextServiceFactory::build_future_clause_snapshot(
        "統一",
        "白",
        "touitusiro",
        "とういつしろ",
        "",
        6,
        0,
        &candidates(
            &["統一", "統一し"],
            &["白", "ろ白"],
            "とういつしろ",
            &[6, 8],
        ),
    );
    let downstream = actual_future_snapshot("路", "", "ro", "ろ", 2);

    TextServiceFactory::rebase_restored_future_snapshot(&mut restored, Some(&downstream));

    assert_eq!(restored.suffix, "路");
    assert_eq!(restored.selected_sub_text, "路");
    assert_eq!(restored.candidates.sub_texts, vec!["路", "ろ路"]);
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
fn boundary_restore_selection_reset_preserves_non_candidate_display_override() {
    let mut snapshot = TextServiceFactory::build_future_clause_snapshot(
        "加減",
        "統一しろ",
        "kagentouitusiro",
        "かげんとういつしろ",
        "",
        5,
        0,
        &candidates(&["加減"], &["統一しろ"], "かげんとういつしろ", &[5]),
    );
    snapshot.clause_preview = "カゲン".to_string();
    snapshot.selected_text = "カゲン".to_string();

    TextServiceFactory::reset_boundary_restored_snapshot_selection(&mut snapshot);

    assert_eq!(snapshot.clause_preview, "カゲン");
    assert_eq!(snapshot.selected_text, "カゲン");
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
