//! In-process candidate UI for AppContainer TSF hosts. Ordinary desktop hosts keep the
//! existing out-of-process window route. All entry points run on the activating TSF thread.

use std::{
    cell::{Cell, RefCell},
    mem::size_of,
    rc::Rc,
    sync::OnceLock,
    time::{Duration, Instant},
};

use shared::proto::WindowPosition;
use windows::core::{Interface, Result};
use windows::{
    core::Error,
    Win32::{
        Foundation::{CloseHandle, BOOL, E_FAIL, HANDLE, HWND},
        Security::{GetTokenInformation, TokenIsAppContainer, TOKEN_QUERY},
        System::Threading::{GetCurrentProcess, OpenProcessToken},
        UI::TextServices::{
            ITfCandidateListUIElement, ITfDocumentMgr, ITfThreadMgr, ITfUIElement, ITfUIElementMgr,
            TF_CLUIE_COUNT, TF_CLUIE_CURRENTPAGE, TF_CLUIE_DOCUMENTMGR, TF_CLUIE_PAGEINDEX,
            TF_CLUIE_SELECTION, TF_CLUIE_STRING,
        },
    },
};

use super::{candidate_element::CandidateElement, candidate_window::CandidateWindow};
const ALL_FLAGS: u32 = TF_CLUIE_DOCUMENTMGR
    | TF_CLUIE_COUNT
    | TF_CLUIE_SELECTION
    | TF_CLUIE_STRING
    | TF_CLUIE_PAGEINDEX
    | TF_CLUIE_CURRENTPAGE;

#[derive(Clone, Debug)]
pub(crate) struct Snapshot {
    pub visible: bool,
    pub position: Option<WindowPosition>,
    pub candidates: Vec<String>,
    pub selected_index: usize,
    pub reading: String,
    pub candidate_list_visible: bool,
    pub reading_vertical_adjustment: i32,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            visible: false,
            position: None,
            candidates: Vec::new(),
            selected_index: 0,
            reading: String::new(),
            candidate_list_visible: true,
            reading_vertical_adjustment: 0,
        }
    }
}

impl Snapshot {
    fn fallback_request(
        &self,
        input_mode: Option<&str>,
    ) -> shared::proto::UpdateCandidateWindowRequest {
        shared::proto::UpdateCandidateWindowRequest {
            visible: Some(self.visible),
            position: self.position,
            candidates: Some(shared::proto::CandidateList {
                candidates: self.candidates.clone(),
            }),
            selected_index: Some(self.selected_index.min(i32::MAX as usize) as i32),
            input_mode: input_mode.map(str::to_owned),
            reading: Some(self.reading.clone()),
            candidate_list_visible: Some(self.candidate_list_visible),
            reading_vertical_adjustment: Some(self.reading_vertical_adjustment),
        }
    }

    fn reading_only(&self) -> Self {
        Self {
            visible: self.visible,
            position: self.position,
            reading: self.reading.clone(),
            reading_vertical_adjustment: self.reading_vertical_adjustment,
            candidate_list_visible: false,
            ..Self::default()
        }
    }

    fn list_active(&self) -> bool {
        self.visible && self.candidate_list_visible && !self.candidates.is_empty()
    }

    fn popup_active(&self) -> bool {
        self.visible && self.position.is_some() && (self.list_active() || !self.reading.is_empty())
    }

