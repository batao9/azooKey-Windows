use anyhow::Result;
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash as _, Hasher as _},
    sync::Arc,
};

use super::{
    ClauseBoundarySync, ClauseSnapshot, Composition, ConsumedPrefixRestore, FutureClauseSnapshot,
    TextServiceFactory,
};
use crate::engine::{
    client_action::SetSelectionType,
    ipc_service::{Candidates, ClauseSnapshotOperation, IPCService},
};
use crate::trace::diagnostic_log;

#[derive(Debug, Clone)]
pub(crate) struct CandidateSelection {
    pub(crate) index: i32,
    pub(crate) text: String,
    pub(crate) sub_text: String,
    pub(crate) hiragana: String,
    pub(crate) corresponding_count: i32,
    pub(crate) candidate_id: u64,
}

#[derive(Debug, Clone)]
pub(crate) enum ClauseAdvanceRawInput {
    Unverified,
    Verified(String),
    Unavailable,
}

#[derive(Debug, Clone)]
pub(crate) struct ClauseAdvance {
    pub(crate) shrunk: Candidates,
    pub(crate) navigation: Candidates,
    pub(crate) raw_input: ClauseAdvanceRawInput,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ClauseBoundaryAdjustment {
    pub(crate) candidates: Candidates,
    pub(crate) adjusted_input_count: Option<i32>,
}

impl ClauseBoundaryAdjustment {
    pub(crate) fn skipped() -> Self {
        Self::default()
    }

    pub(crate) fn applied(candidates: Candidates, adjusted_input_count: i32) -> Self {
        Self {
            candidates,
            adjusted_input_count: Some(adjusted_input_count),
        }
    }
}

pub(crate) trait ClauseActionBackend {
    fn move_cursor(&mut self, offset: i32) -> Result<Candidates>;
    fn shrink_text(&mut self, offset: i32) -> Result<Candidates>;

    fn adjust_clause_boundary(
        &mut self,
        current_input_count: i32,
        direction: i32,
        _expected_raw_input: &str,
        target_raw_hiragana: &str,
        target_sub_text: &str,
        previous_candidates: &Candidates,
    ) -> Result<ClauseBoundaryAdjustment>
    where
        Self: Sized,
    {
        let mut synchronized_candidates = previous_candidates.clone();
        if TextServiceFactory::sync_backend_current_clause_to_target(
            self,
            &mut synchronized_candidates,
            target_raw_hiragana,
            target_sub_text,
            current_input_count,
        )? != ClauseBoundarySync::Synchronized
        {
            return Ok(ClauseBoundaryAdjustment::skipped());
        }

        let _ = self.move_cursor_with_context(direction, &synchronized_candidates)?;
        let boundary_candidates = self.move_cursor(0)?;
        if boundary_candidates.texts.is_empty() {
            if direction < 0 {
                let _ = self.move_cursor_with_context(1, &boundary_candidates)?;
                if let Some(selected) = ClauseState::select_split_left_candidate(
                    previous_candidates,
                    current_input_count,
                ) {
                    return Ok(ClauseBoundaryAdjustment::applied(
                        previous_candidates.clone(),
                        selected.corresponding_count,
                    ));
                }
            }
            return Ok(ClauseBoundaryAdjustment::skipped());
        }

        let adjusted_input_count = (0..boundary_candidates.texts.len())
            .filter_map(|index| {
                TextServiceFactory::select_candidate(&boundary_candidates, index as i32)
            })
            .map(|candidate| candidate.corresponding_count)
            .find(|count| {
                (direction < 0 && *count < current_input_count)
                    || (direction > 0 && *count > current_input_count)
            });
        Ok(match adjusted_input_count {
            Some(count) => ClauseBoundaryAdjustment::applied(boundary_candidates, count),
            None => ClauseBoundaryAdjustment::skipped(),
        })
    }

    fn advance_clause(
        &mut self,
        offset: i32,
        previous_candidates: &Candidates,
    ) -> Result<ClauseAdvance> {
        self.update_composition_snapshot(ClauseSnapshotOperation::Push, previous_candidates)?;
        let shrunk = self.shrink_text_with_context(offset, previous_candidates)?;
        let navigation = self.move_cursor(0)?;
        Ok(ClauseAdvance {
            shrunk,
            navigation,
            raw_input: ClauseAdvanceRawInput::Unverified,
        })
    }

    fn prepare_future_clauses(
        &mut self,
        initial_offset: i32,
        previous_candidates: &Candidates,
    ) -> Result<Vec<ClauseAdvance>> {
        let mut offset = initial_offset;
        let mut previous = previous_candidates.clone();
        let mut advances = Vec::new();
        let mut last_signature = None;
        let mut snapshot_count = 0;
        let max_steps = previous
            .hiragana
            .chars()
            .count()
            .clamp(1, shared::MAX_PREPARED_CLAUSE_ADVANCES);

        for _ in 0..max_steps {
            let advance = self.advance_clause(offset, &previous)?;
            snapshot_count += 1;
            let Some(selected) = TextServiceFactory::select_candidate(&advance.navigation, 0)
            else {
                break;
            };
            let is_last = selected.sub_text.is_empty();
            let signature = (
                advance.navigation.hiragana.clone(),
                selected.corresponding_count,
                selected.sub_text.clone(),
            );
            if last_signature.as_ref() == Some(&signature) {
                break;
            }
            advances.push(advance.clone());
            last_signature = Some(signature);
            offset = selected.corresponding_count;
            previous = advance.navigation;
            if is_last {
                break;
            }
        }

        for _ in 0..snapshot_count {
            self.update_composition_snapshot(ClauseSnapshotOperation::Pop, &previous)?;
        }
        Ok(advances)
    }

