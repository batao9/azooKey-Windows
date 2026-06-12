use anyhow::Result;
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash as _, Hasher as _},
};

use super::{ClauseSnapshot, Composition, FutureClauseSnapshot, TextServiceFactory};
use crate::engine::{
    client_action::SetSelectionType,
    ipc_service::{Candidates, IPCService},
};

#[derive(Debug, Clone)]
pub(crate) struct CandidateSelection {
    pub(crate) index: i32,
    pub(crate) text: String,
    pub(crate) sub_text: String,
    pub(crate) hiragana: String,
    pub(crate) corresponding_count: i32,
}

pub(crate) trait ClauseActionBackend {
    fn move_cursor(&mut self, offset: i32) -> Result<Candidates>;
    fn shrink_text(&mut self, offset: i32) -> Result<Candidates>;

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
}

impl ClauseActionBackend for IPCService {
    fn move_cursor(&mut self, offset: i32) -> Result<Candidates> {
        IPCService::move_cursor(self, offset)
    }

    fn shrink_text(&mut self, offset: i32) -> Result<Candidates> {
        IPCService::shrink_text(self, offset)
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
    pub(crate) current_clause_has_split_left_neighbor: &'a mut bool,
    pub(crate) current_clause_split_group_id: &'a mut Option<u64>,
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
    current_clause_has_split_left_neighbor: bool,
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
            current_clause_has_split_left_neighbor: *state.current_clause_has_split_left_neighbor,
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
        current_clause_has_split_left_neighbor: &'a mut bool,
        current_clause_split_group_id: &'a mut Option<u64>,
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
            current_clause_has_split_left_neighbor,
            current_clause_split_group_id,
            next_split_group_id,
        }
    }

    pub(crate) fn write_back(_state: ClauseActionStateMut<'_>) {}

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
        *state.current_clause_is_split_derived = true;
        *state.current_clause_is_direct_split_remainder = false;
        *state.current_clause_has_split_left_neighbor = false;
        *state.current_clause_split_group_id = None;
        TextServiceFactory::rebuild_future_clause_snapshots_from_backend(state, backend)?;
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
                state.candidates,
            );
            snapshot.split_group_id = *state.current_clause_split_group_id;
            snapshot.is_direct_split_remainder = *state.current_clause_is_direct_split_remainder;
            let current_clause_preview =
                TextServiceFactory::current_clause_preview(state.preview, state.fixed_prefix);
            let current_corresponding_count = *state.corresponding_count;
            let previous_candidates = state.candidates.clone();

            let _ = backend.move_cursor_with_context(
                TextServiceFactory::MOVE_CURSOR_PUSH_CLAUSE_SNAPSHOT,
                &previous_candidates,
            )?;
            state.clause_snapshots.push(snapshot);

            *state.candidates = backend
                .shrink_text_with_context(current_corresponding_count, &previous_candidates)?;
            if state.candidates.is_empty_composition() {
                return Ok(ClauseActionEffect::server_reset());
            }
            *state.selection_index = 0;
            *state.raw_input = TextServiceFactory::current_raw_input_suffix(
                state.raw_input,
                current_corresponding_count,
            );
            state.fixed_prefix.push_str(&current_clause_preview);

            if TextServiceFactory::future_snapshot_matches_server(
                state.future_clause_snapshots,
                state.candidates,
            ) {
                if let Some(restored_future) = state.future_clause_snapshots.pop() {
                    TextServiceFactory::sync_backend_current_clause_to_future_snapshot(
                        backend,
                        state.candidates,
                        &restored_future,
                    )?;
                    TextServiceFactory::restore_future_clause_snapshot(
                        state.preview,
                        state.suffix,
                        state.raw_input,
                        state.raw_hiragana,
                        state.corresponding_count,
                        state.selection_index,
                        state.current_clause_is_split_derived,
                        state.current_clause_is_direct_split_remainder,
                        state.current_clause_has_split_left_neighbor,
                        state.current_clause_split_group_id,
                        state.candidates,
                        state.fixed_prefix,
                        &restored_future,
                    );
                    *state.suffix = TextServiceFactory::sync_current_clause_future_suffix(
                        state.candidates,
                        *state.selection_index,
                        *state.corresponding_count,
                        state.future_clause_snapshots,
                    );
                    return Ok(ClauseActionEffect::applied(true));
                }
            } else {
                if !state.future_clause_snapshots.is_empty() {
                    state.future_clause_snapshots.clear();
                }

                if state.future_clause_snapshots.is_empty() {
                    let navigation_candidates = backend.move_cursor(0)?;
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
                            *state.candidates = navigation_candidates;
                            *state.selection_index = navigation_selected.index;
                        }
                    }
                }

                let Some(selected) =
                    TextServiceFactory::select_candidate(state.candidates, *state.selection_index)
                else {
                    let previous_candidates = state.candidates.clone();
                    let _ = backend.move_cursor_with_context(
                        TextServiceFactory::MOVE_CURSOR_POP_CLAUSE_SNAPSHOT,
                        &previous_candidates,
                    )?;
                    if let Some(restored) = state.clause_snapshots.pop() {
                        *state.preview = restored.preview;
                        *state.suffix = restored.suffix;
                        *state.raw_input = restored.raw_input;
                        *state.raw_hiragana = restored.raw_hiragana;
                        *state.fixed_prefix = restored.fixed_prefix;
                        *state.corresponding_count = restored.corresponding_count;
                        *state.selection_index = restored.selection_index;
                        *state.current_clause_is_split_derived = restored.is_split_derived;
                        *state.current_clause_is_direct_split_remainder =
                            restored.is_direct_split_remainder;
                        *state.current_clause_has_split_left_neighbor =
                            restored.has_split_left_neighbor;
                        *state.current_clause_split_group_id = restored.split_group_id;
                        *state.candidates = restored.candidates;
                        return Ok(ClauseActionEffect::applied(true));
                    }
                    return Ok(ClauseActionEffect::skipped());
                };

                *state.current_clause_is_split_derived = false;
                *state.current_clause_is_direct_split_remainder = false;
                *state.current_clause_has_split_left_neighbor = false;
                *state.current_clause_split_group_id = None;
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
                return Ok(ClauseActionEffect::applied(true));
            }

            Ok(ClauseActionEffect::skipped())
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
                    *state.current_clause_has_split_left_neighbor,
                    *state.current_clause_split_group_id,
                    state.candidates,
                );
                let previous_candidates = state.candidates.clone();
                let _ = backend.move_cursor_with_context(
                    TextServiceFactory::MOVE_CURSOR_POP_CLAUSE_SNAPSHOT,
                    &previous_candidates,
                )?;

                *state.preview = restored.preview;
                *state.suffix = restored.suffix;
                *state.raw_input = restored.raw_input;
                *state.raw_hiragana = restored.raw_hiragana;
                *state.fixed_prefix = restored.fixed_prefix;
                *state.corresponding_count = restored.corresponding_count;
                *state.selection_index = restored.selection_index;
                *state.current_clause_is_split_derived = restored.is_split_derived;
                *state.current_clause_is_direct_split_remainder =
                    restored.is_direct_split_remainder;
                *state.current_clause_has_split_left_neighbor = restored.has_split_left_neighbor;
                *state.current_clause_split_group_id = restored.split_group_id;
                *state.candidates = restored.candidates;
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
        if direction == 0 {
            return Ok(ClauseActionEffect::skipped());
        }

        let fallback_candidates = state.candidates.clone();
        if !state.suffix.is_empty() || !state.future_clause_snapshots.is_empty() {
            TextServiceFactory::sync_backend_current_clause_to_target(
                backend,
                state.candidates,
                state.raw_hiragana,
                state.suffix,
                *state.corresponding_count,
            )?;
        }

        let previous_candidates = state.candidates.clone();
        let _ = backend.move_cursor_with_context(direction, &previous_candidates)?;
        let boundary_candidates = backend.move_cursor(0)?;
        if boundary_candidates.texts.is_empty() {
            if direction < 0 {
                let _ = backend.move_cursor_with_context(1, &boundary_candidates)?;
                if let Some(selected) = ClauseState::select_split_left_candidate(
                    &fallback_candidates,
                    *state.corresponding_count,
                ) {
                    *state.candidates = fallback_candidates;
                    return Ok(ClauseState::apply_boundary_candidate_selection(
                        state, selected,
                    ));
                }
            }
            return Ok(ClauseActionEffect::skipped());
        }

        *state.candidates = boundary_candidates;
        if let Some(selected) = TextServiceFactory::select_candidate(state.candidates, 0) {
            Ok(ClauseState::apply_boundary_candidate_selection(
                state, selected,
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
    ) -> ClauseActionEffect {
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
        TextServiceFactory::maybe_push_split_future_clause_snapshot(
            state.future_clause_snapshots,
            state.raw_input,
            state.raw_hiragana,
            *state.corresponding_count,
            &selected.sub_text,
            allow_bootstrap_without_existing_future,
            Some(split_group_id),
        );
        let split_group_still_active = state
            .future_clause_snapshots
            .iter()
            .any(|snapshot| snapshot.split_group_id == Some(split_group_id));
        *state.current_clause_is_split_derived =
            *state.current_clause_has_split_left_neighbor || split_group_still_active;
        *state.current_clause_is_direct_split_remainder = false;
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