    #[allow(clippy::too_many_arguments)]
    fn merge(
        &mut self,
        visible: Option<bool>,
        position: Option<&WindowPosition>,
        candidates: Option<&[String]>,
        selected_index: Option<i32>,
        reading: Option<&str>,
        candidate_list_visible: Option<bool>,
        reading_vertical_adjustment: Option<i32>,
    ) -> (u32, bool) {
        let mut flags = 0;
        let mut changed = false;
        if let Some(value) = visible {
            changed |= self.visible != value;
            self.visible = value;
        }
        if let Some(value) = position {
            if self.position.as_ref() != Some(value) {
                self.position = Some(*value);
                changed = true;
            }
        }
        if let Some(value) = candidates {
            if self.candidates != value {
                if self.candidates.len() != value.len() {
                    flags |= TF_CLUIE_COUNT;
                }
                if self.candidates.is_empty() != value.is_empty() {
                    flags |= TF_CLUIE_PAGEINDEX;
                }
                self.candidates.clear();
                self.candidates.extend_from_slice(value);
                changed = true;
                flags |= TF_CLUIE_STRING;
            }
        }
        let previous_selection = self.selected_index;
        if let Some(value) = selected_index {
            self.selected_index = value.max(0) as usize;
        }
        if let Some(value) = reading {
            if self.reading != value {
                self.reading.clear();
                self.reading.push_str(value);
                changed = true;
            }
        }
        if let Some(value) = candidate_list_visible {
            changed |= self.candidate_list_visible != value;
            self.candidate_list_visible = value;
        }
        if let Some(value) = reading_vertical_adjustment {
            changed |= self.reading_vertical_adjustment != value;
            self.reading_vertical_adjustment = value;
        }
        if visible == Some(false) && !self.reading.is_empty() {
            self.reading.clear();
            changed = true;
        }
        self.selected_index = self
            .selected_index
            .min(self.candidates.len().saturating_sub(1));
        if previous_selection != self.selected_index {
            flags |= TF_CLUIE_SELECTION;
            changed = true;
        }
        (flags, changed)
    }
}

pub(super) struct ElementState {
    pub(super) snapshot: RefCell<Rc<Snapshot>>,
    pub(super) document_mgr: RefCell<Option<ITfDocumentMgr>>,
    pub(super) pages: RefCell<Vec<u32>>,
    pub(super) reported_flags: Cell<u32>,
    pub(super) show: Cell<bool>,
    pub(super) shown: Cell<bool>,
}

pub(super) fn element_show(state: &Rc<ElementState>, show: bool) -> Result<()> {
    state.show.set(show);
    if let Some(controller) = controller().filter(|c| Rc::ptr_eq(&c.state, state)) {
        controller.dirty.set(true);
        controller.publish()?;
    }
    Ok(())
}

pub(super) fn element_pages_changed(state: &Rc<ElementState>) -> Result<()> {
    if let Some(controller) = controller().filter(|c| Rc::ptr_eq(&c.state, state)) {
        controller
            .pending_flags
            .set(controller.pending_flags.get() | TF_CLUIE_PAGEINDEX | TF_CLUIE_CURRENTPAGE);
        controller.publish()?;
    }
    Ok(())
}

struct Controller {
    manager: ITfUIElementMgr,
    element: ITfCandidateListUIElement,
    state: Rc<ElementState>,
    id: Cell<Option<u32>>,
    window: RefCell<Option<CandidateWindow>>,
    indicator: RefCell<Option<CandidateWindow>>,
    pending_mode: RefCell<Option<(String, Instant)>>,
    window_generation: Cell<u64>,
    dirty: Cell<bool>,
    pending_flags: Cell<u32>,
    busy: Cell<bool>,
    alive: Cell<bool>,
}

impl Controller {
    fn publish(&self) -> Result<()> {
        if self.busy.replace(true) {
            return Ok(());
        }
        // Reentrant Show/update is collapsed into one additional pass. No
        // recursive COM/Win32 calls and no unbounded input-path retry loop.
        let result = (|| {
            self.publish_once()?;
            if self.alive.get() && (self.dirty.get() || self.pending_flags.get() != 0) {
                self.publish_once()?;
            }
            Ok(())
        })();
        let result = result.and_then(|()| self.publish_indicator());
        self.busy.set(false);
        result
    }

    fn owner_window(&self) -> HWND {
        let document = self.state.document_mgr.borrow().clone();
        unsafe {
            document
                .and_then(|doc| doc.GetTop().ok())
                .and_then(|context| context.GetActiveView().ok())
                .and_then(|view| view.GetWnd().ok())
                .unwrap_or_default()
        }
    }

    fn clear_windows(&self) {
        self.window_generation
            .set(self.window_generation.get().wrapping_add(1));
        let window = self.window.borrow_mut().take();
        let indicator = self.indicator.borrow_mut().take();
        self.pending_mode.borrow_mut().take();
        drop(window);
        drop(indicator);
    }