    fn move_cursor_with_context(
        &mut self,
        offset: i32,
        _previous_candidates: &Candidates,
    ) -> Result<Candidates> {
        self.move_cursor(offset)
    }

    fn shrink_text_with_context(
        &mut self,
        offset: i32,
        _previous_candidates: &Candidates,
    ) -> Result<Candidates> {
        self.shrink_text(offset)
    }

    fn update_composition_snapshot(
        &mut self,
        _operation: ClauseSnapshotOperation,
        _previous_candidates: &Candidates,
    ) -> Result<()> {
        Ok(())
    }
}

impl ClauseActionBackend for IPCService {
    fn move_cursor(&mut self, offset: i32) -> Result<Candidates> {
        IPCService::move_cursor(self, offset)
    }

    fn shrink_text(&mut self, offset: i32) -> Result<Candidates> {
        IPCService::shrink_text(self, offset)
    }

    fn adjust_clause_boundary(
        &mut self,
        current_input_count: i32,
        direction: i32,
        expected_raw_input: &str,
        _target_raw_hiragana: &str,
        _target_sub_text: &str,
        previous_candidates: &Candidates,
    ) -> Result<ClauseBoundaryAdjustment> {
        IPCService::adjust_clause_boundary(
            self,
            current_input_count,
            direction,
            expected_raw_input,
            previous_candidates,
        )
    }

    fn advance_clause(
        &mut self,
        offset: i32,
        previous_candidates: &Candidates,
    ) -> Result<ClauseAdvance> {
        IPCService::advance_clause(self, offset, previous_candidates)
    }

    fn prepare_future_clauses(
        &mut self,
        initial_offset: i32,
        previous_candidates: &Candidates,
    ) -> Result<Vec<ClauseAdvance>> {
        IPCService::prepare_future_clauses(self, initial_offset, previous_candidates)
    }

    fn move_cursor_with_context(
        &mut self,
        offset: i32,
        previous_candidates: &Candidates,
    ) -> Result<Candidates> {
        IPCService::move_cursor_with_context(self, offset, previous_candidates)
    }

    fn shrink_text_with_context(
        &mut self,
        offset: i32,
        previous_candidates: &Candidates,
    ) -> Result<Candidates> {
        IPCService::shrink_text_with_context(self, offset, previous_candidates)
    }

    fn update_composition_snapshot(
        &mut self,
        operation: ClauseSnapshotOperation,
        previous_candidates: &Candidates,
    ) -> Result<()> {
        IPCService::update_composition_snapshot(self, operation, previous_candidates)
    }
}

pub(crate) struct ClauseActionStateMut<'a> {
    pub(crate) preview: &'a mut String,
    pub(crate) suffix: &'a mut String,
    pub(crate) raw_input: &'a mut String,
    pub(crate) raw_hiragana: &'a mut String,
    pub(crate) fixed_prefix: &'a mut String,
    pub(crate) corresponding_count: &'a mut i32,
    pub(crate) selection_index: &'a mut i32,
    pub(crate) candidates: &'a mut Candidates,
    pub(crate) clause_snapshots: &'a mut Vec<ClauseSnapshot>,
    pub(crate) future_clause_snapshots: &'a mut Vec<FutureClauseSnapshot>,
    pub(crate) current_clause_is_split_derived: &'a mut bool,
    pub(crate) current_clause_is_direct_split_remainder: &'a mut bool,
    pub(crate) current_clause_is_pending_remainder: &'a mut bool,
    pub(crate) current_clause_has_split_left_neighbor: &'a mut bool,
    pub(crate) current_clause_right_boundary_displacement: &'a mut i32,
    pub(crate) current_clause_right_boundary_origin: &'a mut Option<Arc<FutureClauseSnapshot>>,
    pub(crate) current_clause_split_group_id: &'a mut Option<u64>,
    pub(crate) current_clause_consumed_prefix_restore: &'a mut Option<ConsumedPrefixRestore>,
    pub(crate) current_clause_remainder_origin: &'a mut Option<Arc<str>>,
    pub(crate) next_split_group_id: &'a mut u64,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ClauseActionEffect {
    pub(crate) applied: bool,
    pub(crate) update_pos: bool,
    pub(crate) server_reset: bool,
}

impl ClauseActionEffect {
    pub(crate) fn skipped() -> Self {
        Self {
            applied: false,
            update_pos: false,
            server_reset: false,
        }
    }

    pub(crate) fn applied(update_pos: bool) -> Self {
        Self {
            applied: true,
            update_pos,
            server_reset: false,
        }
    }

