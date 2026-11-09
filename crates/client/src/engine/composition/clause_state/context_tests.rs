use super::*;

struct ContextBackend {
    popped: bool,
    refreshes: usize,
}
impl ClauseActionBackend for ContextBackend {
    fn context_epoch(&self) -> u64 {
        2
    }
    fn move_cursor(&mut self, offset: i32) -> Result<Candidates> {
        panic!("automatic segmentation is not a boundary refresh: {offset}");
    }
    fn refresh_context_candidates(&mut self, input_count: i32) -> Result<Candidates> {
        assert_eq!(input_count, 4);
        assert!(self.popped);
        self.refreshes += 1;
        Ok(Candidates {
            context_epoch: 2,
            explicit_selection: None,
            texts: vec!["箸".into(), "橋".into()],
            sub_texts: vec!["をつかう".into(); 2],
            corresponding_count: vec![4; 2],
            candidate_ids: vec![21, 22],
            hiragana: "はしをつかう".into(),
            canonical_raw_input: Some("hashiwotsukau".into()),
        })
    }
    fn shrink_text(&mut self, _: i32) -> Result<Candidates> {
        unreachable!()
    }
    fn update_composition_snapshot(
        &mut self,
        operation: ClauseSnapshotOperation,
        _: &Candidates,
        _: u64,
    ) -> Result<()> {
        assert!(matches!(operation, ClauseSnapshotOperation::Pop));
        self.popped = true;
        Ok(())
    }
}

fn state(composition: &mut Composition) -> ClauseActionStateMut<'_> {
    ClauseState::from_composition_parts(
        &mut composition.preview,
        &mut composition.suffix,
        &mut composition.raw_input,
        &mut composition.raw_hiragana,
        &mut composition.fixed_prefix,
        &mut composition.corresponding_count,
        &mut composition.selection_index,
        &mut composition.candidates,
        &mut composition.clause_snapshots,
        &mut composition.future_clause_snapshots,
        &mut composition.current_clause_is_split_derived,
        &mut composition.current_clause_is_direct_split_remainder,
        &mut composition.current_clause_is_pending_remainder,
        &mut composition.current_clause_has_split_left_neighbor,
        &mut composition.current_clause_right_boundary_displacement,
        &mut composition.current_clause_right_boundary_origin,
        &mut composition.current_clause_split_group_id,
        &mut composition.current_clause_consumed_prefix_restore,
        &mut composition.current_clause_remainder_origin,
        &mut composition.next_split_group_id,
    )
}

#[test]
fn right_context_change_then_left_navigation_refreshes_rankings_without_changing_input_or_boundaries(
) {
    for explicit_selection in [false, true] {
        let mut composition = Composition {
            preview: if explicit_selection { "端" } else { "橋" }.into(),
            suffix: "をつかう".into(),
            raw_input: "hashiwotsukau".into(),
            raw_hiragana: "はしをつかう".into(),
            corresponding_count: 4,
            selection_index: i32::from(explicit_selection),
            candidates: Candidates {
                context_epoch: 1,
                explicit_selection: None,
                texts: vec!["橋".into(), "端".into()],
                sub_texts: vec!["をつかう".into(); 2],
                corresponding_count: vec![4; 2],
                candidate_ids: vec![11, 12],
                hiragana: "はしをつかう".into(),
                canonical_raw_input: Some("hashiwotsukau".into()),
            },
            ..Composition::default()
        };
        let snapshot = ClauseSnapshot {
            preview: composition.preview.clone(),
            suffix: composition.suffix.clone(),
            raw_input: composition.raw_input.clone(),
            raw_hiragana: composition.raw_hiragana.clone(),
            fixed_prefix: String::new(),
            corresponding_count: 4,
            selection_index: composition.selection_index,
            candidates: composition.candidates.clone(),
            is_split_derived: false,
            is_direct_split_remainder: false,
            is_pending_remainder: false,
            has_split_left_neighbor: false,
            right_boundary_displacement: 0,
            right_boundary_origin: None,
            split_group_id: None,
            consumed_prefix_restore: None,
            remainder_origin: None,
        };
        // Navigate to the following clause, then acknowledge a changed right
        // context. Only its current candidates are fresh; the left snapshot is old.
        composition.clause_snapshots.push(snapshot);
        composition.fixed_prefix = "橋".into();
        composition.preview = "橋を使う".into();
        composition.suffix.clear();
        composition.raw_input = "wotsukau".into();
        composition.raw_hiragana = "をつかう".into();
        composition.corresponding_count = 8;
        composition.selection_index = 0;
        composition.candidates = Candidates {
            context_epoch: 2,
            texts: vec!["を使う".into()],
            sub_texts: vec!["".into()],
            corresponding_count: vec![8],
            hiragana: "をつかう".into(),
            ..Candidates::default()
        };
        let mut backend = ContextBackend {
            popped: false,
            refreshes: 0,
        };
        let transition = ClauseState::transition_with_backend(
            &mut state(&mut composition),
            ClauseCommand::MoveBy(-1),
            ClauseTransitionInput::default(),
            &mut backend,
        )
        .unwrap();
        assert!(transition.effect.applied);
        assert_eq!(backend.refreshes, 1);
        assert_eq!(composition.raw_input, "hashiwotsukau");
        assert_eq!(composition.corresponding_count, 4);
        assert_eq!(composition.suffix, "をつかう");
        assert_eq!(composition.candidates.context_epoch, 2);
        assert_eq!(composition.candidates.texts[0], "箸");
        assert_eq!(
            composition.preview,
            if explicit_selection { "端" } else { "箸" }
        );
        if explicit_selection {
            assert_eq!(
                composition.candidates.candidate_ids[composition.selection_index as usize],
                0
            );
        }
    }
}