    fn publish_indicator(&self) -> Result<()> {
        let position = self.state.snapshot.borrow().position;
        if position.is_none() || self.pending_mode.borrow().is_none() || !self.alive.get() {
            return Ok(());
        }
        let (mode, requested) = self.pending_mode.borrow_mut().take().unwrap();
        if requested.elapsed() > Duration::from_millis(500) {
            return Ok(());
        }
        let generation = self.window_generation.get();
        let existing = self.indicator.borrow_mut().take();
        let mut window = match existing {
            Some(window) => window,
            None => CandidateWindow::new(self.owner_window())?,
        };
        if !self.alive.get() || generation != self.window_generation.get() {
            return Ok(());
        }
        let result = window.show_mode(position.as_ref().unwrap(), &mode);
        if result.is_err() {
            *self.pending_mode.borrow_mut() = Some((mode, requested));
        }
        if self.alive.get() && generation == self.window_generation.get() {
            *self.indicator.borrow_mut() = Some(window);
        }
        result
    }

    fn publish_once(&self) -> Result<()> {
        if !self.alive.get() {
            return Ok(());
        }
        let snapshot = self
            .state
            .snapshot
            .try_borrow()
            .map_err(|_| Error::from_hresult(E_FAIL))?
            .clone();
        let should_list = snapshot.list_active();
        if snapshot.visible {
            let indicator = self.indicator.borrow_mut().take();
            if let Some(indicator) = indicator {
                indicator.hide();
                if self.alive.get() {
                    *self.indicator.borrow_mut() = Some(indicator);
                }
            }
        }
        let dirty = self.dirty.replace(false);
        let flags = self.pending_flags.replace(0);

        if !should_list {
            if let Some(id) = self.id.take() {
                if let Err(error) = unsafe { self.manager.EndUIElement(id) } {
                    self.id.set(Some(id));
                    self.dirty.set(true);
                    return Err(error);
                }
            }
        }

        if should_list && self.id.get().is_none() {
            self.state.show.set(true);
            self.state.reported_flags.set(ALL_FLAGS);
            let mut show = BOOL::from(true);
            let mut id = 0;
            let ui: ITfUIElement = self.element.cast()?;
            if let Err(error) = unsafe { self.manager.BeginUIElement(&ui, &mut show, &mut id) } {
                self.dirty.set(true);
                self.pending_flags.set(self.pending_flags.get() | flags);
                return Err(error);
            }
            if !self.alive.get() {
                let _ = unsafe { self.manager.EndUIElement(id) };
                return Ok(());
            }
            self.id.set(Some(id));
            if !show.as_bool() {
                self.state.show.set(false);
            }
        } else if should_list && flags != 0 {
            self.state.reported_flags.set(flags);
            let id = self.id.get().ok_or_else(|| Error::from_hresult(E_FAIL))?;
            if let Err(error) = unsafe { self.manager.UpdateUIElement(id) } {
                self.dirty.set(true);
                self.pending_flags.set(self.pending_flags.get() | flags);
                return Err(error);
            }
        }

        // Host callbacks can replace the snapshot during Begin/Update/End.
        // Reading-only mode has no candidate UI element, but still needs a popup.
        let snapshot = self
            .state
            .snapshot
            .try_borrow()
            .map_err(|_| Error::from_hresult(E_FAIL))?
            .clone();
        if !self.alive.get() {
            return Ok(());
        }
        // A host-provided candidate list replaces ours; never display both.
        if !snapshot.popup_active() || (!self.state.show.get() && snapshot.reading.is_empty()) {
            self.state.shown.set(false);
            let window = self.window.borrow_mut().take();
            if let Some(window) = window {
                window.hide();
                if self.alive.get() {
                    *self.window.borrow_mut() = Some(window);
                }
            }
        } else if dirty || !self.state.shown.get() {
            // Take the window out of RefCell before calling user32: callbacks
            // (including Show) may synchronously reenter on this same thread.
            let generation = self.window_generation.get();
            let existing = self.window.borrow_mut().take();
            let mut window = match existing {
                Some(window) => window,
                None => CandidateWindow::new(self.owner_window())?,
            };
            if !self.alive.get() || generation != self.window_generation.get() {
                return Ok(());
            }
            // The host replaces only candidate rows, not the reading header.
            let reading_only;
            let frame = if snapshot.list_active() && !self.state.show.get() {
                reading_only = snapshot.reading_only();
                &reading_only
            } else {
                snapshot.as_ref()
            };
            let result = window.update(frame);
            // Show(false), a hide, or deactivation can arrive inside user32.
            let latest = self
                .state
                .snapshot
                .try_borrow()
                .map_err(|_| Error::from_hresult(E_FAIL))?
                .clone();
            let should_show = self.alive.get()
                && generation == self.window_generation.get()
                && latest.popup_active()
                && (self.state.show.get() || !latest.reading.is_empty());
            if result.is_ok() && !should_show {
                window.hide();
            }
            if self.alive.get() && generation == self.window_generation.get() {
                *self.window.borrow_mut() = Some(window);
                self.state.shown.set(
                    result.is_ok() && should_show && latest.list_active() && self.state.show.get(),
                );
                if result.is_err() {
                    self.dirty.set(true);
                }
            }
            result?;
        }
        Ok(())
    }