    pub(crate) fn server_reset() -> Self {
        Self {
            applied: false,
            update_pos: false,
            server_reset: true,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct MoveClauseProgressMarker {
    preview_len: usize,
    suffix_len: usize,
    raw_input_len: usize,
    raw_hiragana_len: usize,
    fixed_prefix_len: usize,
    text_hash: u64,
    corresponding_count: i32,
    selection_index: i32,
    clause_snapshot_count: usize,
    future_clause_snapshot_count: usize,
    current_clause_is_split_derived: bool,
    current_clause_is_direct_split_remainder: bool,
    current_clause_is_pending_remainder: bool,
    current_clause_has_split_left_neighbor: bool,
    current_clause_right_boundary_displacement: i32,
    current_clause_has_right_boundary_origin: bool,
    current_clause_split_group_id: Option<u64>,
}

impl MoveClauseProgressMarker {
    pub(crate) fn from_state(state: &ClauseActionStateMut<'_>) -> Self {
        let mut hasher = DefaultHasher::new();
        state.preview.hash(&mut hasher);
        state.suffix.hash(&mut hasher);
        state.raw_input.hash(&mut hasher);
        state.raw_hiragana.hash(&mut hasher);
        state.fixed_prefix.hash(&mut hasher);

        Self {
            preview_len: state.preview.len(),
            suffix_len: state.suffix.len(),
            raw_input_len: state.raw_input.len(),
            raw_hiragana_len: state.raw_hiragana.len(),
            fixed_prefix_len: state.fixed_prefix.len(),
            text_hash: hasher.finish(),
            corresponding_count: *state.corresponding_count,
            selection_index: *state.selection_index,
            clause_snapshot_count: state.clause_snapshots.len(),
            future_clause_snapshot_count: state.future_clause_snapshots.len(),
            current_clause_is_split_derived: *state.current_clause_is_split_derived,
            current_clause_is_direct_split_remainder: *state
                .current_clause_is_direct_split_remainder,
            current_clause_is_pending_remainder: *state.current_clause_is_pending_remainder,
            current_clause_has_split_left_neighbor: *state.current_clause_has_split_left_neighbor,
            current_clause_right_boundary_displacement: *state
                .current_clause_right_boundary_displacement,
            current_clause_has_right_boundary_origin: state
                .current_clause_right_boundary_origin
                .is_some(),
            current_clause_split_group_id: *state.current_clause_split_group_id,
        }
    }
}

#[allow(dead_code)]
#[derive(Copy, Clone, Debug)]
pub(crate) enum ClauseCommand<'a> {
    StartClauseNavigation,
    MoveBy(i32),
    MoveLeft,
    MoveRight,
    MoveToLast,
    AdjustBoundary(i32),
    AdjustBoundaryLeft,
    AdjustBoundaryRight,
    SetSelection(&'a SetSelectionType),
    CommitAll,
    CommitCurrentAndMoveNext,
    CommitFirst,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ClauseTransitionInput {
    pub(crate) candidates: Option<Candidates>,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ClauseTransition {
    pub(crate) effect: ClauseActionEffect,
}

pub(crate) struct ClauseState;

impl ClauseState {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_composition_parts<'a>(
        preview: &'a mut String,
        suffix: &'a mut String,
        raw_input: &'a mut String,
        raw_hiragana: &'a mut String,
        fixed_prefix: &'a mut String,
        corresponding_count: &'a mut i32,
        selection_index: &'a mut i32,
        candidates: &'a mut Candidates,
        clause_snapshots: &'a mut Vec<ClauseSnapshot>,
        future_clause_snapshots: &'a mut Vec<FutureClauseSnapshot>,
        current_clause_is_split_derived: &'a mut bool,
        current_clause_is_direct_split_remainder: &'a mut bool,
        current_clause_is_pending_remainder: &'a mut bool,
        current_clause_has_split_left_neighbor: &'a mut bool,
        current_clause_right_boundary_displacement: &'a mut i32,
        current_clause_right_boundary_origin: &'a mut Option<Arc<FutureClauseSnapshot>>,
        current_clause_split_group_id: &'a mut Option<u64>,
        current_clause_consumed_prefix_restore: &'a mut Option<ConsumedPrefixRestore>,
        current_clause_remainder_origin: &'a mut Option<Arc<str>>,
        next_split_group_id: &'a mut u64,
    ) -> ClauseActionStateMut<'a> {
        ClauseActionStateMut {
            preview,
            suffix,
            raw_input,
            raw_hiragana,
            fixed_prefix,
            corresponding_count,
            selection_index,
            candidates,
            clause_snapshots,
            future_clause_snapshots,
            current_clause_is_split_derived,
            current_clause_is_direct_split_remainder,
            current_clause_is_pending_remainder,
            current_clause_has_split_left_neighbor,
            current_clause_right_boundary_displacement,
            current_clause_right_boundary_origin,
            current_clause_split_group_id,
            current_clause_consumed_prefix_restore,
            current_clause_remainder_origin,
            next_split_group_id,
        }
    }

    pub(crate) fn write_back(_state: ClauseActionStateMut<'_>) {}

    fn restore_current_clause_snapshot(
        state: &mut ClauseActionStateMut<'_>,
        restored: ClauseSnapshot,
    ) {
        *state.preview = restored.preview;
        *state.suffix = restored.suffix;
        *state.raw_input = restored.raw_input;
        *state.raw_hiragana = restored.raw_hiragana;
        *state.fixed_prefix = restored.fixed_prefix;
        *state.corresponding_count = restored.corresponding_count;
        *state.selection_index = restored.selection_index;
        *state.current_clause_is_split_derived = restored.is_split_derived;
        *state.current_clause_is_direct_split_remainder = restored.is_direct_split_remainder;
        *state.current_clause_is_pending_remainder = restored.is_pending_remainder;
        *state.current_clause_has_split_left_neighbor = restored.has_split_left_neighbor;
        *state.current_clause_right_boundary_displacement = restored.right_boundary_displacement;
        *state.current_clause_right_boundary_origin = restored.right_boundary_origin;
        *state.current_clause_split_group_id = restored.split_group_id;
        *state.current_clause_consumed_prefix_restore = restored.consumed_prefix_restore;
        *state.current_clause_remainder_origin = restored.remainder_origin;
        *state.candidates = restored.candidates;
    }

    pub(crate) fn transition_with_backend<B: ClauseActionBackend>(
        state: &mut ClauseActionStateMut<'_>,
        command: ClauseCommand<'_>,
        input: ClauseTransitionInput,
        backend: &mut B,
    ) -> Result<ClauseTransition> {
        let candidates = input.candidates;
        let effect = match command {
            ClauseCommand::StartClauseNavigation => {
                Self::ensure_clause_navigation_ready(state, backend, candidates)?
            }
            ClauseCommand::MoveBy(direction) => Self::apply_move_clause(state, backend, direction)?,
            ClauseCommand::MoveLeft => Self::apply_move_clause(state, backend, -1)?,
            ClauseCommand::MoveRight => Self::apply_move_clause(state, backend, 1)?,
            ClauseCommand::MoveToLast => {
                Self::apply_move_clause(state, backend, TextServiceFactory::MOVE_CLAUSE_TO_LAST)?
            }
            ClauseCommand::AdjustBoundary(direction) => {
                Self::apply_adjust_boundary(state, backend, direction)?
            }
            ClauseCommand::AdjustBoundaryLeft => Self::apply_adjust_boundary(state, backend, -1)?,
            ClauseCommand::AdjustBoundaryRight => Self::apply_adjust_boundary(state, backend, 1)?,
            ClauseCommand::SetSelection(selection) => Self::apply_set_selection(state, selection),
            ClauseCommand::CommitAll
            | ClauseCommand::CommitCurrentAndMoveNext
            | ClauseCommand::CommitFirst => ClauseActionEffect::skipped(),
        };
        Ok(ClauseTransition { effect })
    }

    pub(crate) fn transition_without_backend(
        state: &mut ClauseActionStateMut<'_>,
        command: ClauseCommand<'_>,
    ) -> ClauseTransition {
        let effect = match command {
            ClauseCommand::SetSelection(selection) => Self::apply_set_selection(state, selection),
            ClauseCommand::CommitAll
            | ClauseCommand::CommitCurrentAndMoveNext
            | ClauseCommand::CommitFirst => ClauseActionEffect::skipped(),
            _ => ClauseActionEffect::skipped(),
        };
        ClauseTransition { effect }
    }

    pub(crate) fn is_active_for_composition(composition: &Composition) -> bool {
        !composition.clause_snapshots.is_empty()
            || !composition.future_clause_snapshots.is_empty()
            || composition.current_clause_is_split_derived
            || composition.current_clause_split_group_id.is_some()
    }

    #[inline]
    pub(crate) fn is_clause_navigation_state_active(state: &ClauseActionStateMut<'_>) -> bool {
        !state.clause_snapshots.is_empty()
            || !state.future_clause_snapshots.is_empty()
            || *state.current_clause_is_split_derived
            || state.current_clause_split_group_id.is_some()
    }

    #[inline]
    pub(crate) fn ensure_clause_navigation_ready<B: ClauseActionBackend>(
        state: &mut ClauseActionStateMut<'_>,
        backend: &mut B,
        candidates: Option<Candidates>,
    ) -> Result<ClauseActionEffect> {
        if ClauseState::is_clause_navigation_state_active(state)
            || state.candidates.texts.is_empty()
            || state.raw_hiragana.is_empty()
        {
            return Ok(ClauseActionEffect::skipped());
        }

        if !state.suffix.is_empty() {
            return Ok(ClauseActionEffect::skipped());
        }

        let navigation_candidates = match candidates {
            Some(candidates) => candidates,
            None => backend.move_cursor(0)?,
        };
        if navigation_candidates.is_empty_composition() {
            return Ok(ClauseActionEffect::server_reset());
        }
        let Some(mut selected) =
            TextServiceFactory::select_navigation_candidate_for_current_preview(
                &navigation_candidates,
                state.preview,
                state.fixed_prefix,
                *state.selection_index,
            )
        else {
            return Ok(ClauseActionEffect::skipped());
        };

        if !TextServiceFactory::candidate_splits_raw_input(&selected, state.raw_input) {
            return Ok(ClauseActionEffect::skipped());
        }

        let display_override_set_type = TextServiceFactory::display_override_set_type(
            state.preview,
            state.fixed_prefix,
            state.raw_input,
            state.raw_hiragana,
        );

        *state.candidates = navigation_candidates;
        *state.selection_index = selected.index;
        *state.corresponding_count = selected.corresponding_count;
        let display_suffix = TextServiceFactory::display_suffix_after_selected_clause(
            state.preview,
            state.fixed_prefix,
            state.suffix,
            &selected,
        );
        selected.sub_text = display_suffix.clone();
        if let Some(sub_text) = state
            .candidates
            .sub_texts
            .get_mut(selected.index.max(0) as usize)
        {
            *sub_text = display_suffix.clone();
        }
        *state.preview =
            TextServiceFactory::merge_preview_with_prefix(state.fixed_prefix, &selected.text);
        *state.suffix = display_suffix;
        *state.raw_hiragana = selected.hiragana.clone();
        let split_group_id = *state.next_split_group_id;
        *state.next_split_group_id += 1;
        *state.current_clause_is_split_derived = true;
        *state.current_clause_is_direct_split_remainder = false;
        *state.current_clause_is_pending_remainder = false;
        *state.current_clause_has_split_left_neighbor = false;
        *state.current_clause_right_boundary_displacement = 0;
        *state.current_clause_right_boundary_origin = None;
        *state.current_clause_split_group_id = Some(split_group_id);
        *state.current_clause_consumed_prefix_restore = None;
        *state.current_clause_remainder_origin = None;
        let prepared =
            backend.prepare_future_clauses(selected.corresponding_count, state.candidates)?;
        TextServiceFactory::rebuild_future_clause_snapshots_from_prepared(state, prepared)?;
        if let Some(set_type) = display_override_set_type {
            let suffix_raw_input = state
                .future_clause_snapshots
                .last()
                .map(|snapshot| snapshot.raw_input.as_str());
            let suffix_raw_hiragana = state
                .future_clause_snapshots
                .last()
                .map(|snapshot| snapshot.raw_hiragana.as_str());
            let (converted_text, converted_sub_text) =
                TextServiceFactory::display_override_split_for_selected_candidate(
                    &set_type,
                    state.raw_input,
                    state.raw_hiragana,
                    &selected,
                    suffix_raw_input,
                    suffix_raw_hiragana,
                );
            selected.text = converted_text;
            selected.sub_text = converted_sub_text;
            let selected_index = selected.index.max(0) as usize;
            if let Some(text) = state.candidates.texts.get_mut(selected_index) {
                *text = selected.text.clone();
            }
            if let Some(sub_text) = state.candidates.sub_texts.get_mut(selected_index) {
                *sub_text = selected.sub_text.clone();
            }
            *state.preview =
                TextServiceFactory::merge_preview_with_prefix(state.fixed_prefix, &selected.text);
            TextServiceFactory::clear_current_learning_candidate_ids(state.candidates);
            for snapshot in state.future_clause_snapshots.iter_mut() {
                TextServiceFactory::clear_current_learning_candidate_ids(&mut snapshot.candidates);
            }
        }
        *state.suffix = TextServiceFactory::sync_current_clause_future_suffix(
            state.candidates,
            *state.selection_index,
            *state.corresponding_count,
            state.future_clause_snapshots,
        );

        Ok(ClauseActionEffect::applied(true))
    }

    #[inline]
    pub(crate) fn apply_move_clause<B: ClauseActionBackend>(
        state: &mut ClauseActionStateMut<'_>,
        backend: &mut B,
        direction: i32,
    ) -> Result<ClauseActionEffect> {
        if direction == TextServiceFactory::MOVE_CLAUSE_TO_LAST {
            let mut applied_any = false;
            loop {
                let before = MoveClauseProgressMarker::from_state(state);
                let effect = ClauseState::apply_move_clause(state, backend, 1)?;
                if effect.server_reset {
                    return Ok(effect);
                }
                if !effect.applied {
                    break;
                }
                let after = MoveClauseProgressMarker::from_state(state);
                if before == after {
                    break;
                }
                applied_any = true;
                if state.suffix.is_empty() {
                    break;
                }
            }

            return Ok(if applied_any {
                ClauseActionEffect::applied(true)
            } else {
                ClauseActionEffect::skipped()
            });
        }

        if direction > 0 {
            if state.suffix.is_empty() {
                return Ok(ClauseActionEffect::skipped());
            }

            let mut snapshot = TextServiceFactory::build_clause_snapshot(
                state.preview,
                state.suffix,
                state.raw_input,
                state.raw_hiragana,
                state.fixed_prefix,
                *state.corresponding_count,
                *state.selection_index,
                *state.current_clause_is_split_derived,
                *state.current_clause_has_split_left_neighbor,
                *state.current_clause_right_boundary_displacement,
                state.current_clause_right_boundary_origin.clone(),
                state.candidates,
            );
            snapshot.split_group_id = *state.current_clause_split_group_id;
            snapshot.is_direct_split_remainder = *state.current_clause_is_direct_split_remainder;
            snapshot.is_pending_remainder = *state.current_clause_is_pending_remainder;
            snapshot.consumed_prefix_restore = state.current_clause_consumed_prefix_restore.clone();
            snapshot.remainder_origin = state.current_clause_remainder_origin.clone();
            let current_clause_preview =
                TextServiceFactory::current_clause_preview(state.preview, state.fixed_prefix);
            let current_corresponding_count = *state.corresponding_count;
            let previous_candidates = state.candidates.clone();

            state.clause_snapshots.push(snapshot.clone());

            let advance =
                backend.advance_clause(current_corresponding_count, &previous_candidates)?;
            let navigation_candidates = advance.navigation;
            let raw_input_identity = advance.raw_input;
            *state.candidates = advance.shrunk;
            if state.candidates.is_empty_composition() {
                return Ok(ClauseActionEffect::server_reset());
            }
            *state.selection_index = 0;
            *state.raw_input = TextServiceFactory::current_raw_input_suffix(
                state.raw_input,
                current_corresponding_count,
            );
            if let ClauseAdvanceRawInput::Verified(server_raw_input) = &raw_input_identity {
                if state.raw_input.as_str() != server_raw_input.as_str() {
                    diagnostic_log(format!(
                        "kind=future-cache\tevent=raw-input-reconcile\tclient_raw_input={}\tserver_raw_input={}",
                        TextServiceFactory::sanitize_log_field(state.raw_input),
                        TextServiceFactory::sanitize_log_field(server_raw_input),
                    ));
                    *state.raw_input = server_raw_input.clone();
                }
            }
            state.fixed_prefix.push_str(&current_clause_preview);

            let materialized_candidates = if navigation_candidates.texts.is_empty() {
                state.candidates.clone()
            } else {
                navigation_candidates.clone()
            };
            if !matches!(raw_input_identity, ClauseAdvanceRawInput::Unavailable)
                && TextServiceFactory::future_snapshot_matches_server(
                    state.future_clause_snapshots,
                    match &raw_input_identity {
                        ClauseAdvanceRawInput::Verified(raw_input) => Some(raw_input.as_str()),
                        ClauseAdvanceRawInput::Unverified | ClauseAdvanceRawInput::Unavailable => {
                            None
                        }
                    },
                    &materialized_candidates,
                )
            {
                if let Some(restored_future) = state.future_clause_snapshots.pop() {
                    *state.candidates = materialized_candidates;
                    match TextServiceFactory::sync_backend_current_clause_to_future_snapshot(
                        backend,
                        state.candidates,
                        &restored_future,
                    )? {
                        ClauseBoundarySync::BackendDesynchronized => {
                            let rollback_candidates = state.candidates.clone();
                            backend.update_composition_snapshot(
                                ClauseSnapshotOperation::Pop,
                                &rollback_candidates,
                            )?;
                            state.future_clause_snapshots.push(restored_future);
                            let restored = state.clause_snapshots.pop().unwrap_or(snapshot);
                            Self::restore_current_clause_snapshot(state, restored);
                            diagnostic_log(
                                "kind=future-cache\tevent=sync-desynchronized-rollback".to_string(),
                            );
                            return Ok(ClauseActionEffect::skipped());
                        }
                        ClauseBoundarySync::Unavailable => {
                            state.future_clause_snapshots.clear();
                        }
                        ClauseBoundarySync::Synchronized => {
                            let restored_future =
                                TextServiceFactory::rehydrate_future_clause_snapshot_candidates(
                                    &restored_future,
                                    state.candidates,
                                );
                            TextServiceFactory::restore_future_clause_snapshot(
                                state.preview,
                                state.suffix,
                                state.raw_input,
                                state.raw_hiragana,
                                state.corresponding_count,
                                state.selection_index,
                                state.current_clause_is_split_derived,
                                state.current_clause_is_direct_split_remainder,
                                state.current_clause_is_pending_remainder,
                                state.current_clause_has_split_left_neighbor,
                                state.current_clause_right_boundary_displacement,
                                state.current_clause_right_boundary_origin,
                                state.current_clause_split_group_id,
                                state.current_clause_consumed_prefix_restore,
                                state.current_clause_remainder_origin,
                                state.candidates,
                                state.fixed_prefix,
                                &restored_future,
                            );
                            *state.current_clause_is_pending_remainder = false;
                            *state.suffix = TextServiceFactory::sync_current_clause_future_suffix(
                                state.candidates,
                                *state.selection_index,
                                *state.corresponding_count,
                                state.future_clause_snapshots,
                            );
                            return Ok(ClauseActionEffect::applied(true));
                        }
                    }
                }
            }
            {
                if !state.future_clause_snapshots.is_empty() {
                    state.future_clause_snapshots.clear();
                }

                if state.future_clause_snapshots.is_empty() {
                    if let Some(navigation_selected) = TextServiceFactory::select_candidate(
                        &navigation_candidates,
                        *state.selection_index,
                    ) {
                        let navigation_has_richer_current_candidates =
                            navigation_candidates.texts.len() > state.candidates.texts.len()
                                && ClauseState::candidate_hiragana_matches_current_clause(
                                    state.candidates,
                                    &navigation_candidates,
                                );
                        if TextServiceFactory::candidate_splits_raw_input(
                            &navigation_selected,
                            state.raw_input,
                        ) || navigation_has_richer_current_candidates
                        {
                            *state.candidates = navigation_candidates.clone();
                            *state.selection_index = navigation_selected.index;
                        }
                    }

                    if let Some(raw_hiragana_suffix) =
                        TextServiceFactory::recover_single_n_raw_hiragana_suffix(
                            &snapshot.raw_hiragana,
                            &navigation_candidates.hiragana,
                        )
                    {
                        let raw_input_suffix = TextServiceFactory::current_raw_input_suffix(
                            &snapshot.raw_input,
                            snapshot.corresponding_count,
                        );
                        let repaired =
                            TextServiceFactory::build_conservative_future_clause_snapshot(
                                &snapshot.suffix,
                                "",
                                &raw_input_suffix,
                                &raw_hiragana_suffix,
                                raw_input_suffix.chars().count() as i32,
                            );
                        *state.candidates = repaired.candidates;
                        *state.selection_index = repaired.selection_index;
                    }
                }

                let Some(selected) =
                    TextServiceFactory::select_candidate(state.candidates, *state.selection_index)
                else {
                    let previous_candidates = state.candidates.clone();
                    backend.update_composition_snapshot(
                        ClauseSnapshotOperation::Pop,
                        &previous_candidates,
                    )?;
                    if let Some(restored) = state.clause_snapshots.pop() {
                        Self::restore_current_clause_snapshot(state, restored);
                        return Ok(ClauseActionEffect::applied(true));
                    }
                    return Ok(ClauseActionEffect::skipped());
                };

                *state.current_clause_is_split_derived = false;
                *state.current_clause_is_direct_split_remainder = false;
                *state.current_clause_is_pending_remainder = false;
                *state.current_clause_has_split_left_neighbor = false;
                *state.current_clause_right_boundary_displacement = 0;
                *state.current_clause_right_boundary_origin = None;
                *state.current_clause_split_group_id = None;
                *state.current_clause_consumed_prefix_restore = None;
                *state.current_clause_remainder_origin = None;
                *state.selection_index = selected.index;
                *state.corresponding_count = selected.corresponding_count;
                let display_suffix = TextServiceFactory::display_suffix_after_selected_clause(
                    state.preview,
                    state.fixed_prefix,
                    state.suffix,
                    &selected,
                );
                *state.preview = TextServiceFactory::merge_preview_with_prefix(
                    state.fixed_prefix,
                    &selected.text,
                );
                *state.suffix = display_suffix;
                *state.raw_hiragana = selected.hiragana;
                Ok(ClauseActionEffect::applied(true))
            }
        } else if direction < 0 {
            if let Some(restored) = state.clause_snapshots.pop() {
                TextServiceFactory::push_current_future_clause_snapshot(
                    state.future_clause_snapshots,
                    state.preview,
                    state.suffix,
                    state.raw_input,
                    state.raw_hiragana,
                    state.fixed_prefix,
                    *state.corresponding_count,
                    *state.selection_index,
                    *state.current_clause_is_split_derived,
                    *state.current_clause_is_direct_split_remainder,
                    *state.current_clause_is_pending_remainder,
                    *state.current_clause_has_split_left_neighbor,
                    *state.current_clause_right_boundary_displacement,
                    state.current_clause_right_boundary_origin.clone(),
                    *state.current_clause_split_group_id,
                    state.current_clause_consumed_prefix_restore.clone(),
                    state.current_clause_remainder_origin.clone(),
                    state.candidates,
                );
                let previous_candidates = state.candidates.clone();
                backend.update_composition_snapshot(
                    ClauseSnapshotOperation::Pop,
                    &previous_candidates,
                )?;

                Self::restore_current_clause_snapshot(state, restored);
                Ok(ClauseActionEffect::applied(true))
            } else {
                Ok(ClauseActionEffect::skipped())
            }
        } else {
            Ok(ClauseActionEffect::skipped())
        }
    }

    #[inline]
    fn candidate_hiragana_matches_current_clause(
        current_candidates: &Candidates,
        next_candidates: &Candidates,
    ) -> bool {
        current_candidates.hiragana.is_empty()
            || next_candidates.hiragana == current_candidates.hiragana
            || current_candidates
                .hiragana
                .ends_with(&next_candidates.hiragana)
    }

    #[inline]
    pub(crate) fn apply_adjust_boundary<B: ClauseActionBackend>(
        state: &mut ClauseActionStateMut<'_>,
        backend: &mut B,
        direction: i32,
    ) -> Result<ClauseActionEffect> {
        if direction == 0
            || state.candidates.texts.is_empty()
            || state.raw_input.is_empty()
            || state.raw_hiragana.is_empty()
        {
            return Ok(ClauseActionEffect::skipped());
        }

        let previous_candidates = state.candidates.clone();
        let target_raw_hiragana = state
            .raw_hiragana
            .strip_suffix(state.suffix.as_str())
            .filter(|current| !state.suffix.is_empty() && !current.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                TextServiceFactory::current_clause_raw_hiragana_preview(
                    state.raw_hiragana,
                    *state.corresponding_count,
                    state.future_clause_snapshots,
                )
            });
        let adjustment = backend.adjust_clause_boundary(
            *state.corresponding_count,
            direction,
            state.raw_input,
            &target_raw_hiragana,
            state.suffix,
            &previous_candidates,
        )?;
        let Some(adjusted_input_count) = adjustment.adjusted_input_count else {
            return Ok(ClauseActionEffect::skipped());
        };
        if (direction < 0 && adjusted_input_count >= *state.corresponding_count)
            || (direction > 0 && adjusted_input_count <= *state.corresponding_count)
        {
            Ok(ClauseActionEffect::skipped())
        } else if let Some(selected) = (0..adjustment.candidates.texts.len())
            .filter_map(|index| {
                TextServiceFactory::select_candidate(&adjustment.candidates, index as i32)
            })
            .find(|candidate| candidate.corresponding_count == adjusted_input_count)
        {
            if state.current_clause_right_boundary_origin.is_none() {
                let mut origin = TextServiceFactory::build_future_clause_snapshot(
                    state.preview,
                    state.suffix,
                    state.raw_input,
                    state.raw_hiragana,
                    state.fixed_prefix,
                    *state.corresponding_count,
                    *state.selection_index,
                    state.candidates,
                );
                origin.is_split_derived = *state.current_clause_is_split_derived;
                origin.is_direct_split_remainder = *state.current_clause_is_direct_split_remainder;
                origin.is_pending_remainder = *state.current_clause_is_pending_remainder;
                origin.has_split_left_neighbor = *state.current_clause_has_split_left_neighbor;
                origin.right_boundary_displacement =
                    *state.current_clause_right_boundary_displacement;
                origin.split_group_id = *state.current_clause_split_group_id;
                origin.consumed_prefix_restore =
                    state.current_clause_consumed_prefix_restore.clone();
                origin.remainder_origin = state.current_clause_remainder_origin.clone();
                *state.current_clause_right_boundary_origin = Some(Arc::new(origin));
            }
            *state.candidates = adjustment.candidates;
            Ok(ClauseState::apply_boundary_candidate_selection(
                state, selected, direction,
            ))
        } else {
            Ok(ClauseActionEffect::skipped())
        }
    }

    #[inline]
    pub(crate) fn select_split_left_candidate(
        candidates: &Candidates,
        current_corresponding_count: i32,
    ) -> Option<CandidateSelection> {
        (0..candidates.texts.len())
            .filter_map(|index| TextServiceFactory::select_candidate(candidates, index as i32))
            .filter(|candidate| candidate.corresponding_count < current_corresponding_count)
            .max_by_key(|candidate| candidate.corresponding_count)
    }

    #[inline]
    pub(crate) fn apply_boundary_candidate_selection(
        state: &mut ClauseActionStateMut<'_>,
        selected: CandidateSelection,
        direction: i32,
    ) -> ClauseActionEffect {
        let previous_right_boundary_displacement =
            *state.current_clause_right_boundary_displacement;
        let boundary_origin = state
            .current_clause_remainder_origin
            .clone()
            .or_else(|| {
                state
                    .current_clause_right_boundary_origin
                    .as_deref()
                    .map(TextServiceFactory::future_snapshot_origin)
            })
            .unwrap_or_else(|| Arc::from(state.raw_input.as_str()));
        let adjacent_origin = state
            .future_clause_snapshots
            .last()
            .map(TextServiceFactory::future_snapshot_origin);
        let released_raw_suffix = TextServiceFactory::current_raw_suffix(
            &selected.hiragana,
            selected.corresponding_count,
        );
        let released_remainder_origin = (direction < 0).then(|| {
            if previous_right_boundary_displacement > 0 {
                state
                    .current_clause_consumed_prefix_restore
                    .as_deref()
                    .filter(|restored| {
                        TextServiceFactory::consumed_restore_owns_reappearing_suffix(
                            state.raw_input,
                            &released_raw_suffix,
                            restored,
                        )
                    })
                    .map(TextServiceFactory::future_snapshot_origin)
                    .or(adjacent_origin.clone())
                    .unwrap_or_else(|| Arc::from(state.raw_input.as_str()))
            } else {
                state
                    .current_clause_right_boundary_origin
                    .as_deref()
                    .map(TextServiceFactory::future_snapshot_origin)
                    .unwrap_or_else(|| boundary_origin.clone())
            }
        });
        let split_group_id = (*state.current_clause_split_group_id)
            .or_else(|| {
                state.future_clause_snapshots.last().and_then(|snapshot| {
                    snapshot
                        .is_conservative
                        .then_some(snapshot.split_group_id)
                        .flatten()
                        .or_else(|| {
                            snapshot
                                .has_split_left_neighbor
                                .then_some(snapshot.split_group_id)
                                .flatten()
                        })
                })
            })
            .unwrap_or_else(|| {
                let group_id = *state.next_split_group_id;
                *state.next_split_group_id += 1;
                group_id
            });
        if *state.current_clause_split_group_id != Some(split_group_id) {
            *state.current_clause_right_boundary_displacement = 0;
        }
        *state.current_clause_right_boundary_displacement += direction.signum();
        let next_remainder_origin = if *state.current_clause_right_boundary_displacement > 0
            && adjacent_origin.as_ref() != Some(&boundary_origin)
        {
            None
        } else {
            Some(boundary_origin)
        };
        *state.selection_index = selected.index;
        *state.corresponding_count = selected.corresponding_count;
        *state.preview =
            TextServiceFactory::merge_preview_with_prefix(state.fixed_prefix, &selected.text);
        *state.raw_hiragana = selected.hiragana;
        *state.suffix = selected.sub_text.clone();
        *state.current_clause_split_group_id = Some(split_group_id);
        let allow_bootstrap_without_existing_future = state.future_clause_snapshots.is_empty()
            && !state.clause_snapshots.is_empty()
            && TextServiceFactory::current_raw_suffix(
                state.raw_hiragana,
                *state.corresponding_count,
            )
            .is_empty();
        TextServiceFactory::maybe_push_split_future_clause_snapshot_with_restore(
            state.future_clause_snapshots,
            state.raw_input,
            state.raw_hiragana,
            *state.corresponding_count,
            &selected.sub_text,
            allow_bootstrap_without_existing_future,
            Some(split_group_id),
            state.current_clause_consumed_prefix_restore,
            released_remainder_origin,
            previous_right_boundary_displacement <= 0,
        );
        TextServiceFactory::coalesce_adjacent_pending_remainders(state.future_clause_snapshots);
        *state.current_clause_remainder_origin = next_remainder_origin;
        let split_group_still_active = state
            .future_clause_snapshots
            .iter()
            .any(|snapshot| snapshot.split_group_id == Some(split_group_id));
        *state.current_clause_is_split_derived =
            *state.current_clause_has_split_left_neighbor || split_group_still_active;
        *state.current_clause_is_direct_split_remainder = false;
        *state.current_clause_is_pending_remainder = false;
        *state.current_clause_split_group_id = state
            .current_clause_is_split_derived
            .then_some(split_group_id);
        *state.suffix = TextServiceFactory::sync_current_clause_future_suffix(
            state.candidates,
            *state.selection_index,
            *state.corresponding_count,
            state.future_clause_snapshots,
        );
        TextServiceFactory::sync_clause_snapshot_suffixes(
            state.clause_snapshots,
            state.preview,
            state.suffix,
        );
        ClauseActionEffect::applied(true)
    }

    #[inline]
    pub(crate) fn apply_set_selection(
        state: &mut ClauseActionStateMut<'_>,
        selection: &SetSelectionType,
    ) -> ClauseActionEffect {
        let desired_index = match selection {
            SetSelectionType::Up => *state.selection_index - 1,
            SetSelectionType::Down => *state.selection_index + 1,
            SetSelectionType::Number(number) => *number,
        };

        if let Some(selected) =
            TextServiceFactory::select_candidate(state.candidates, desired_index)
        {
            *state.selection_index = selected.index;
            *state.corresponding_count = selected.corresponding_count;
            *state.preview =
                TextServiceFactory::merge_preview_with_prefix(state.fixed_prefix, &selected.text);
            *state.raw_hiragana = selected.hiragana;
            *state.suffix = TextServiceFactory::sync_current_clause_future_suffix(
                state.candidates,
                *state.selection_index,
                *state.corresponding_count,
                state.future_clause_snapshots,
            );
            TextServiceFactory::sync_clause_snapshot_suffixes(
                state.clause_snapshots,
                state.preview,
                state.suffix,
            );

            ClauseActionEffect::applied(false)
        } else {
            ClauseActionEffect::skipped()
        }
    }
}
