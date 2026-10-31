use shared::proto::{CandidateList, UpdateCandidateWindowRequest, WindowPosition};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CandidateWindowRevision {
    pub server_session_id: u64,
    pub candidate_revision: u64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum CandidateWindowUpdateKind {
    Unconditional,
    Selection(Option<CandidateWindowRevision>),
}

/// Borrow strings until an external UI request actually needs an owned payload.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct CandidateWindowUpdate<'a> {
    pub visible: Option<bool>,
    pub position: Option<WindowPosition>,
    pub candidates: Option<&'a [String]>,
    pub selected_index: Option<i32>,
    pub input_mode: Option<&'a str>,
    pub reading: Option<&'a str>,
    pub candidate_list_visible: Option<bool>,
    pub reading_vertical_adjustment: Option<i32>,
}

impl CandidateWindowUpdate<'_> {
    pub fn to_request(self) -> UpdateCandidateWindowRequest {
        UpdateCandidateWindowRequest {
            visible: self.visible,
            position: self.position,
            candidates: self.candidates.map(|candidates| CandidateList {
                candidates: candidates.to_vec(),
            }),
            selected_index: self.selected_index,
            input_mode: self.input_mode.map(str::to_owned),
            reading: self.reading.map(str::to_owned),
            candidate_list_visible: self.candidate_list_visible,
            reading_vertical_adjustment: self.reading_vertical_adjustment,
        }
    }
}

/// Per-text-service metadata; no duplicate candidate strings. Invalidate before
/// fallible replacement work so failed list B cannot reuse list A's acknowledgement.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CandidateWindowSyncState {
    pub generation: u64,
    pub revision: Option<CandidateWindowRevision>,
    pub visible: bool,
    pub position: Option<WindowPosition>,
    pub candidate_list_visible: bool,
}

impl Default for CandidateWindowSyncState {
    fn default() -> Self {
        Self {
            generation: 0,
            revision: None,
            visible: false,
            position: None,
            candidate_list_visible: true,
        }
    }
}

impl CandidateWindowSyncState {
    pub fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.revision = None;
    }

    pub fn merge(
        &mut self,
        visible: Option<bool>,
        position: Option<WindowPosition>,
        candidate_list_visible: Option<bool>,
    ) {
        if let Some(visible) = visible {
            self.visible = visible;
        }
        if let Some(position) = position {
            self.position = Some(position);
        }
        if let Some(visible) = candidate_list_visible {
            self.candidate_list_visible = visible;
        }
    }

    pub fn acknowledge(&mut self, generation: u64, revision: Option<CandidateWindowRevision>) {
        if self.generation == generation {
            self.revision = revision;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_replacement_cannot_reuse_previous_rows_or_late_acknowledgements() {
        let mut state = CandidateWindowSyncState::default();
        let first = state.generation;
        let revision = Some(CandidateWindowRevision {
            server_session_id: 1,
            candidate_revision: 2,
        });
        state.acknowledge(first, revision);
        state.invalidate();
        state.merge(Some(true), None, Some(false));
        state.acknowledge(first, revision);
        assert_eq!(state.revision, None);
        assert!(state.visible);
        assert!(!state.candidate_list_visible);
        state.acknowledge(state.generation, None);
        assert_eq!(state.revision, None);
    }
}