    fn close(&self) {
        self.alive.set(false);
        self.state.shown.set(false);
        if let Some(id) = self.id.take() {
            let _ = unsafe { self.manager.EndUIElement(id) };
        }
        // EndUIElement precedes destruction, even if the host reentered us.
        self.clear_windows();
    }
}

thread_local! {
    static CONTROLLER: RefCell<Option<Rc<Controller>>> = const { RefCell::new(None) };
}

fn controller() -> Option<Rc<Controller>> {
    CONTROLLER
        .try_with(|slot| slot.try_borrow().ok().and_then(|slot| slot.clone()))
        .ok()
        .flatten()
}

fn process_is_appcontainer() -> Result<bool> {
    static APP_CONTAINER: OnceLock<std::result::Result<bool, windows::core::HRESULT>> =
        OnceLock::new();
    APP_CONTAINER
        .get_or_init(|| {
            let mut token = HANDLE::default();
            unsafe {
                OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)
                    .map_err(|error| error.code())?;
                let mut value: u32 = 0;
                let mut length = 0;
                let result = GetTokenInformation(
                    token,
                    TokenIsAppContainer,
                    Some((&mut value as *mut u32).cast()),
                    size_of::<u32>() as u32,
                    &mut length,
                );
                let _ = CloseHandle(token);
                result.map_err(|error| error.code())?;
                if length != size_of::<u32>() as u32 {
                    return Err(E_FAIL);
                }
                Ok(value != 0)
            }
        })
        .as_ref()
        .copied()
        .map_err(|code| Error::from_hresult(*code))
}

/// Called on the TSF activation thread. A token-query/cast failure is reported,
/// never mistaken for an active native route.
pub(crate) fn initialize(thread_mgr: &ITfThreadMgr) -> Result<()> {
    if !process_is_appcontainer()? {
        return Ok(());
    }
    // An unsuccessful reactivation must not leave an old controller claiming
    // the native route is ready.
    shutdown();
    let manager = thread_mgr.cast::<ITfUIElementMgr>()?;
    let state = Rc::new(ElementState {
        snapshot: RefCell::new(Rc::new(Snapshot::default())),
        document_mgr: RefCell::new(unsafe { thread_mgr.GetFocus().ok() }),
        pages: RefCell::new(Vec::new()),
        reported_flags: Cell::new(ALL_FLAGS),
        show: Cell::new(true),
        shown: Cell::new(false),
    });
    let module_references = crate::globals::DllModule::get()
        .map_err(|_| Error::from_hresult(E_FAIL))?
        .ref_count
        .clone();
    let element = CandidateElement::create(state.clone(), module_references);
    let controller = Rc::new(Controller {
        manager,
        element,
        state,
        id: Cell::new(None),
        window: RefCell::new(None),
        indicator: RefCell::new(None),
        pending_mode: RefCell::new(None),
        window_generation: Cell::new(0),
        dirty: Cell::new(false),
        pending_flags: Cell::new(ALL_FLAGS),
        busy: Cell::new(false),
        alive: Cell::new(true),
    });
    CONTROLLER.with(|slot| *slot.borrow_mut() = Some(controller));
    Ok(())
}

pub(crate) fn show_input_mode(mode: &str) -> Result<()> {
    if let Some(controller) = controller() {
        *controller.pending_mode.borrow_mut() = Some((mode.to_owned(), Instant::now()));
        controller.publish()?;
    }
    Ok(())
}

pub(crate) fn is_active() -> bool {
    controller().is_some_and(|value| value.alive.get())
}

pub(crate) fn shutdown() {
    let old = CONTROLLER.with(|slot| slot.borrow_mut().take());
    if let Some(old) = old {
        old.close();
    }
}

