use std::{
    cell::{Ref, RefCell, RefMut},
    collections::HashMap,
    time::{Duration, Instant},
};

use windows::{
    core::{Interface, GUID},
    Win32::UI::TextServices::{ITfContext, ITfTextInputProcessor, ITfThreadMgr},
};

use anyhow::{Context, Result};

use crate::engine::{composition::Composition, input_mode::InputMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UpdatePosState {
    #[default]
    Idle,
    Updating {
        suppress_layout_until: Instant,
    },
    SuppressingLayoutChange {
        until: Instant,
    },
}

impl UpdatePosState {
    const LAYOUT_CHANGE_SUPPRESSION: Duration = Duration::from_millis(200);

    pub fn try_begin_update(&mut self, now: Instant) -> bool {
        if matches!(self, Self::Updating { .. }) {
            return false;
        }

        *self = Self::Updating {
            suppress_layout_until: now + Self::LAYOUT_CHANGE_SUPPRESSION,
        };

        true
    }

    pub fn finish_update(&mut self, now: Instant) {
        *self = match *self {
            Self::Updating {
                suppress_layout_until,
            } if now <= suppress_layout_until => Self::SuppressingLayoutChange {
                until: suppress_layout_until,
            },
            Self::Updating { .. } => Self::Idle,
            state => state,
        };
    }

    pub fn should_skip_layout_change(&mut self, now: Instant) -> bool {
        match *self {
            Self::Idle => false,
            Self::Updating { .. } => true,
            Self::SuppressingLayoutChange { until } if now <= until => true,
            Self::SuppressingLayoutChange { .. } => {
                *self = Self::Idle;
                false
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CandidateWindowPositionState {
    last_attempt_at: Option<Instant>,
}

impl CandidateWindowPositionState {
    const THROTTLE_INTERVAL: Duration = Duration::from_millis(50);

    pub fn should_throttle(&self, now: Instant) -> bool {
        self.last_attempt_at
            .is_some_and(|last| now.duration_since(last) < Self::THROTTLE_INTERVAL)
    }

    pub fn mark_attempt(&mut self, now: Instant) {
        self.last_attempt_at = Some(now);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CandidateWindowVisibilityState {
    visible: bool,
}

impl CandidateWindowVisibilityState {
    pub fn apply_visibility_update(&mut self, visible: Option<bool>) {
        if let Some(visible) = visible {
            self.visible = visible;
        }
    }

    pub fn should_update_position(&self, update_pos: bool, visible: Option<bool>) -> bool {
        if !update_pos {
            return false;
        }

        match visible {
            Some(true) => true,
            Some(false) => false,
            None => self.visible,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SurroundingTextContextState {
    connection_id: Option<u64>,
    context: String,
}

impl SurroundingTextContextState {
    pub fn should_send(&self, connection_id: u64, context: &str) -> bool {
        self.connection_id != Some(connection_id) || self.context != context
    }

    pub fn remember(&mut self, connection_id: u64, context: &str) {
        self.connection_id = Some(connection_id);
        self.context.clear();
        self.context.push_str(context);
    }
}

#[derive(Default, Debug)]
pub struct TextService {
    pub tid: u32,
    pub thread_mgr: Option<ITfThreadMgr>,
    pub context: Option<ITfContext>,
    pub layout_sink_context: Option<ITfContext>,
    pub composition: RefCell<Composition>,
    pub update_pos_state: UpdatePosState,
    pub candidate_window_position_state: CandidateWindowPositionState,
    pub candidate_window_visibility_state: CandidateWindowVisibilityState,
    pub surrounding_text_context_state: SurroundingTextContextState,
    pub sink_cookies: HashMap<GUID, u32>,
    pub display_attribute_atom: HashMap<GUID, u32>,
    pub mode: InputMode,
    pub this: Option<ITfTextInputProcessor>,
    pub shift_key_down: bool,
}

impl TextService {
    pub fn this<I: Interface>(&self) -> Result<I> {
        if let Some(this) = self.this.as_ref() {
            Ok(this.cast()?)
        } else {
            anyhow::bail!("this is null");
        }
    }

    pub fn thread_mgr(&self) -> Result<ITfThreadMgr> {
        self.thread_mgr.clone().context("Thread manager is null")
    }

    pub fn context<I: Interface>(&self) -> Result<I> {
        let context = self.context.as_ref().context("Context is null")?;
        Ok(context.cast()?)
    }

    pub fn borrow_composition(&self) -> Result<Ref<'_, Composition>> {
        Ok(self.composition.try_borrow()?)
    }

    pub fn borrow_mut_composition(&self) -> Result<RefMut<'_, Composition>> {
        Ok(self.composition.try_borrow_mut()?)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CandidateWindowPositionState, CandidateWindowVisibilityState, SurroundingTextContextState,
    };
    use std::time::{Duration, Instant};

    #[test]
    fn surrounding_text_context_state_resends_after_connection_change() {
        let mut state = SurroundingTextContextState::default();

        assert!(state.should_send(1, "context"));
        state.remember(1, "context");

        assert!(!state.should_send(1, "context"));
        assert!(state.should_send(2, "context"));
        assert!(state.should_send(1, "changed"));
    }

    #[test]
    fn candidate_window_position_state_throttles_recent_attempts() {
        let mut state = CandidateWindowPositionState::default();
        let now = Instant::now();

        assert!(!state.should_throttle(now));
        state.mark_attempt(now);

        assert!(state.should_throttle(now + Duration::from_millis(10)));
        assert!(!state.should_throttle(now + Duration::from_millis(60)));
    }

    #[test]
    fn candidate_window_visibility_state_keeps_none_updates_dependent_on_known_visibility() {
        let mut state = CandidateWindowVisibilityState::default();

        assert!(!state.should_update_position(true, None));
        assert!(state.should_update_position(true, Some(true)));
        assert!(!state.should_update_position(true, Some(false)));
        assert!(!state.should_update_position(false, Some(true)));

        state.apply_visibility_update(Some(true));
        assert!(state.should_update_position(true, None));

        state.apply_visibility_update(Some(false));
        assert!(!state.should_update_position(true, None));
    }
}