struct RankingBackend {
    epoch: u64,
    bridge_first: bool,
    unavailable: bool,
}
impl ClauseActionBackend for RankingBackend {
    fn context_epoch(&self) -> u64 {
        self.epoch
    }
    fn move_cursor(&mut self, _: i32) -> Result<Candidates> {
        // Model the real automatic first-clause contract: this is deliberately
        // NOT the requested full-span raw count4 and must not be used to refresh.
        Ok(Candidates {
            corresponding_count: vec![2],
            texts: vec!["は".into()],
            ..Candidates::default()
        })
    }
    fn shrink_text(&mut self, _: i32) -> Result<Candidates> {
        unreachable!()
    }
    fn refresh_context_candidates(&mut self, input_count: i32) -> Result<Candidates> {
        assert_eq!(input_count, 4);
        if self.unavailable {
            return Ok(Candidates::default());
        }
        Ok(Candidates {
            context_epoch: self.epoch,
            texts: if self.bridge_first {
                vec!["橋".into(), "箸".into()]
            } else {
                vec!["箸".into(), "橋".into()]
            },
            sub_texts: vec!["".into(); 2],
            corresponding_count: vec![4; 2],
            candidate_ids: vec![31, 32],
            hiragana: "はし".into(),
            ..Candidates::default()
        })
    }
}

fn single_clause() -> Composition {
    Composition {
        preview: "箸".into(),
        raw_input: "hashi".into(),
        raw_hiragana: "はし".into(),
        corresponding_count: 4,
        candidates: Candidates {
            context_epoch: 1,
            texts: vec!["箸".into(), "橋".into()],
            sub_texts: vec!["".into(); 2],
            corresponding_count: vec![4; 2],
            hiragana: "はし".into(),
            ..Candidates::default()
        },
        ..Composition::default()
    }
}

#[test]
fn context_refresh_preserves_full_boundary_and_publishes_an_otherwise_skipped_action() {
    let mut composition = single_clause();
    let mut backend = RankingBackend {
        epoch: 2,
        bridge_first: true,
        unavailable: false,
    };
    let effect = ClauseState::transition_with_backend(
        &mut state(&mut composition),
        ClauseCommand::MoveBy(0),
        ClauseTransitionInput::default(),
        &mut backend,
    )
    .unwrap()
    .effect;
    assert!(effect.applied && effect.update_pos);
    assert_eq!(composition.preview, "橋");
    assert_eq!(composition.corresponding_count, 4);
    assert_eq!(composition.raw_input, "hashi");
    backend.epoch = 3;
    backend.unavailable = true;
    let error = ClauseState::transition_with_backend(
        &mut state(&mut composition),
        ClauseCommand::MoveBy(0),
        ClauseTransitionInput::default(),
        &mut backend,
    )
    .unwrap_err();
    assert!(crate::engine::ipc_service::is_non_destructive_ipc_error(
        &error
    ));
    assert_eq!(composition.preview, "橋");
    assert_eq!(composition.raw_input, "hashi");
}

#[test]
fn explicit_selection_survives_reranking_to_zero_and_a_later_context_change() {
    let mut composition = single_clause();
    ClauseState::apply_set_selection(&mut state(&mut composition), &SetSelectionType::Number(1));
    let mut backend = RankingBackend {
        epoch: 2,
        bridge_first: true,
        unavailable: false,
    };
    for epoch in [2, 3] {
        backend.epoch = epoch;
        backend.bridge_first = epoch == 2;
        ClauseState::transition_with_backend(
            &mut state(&mut composition),
            ClauseCommand::MoveBy(0),
            ClauseTransitionInput::default(),
            &mut backend,
        )
        .unwrap();
        assert_eq!(composition.preview, "橋");
        assert_eq!(
            composition
                .candidates
                .explicit_selection
                .as_ref()
                .map(|choice| choice.surface.as_str()),
            Some("橋")
        );
        assert_eq!(composition.selection_index, if epoch == 2 { 0 } else { 1 });
    }
}

#[test]
fn script_choice_after_candidate_selection_survives_context_refresh_and_navigation() {
    use crate::engine::client_action::SetTextType;
    for kind in [
        SetTextType::Hiragana,
        SetTextType::Katakana,
        SetTextType::HalfKatakana,
        SetTextType::FullLatin,
        SetTextType::HalfLatin,
    ] {
        for selected_first in [false, true] {
            let mut composition = single_clause();
            if selected_first {
                ClauseState::apply_set_selection(
                    &mut state(&mut composition),
                    &SetSelectionType::Number(1),
                );
            }
            let surface = TextServiceFactory::converted_clause_preview_text(&kind, "hashi", "はし");
            composition.preview.clone_from(&surface);
            TextServiceFactory::remember_explicit_script_choice(
                &mut composition.candidates,
                &surface,
            );
            let mut backend = RankingBackend {
                epoch: 2,
                bridge_first: true,
                unavailable: false,
            };
            ClauseState::transition_with_backend(
                &mut state(&mut composition),
                ClauseCommand::MoveBy(0),
                ClauseTransitionInput::default(),
                &mut backend,
            )
            .unwrap();
            assert_eq!(composition.preview, surface);
            assert_eq!(
                composition
                    .candidates
                    .explicit_selection
                    .as_ref()
                    .map(|choice| choice.surface.as_str()),
                Some(surface.as_str())
            );
            assert_eq!(
                composition.candidates.candidate_ids[composition.selection_index as usize],
                0
            );
            assert_eq!(composition.raw_input, "hashi");
        }
    }
}