// A failed native update may have been a selection-only delta. Rehydrate the
// external renderer from the complete merged state before retiring this route.
pub(crate) fn take_fallback_request(
    input_mode: Option<&str>,
) -> Option<shared::proto::UpdateCandidateWindowRequest> {
    let old = CONTROLLER.with(|slot| slot.borrow_mut().take())?;
    let mode = input_mode.map(str::to_owned).or_else(|| {
        old.pending_mode
            .borrow()
            .as_ref()
            .map(|(mode, _)| mode.clone())
    });
    let request = old
        .state
        .snapshot
        .borrow()
        .fallback_request(mode.as_deref());
    old.close();
    Some(request)
}

pub(crate) fn set_document_manager(document_mgr: Option<&ITfDocumentMgr>) -> Result<()> {
    if let Some(controller) = controller() {
        *controller
            .state
            .document_mgr
            .try_borrow_mut()
            .map_err(|_| Error::from_hresult(E_FAIL))? = document_mgr.cloned();
        controller.clear_windows();
        controller
            .pending_flags
            .set(controller.pending_flags.get() | TF_CLUIE_DOCUMENTMGR);
        controller.publish()?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn update(
    visible: Option<bool>,
    position: Option<&WindowPosition>,
    candidates: Option<&[String]>,
    selected_index: Option<i32>,
    reading: Option<&str>,
    candidate_list_visible: Option<bool>,
    reading_vertical_adjustment: Option<i32>,
) -> Result<bool> {
    let Some(controller) = controller() else {
        return Ok(false);
    };
    if !controller.alive.get() {
        return Ok(false);
    }
    let (mut flags, changed) = {
        let mut current = controller
            .state
            .snapshot
            .try_borrow_mut()
            .map_err(|_| Error::from_hresult(E_FAIL))?;
        Rc::make_mut(&mut current).merge(
            visible,
            position,
            candidates,
            selected_index,
            reading,
            candidate_list_visible,
            reading_vertical_adjustment,
        )
    };
    if flags & TF_CLUIE_COUNT != 0 {
        let count = controller.state.snapshot.borrow().candidates.len();
        *controller.state.pages.borrow_mut() =
            (0..count).step_by(5).map(|index| index as u32).collect();
        flags |= TF_CLUIE_PAGEINDEX | TF_CLUIE_CURRENTPAGE;
    }
    if flags & TF_CLUIE_SELECTION != 0 {
        flags |= TF_CLUIE_CURRENTPAGE;
    }
    controller
        .pending_flags
        .set(controller.pending_flags.get() | flags);
    if changed {
        controller.dirty.set(true);
    }
    if changed || flags != 0 || controller.dirty.get() || controller.pending_flags.get() != 0 {
        controller.publish()?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{atomic::AtomicUsize, Arc};
    use windows::{core::implement, Win32::Foundation::E_INVALIDARG};

    #[test]
    fn partial_update_clamps_selection_and_keeps_visibility() {
        let mut snapshot = Snapshot::default();
        assert!(snapshot.candidate_list_visible);
        assert!(!snapshot.list_active());
        let candidates = vec!["あ".to_owned(), "愛".to_owned()];
        let (flags, changed) = snapshot.merge(
            Some(true),
            None,
            Some(&candidates),
            Some(8),
            None,
            None,
            None,
        );
        assert!(snapshot.list_active());
        assert!(changed);
        assert_eq!(snapshot.selected_index, 1);
        assert_eq!(
            flags,
            TF_CLUIE_COUNT | TF_CLUIE_STRING | TF_CLUIE_PAGEINDEX | TF_CLUIE_SELECTION
        );
        assert_eq!(
            snapshot.merge(None, None, Some(&candidates), Some(1), None, None, None),
            (0, false)
        );
        snapshot.merge(None, None, None, Some(-1), Some("あ"), Some(false), None);
        assert_eq!(snapshot.selected_index, 0);
        assert!(!snapshot.list_active());
        snapshot.merge(None, None, None, None, None, Some(true), None);
        assert!(snapshot.list_active());
        snapshot.merge(None, None, Some(&[]), None, None, None, None);
        assert!(!snapshot.list_active());
    }

    #[test]
    fn reading_only_popup_needs_a_position_and_hide_clears_reading() {
        let mut snapshot = Snapshot::default();
        snapshot.merge(
            Some(true),
            None,
            None,
            None,
            Some("あいうえお"),
            Some(false),
            None,
        );
        assert!(!snapshot.list_active());
        assert!(!snapshot.popup_active());
        let position = WindowPosition {
            left: 94,
            top: 914,
            right: 164,
            bottom: 940,
        };
        snapshot.merge(None, Some(&position), None, None, None, None, None);
        assert!(snapshot.popup_active());
        snapshot.merge(Some(false), None, None, None, None, None, None);
        assert!(!snapshot.popup_active());
        assert!(snapshot.reading.is_empty());
        snapshot.merge(Some(true), None, None, None, None, None, None);
        assert!(!snapshot.popup_active());
    }

    #[test]
    fn host_takes_rows_but_reading_and_full_fallback_state_are_preserved() {
        let snapshot = Snapshot {
            visible: true,
            position: Some(WindowPosition {
                left: 90,
                top: 914,
                right: 160,
                bottom: 940,
            }),
            candidates: vec!["候補".into(), "候補2".into()],
            selected_index: 1,
            reading: "こうほ".into(),
            reading_vertical_adjustment: 4,
            ..Snapshot::default()
        };
        let reading_only = snapshot.reading_only();
        assert!(reading_only.popup_active());
        assert!(!reading_only.list_active());
        assert!(reading_only.candidates.is_empty());
        assert_eq!(reading_only.reading, snapshot.reading);
        assert_eq!(reading_only.position, snapshot.position);
        let handoff = snapshot.fallback_request(Some("あ"));
        assert_eq!(handoff.visible, Some(true));
        assert_eq!(handoff.position, snapshot.position);
        assert_eq!(handoff.candidates.unwrap().candidates, snapshot.candidates);
        assert_eq!(handoff.selected_index, Some(1));
        assert_eq!(handoff.reading.as_deref(), Some("こうほ"));
        assert_eq!(handoff.candidate_list_visible, Some(true));
        assert_eq!(handoff.reading_vertical_adjustment, Some(4));
    }

    #[derive(Default)]
    struct HostState {
        events: RefCell<Vec<&'static str>>,
        element: RefCell<Option<ITfUIElement>>,
        fail_update: Cell<bool>,
    }

    #[implement(ITfUIElementMgr)]
    struct TestHost(Rc<HostState>);

    impl windows::Win32::UI::TextServices::ITfUIElementMgr_Impl for TestHost_Impl {
        fn BeginUIElement(
            &self,
            element: Option<&ITfUIElement>,
            show: *mut BOOL,
            id: *mut u32,
        ) -> Result<()> {
            let element = element.unwrap();
            // A real host may query and Show synchronously inside Begin.
            assert_eq!(
                unsafe { element.cast::<ITfCandidateListUIElement>()?.GetCount()? },
                6
            );
            *self.0.element.borrow_mut() = Some(element.clone());
            self.0.events.borrow_mut().push("begin");
            unsafe {
                *show = BOOL(0);
                *id = 7;
                element.Show(false)?;
            }
            Ok(())
        }
        fn UpdateUIElement(&self, id: u32) -> Result<()> {
            assert_eq!(id, 7);
            self.0.events.borrow_mut().push("update");
            if self.0.fail_update.replace(false) {
                return Err(Error::from_hresult(E_FAIL));
            }
            let element = self.0.element.borrow().clone().unwrap();
            unsafe {
                element.Show(false)?;
            }
            Ok(())
        }
        fn EndUIElement(&self, id: u32) -> Result<()> {
            assert_eq!(id, 7);
            self.0.events.borrow_mut().push("end");
            self.0.element.borrow_mut().take();
            Ok(())
        }
        fn GetUIElement(&self, _id: u32) -> Result<ITfUIElement> {
            self.0
                .element
                .borrow()
                .clone()
                .ok_or_else(|| Error::from_hresult(E_INVALIDARG))
        }
        fn EnumUIElements(&self) -> Result<windows::Win32::UI::TextServices::IEnumTfUIElements> {
            Err(Error::from_hresult(windows::Win32::Foundation::E_NOTIMPL))
        }
    }

    #[test]
    fn host_rendering_handles_reentrant_show_and_ends_on_hide() {
        let host = Rc::new(HostState::default());
        let manager: ITfUIElementMgr = TestHost(host.clone()).into();
        let state = Rc::new(ElementState {
            snapshot: RefCell::new(Rc::new(Snapshot::default())),
            document_mgr: RefCell::new(None),
            pages: RefCell::new(Vec::new()),
            reported_flags: Cell::new(0),
            show: Cell::new(true),
            shown: Cell::new(false),
        });
        let element = CandidateElement::create(state.clone(), Arc::new(AtomicUsize::new(0)));
        let controller = Rc::new(Controller {
            manager,
            element: element.clone(),
            state,
            id: Cell::new(None),
            window: RefCell::new(None),
            indicator: RefCell::new(None),
            pending_mode: RefCell::new(None),
            window_generation: Cell::new(0),
            dirty: Cell::new(false),
            pending_flags: Cell::new(0),
            busy: Cell::new(false),
            alive: Cell::new(true),
        });
        CONTROLLER.with(|slot| *slot.borrow_mut() = Some(controller.clone()));
        let candidates = (0..6)
            .map(|index| format!("候補{index}"))
            .collect::<Vec<_>>();
        assert!(update(
            Some(true),
            None,
            Some(&candidates),
            Some(0),
            Some("こうほ"),
            None,
            None
        )
        .unwrap());
        assert_eq!(&*host.events.borrow(), &["begin"]);
        assert!(!controller.state.shown.get());
        assert!(controller.window.borrow().is_none());
        show_input_mode("あ").unwrap();
        assert_eq!(
            controller
                .pending_mode
                .borrow()
                .as_ref()
                .map(|(mode, _)| mode.as_str()),
            Some("あ")
        );
        update(None, None, None, Some(5), None, None, None).unwrap();
        assert_eq!(&*host.events.borrow(), &["begin", "update"]);
        unsafe {
            assert_eq!(element.GetSelection().unwrap(), 5);
            assert_eq!(element.GetCurrentPage().unwrap(), 1);
            let mut starts = [0; 2];
            let mut count = 0;
            element.GetPageIndex(&mut starts, &mut count).unwrap();
            assert_eq!((starts, count), ([0, 5], 2));
            element.SetPageIndex(&[0, 3]).unwrap();
            assert!(element.SetPageIndex(&[0, 6]).is_err());
        }
        update(Some(false), None, None, None, None, None, None).unwrap();
        assert!(controller.state.snapshot.borrow().reading.is_empty());
        assert_eq!(
            &*host.events.borrow(),
            &["begin", "update", "update", "end"]
        );
        // A subsequent manager failure preserves the merged state until the
        // production IPC route takes a full handoff request, not just its delta.
        update(Some(true), None, None, Some(0), None, None, None).unwrap();
        host.fail_update.set(true);
        assert!(update(None, None, None, Some(5), None, None, None).is_err());
        let request = take_fallback_request(Some("A")).unwrap();
        assert_eq!(request.visible, Some(true));
        assert_eq!(request.selected_index, Some(5));
        assert_eq!(request.candidates.unwrap().candidates, candidates);
        assert_eq!(request.candidate_list_visible, Some(true));
        assert_eq!(request.input_mode.as_deref(), Some("A"));
        assert!(!is_active());
        assert!(host.element.borrow().is_none());
    }

    #[test]
    fn page_contract_rejects_invalid_buffers() {
        let state = Rc::new(ElementState {
            snapshot: RefCell::new(Rc::new(Snapshot {
                candidates: vec!["one".into()],
                ..Snapshot::default()
            })),
            document_mgr: RefCell::new(None),
            pages: RefCell::new(vec![0]),
            reported_flags: Cell::new(ALL_FLAGS),
            show: Cell::new(false),
            shown: Cell::new(false),
        });
        let element = CandidateElement::create(state, Arc::new(AtomicUsize::new(0)));
        let mut count = 0;
        let mut first = u32::MAX;
        unsafe {
            element.GetPageIndex(&mut [], &mut count).unwrap();
            assert_eq!(count, 1);
            element
                .GetPageIndex(std::slice::from_mut(&mut first), &mut count)
                .unwrap();
            assert_eq!((first, count), (0, 1));
            assert!(element.SetPageIndex(&[1]).is_err());
            element.SetPageIndex(&[0]).unwrap();
            assert!(element.GetString(1).is_err());
            assert_eq!(element.GetCurrentPage().unwrap(), 0);
        }
    }
}
