use windows::Win32::UI::TextServices::{
    ITfContext, ITfDocumentMgr, ITfThreadFocusSink_Impl, ITfThreadMgrEventSink_Impl,
};

use anyhow::Result;

use crate::engine::{
    client_action::ClientAction,
    composition::CompositionState,
    state::{keyboard_disabled_from_context, IMEState},
};

use super::factory::{TextServiceFactory, TextServiceFactory_Impl};

impl TextServiceFactory {
    pub fn set_keyboard_disabled_state(&self, disabled: bool) -> Result<()> {
        let changed = {
            let mut state = IMEState::get()?;
            let changed = state.keyboard_disabled != disabled;
            state.keyboard_disabled = disabled;

            if disabled {
                if let Some(mut ipc_service) = state.ipc_service.clone() {
                    let _ = ipc_service.hide_window();
                    let _ = ipc_service.set_candidates(vec![]);
                    state.ipc_service = Some(ipc_service);
                }
            }

            changed
        };

        if changed {
            self.update_lang_bar()?;
        }

        Ok(())
    }

    pub(crate) fn set_keyboard_disabled_for_document_mgr(
        &self,
        focus: Option<&ITfDocumentMgr>,
    ) -> Result<()> {
        let disabled = match focus {
            Some(focus) => unsafe {
                focus
                    .GetTop()
                    .map(|context| keyboard_disabled_from_context(&context))
                    .unwrap_or(true)
            },
            None => true,
        };

        self.set_keyboard_disabled_state(disabled)
    }
}

impl ITfThreadMgrEventSink_Impl for TextServiceFactory_Impl {
    #[macros::anyhow]
    fn OnInitDocumentMgr(&self, _pdim: Option<&ITfDocumentMgr>) -> Result<()> {
        Ok(())
    }

    #[macros::anyhow]
    fn OnUninitDocumentMgr(&self, _pdim: Option<&ITfDocumentMgr>) -> Result<()> {
        Ok(())
    }

    #[macros::anyhow]
    fn OnSetFocus(
        &self,
        focus: Option<&ITfDocumentMgr>,
        _prevfocus: Option<&ITfDocumentMgr>,
    ) -> Result<()> {
        // if focus is changed, the text layout sink should be updated
        if let Some(focus) = focus {
            self.borrow_mut()?.advise_text_layout_sink(focus.clone())?;
        }
        self.set_keyboard_disabled_for_document_mgr(focus)?;

        let actions = vec![ClientAction::EndComposition];
        self.handle_action(&actions, CompositionState::None)?;

        if focus.is_none() {
            let mut text_service = self.borrow_mut()?;
            text_service.context = None;
            let _ = text_service.unadvise_text_layout_sink();
        }

        Ok(())
    }

    #[macros::anyhow]
    fn OnPushContext(&self, _pic: Option<&ITfContext>) -> Result<()> {
        Ok(())
    }

    #[macros::anyhow]
    fn OnPopContext(&self, _pic: Option<&ITfContext>) -> Result<()> {
        Ok(())
    }
}

impl ITfThreadFocusSink_Impl for TextServiceFactory_Impl {
    #[macros::anyhow]
    fn OnSetThreadFocus(&self) -> Result<()> {
        let focus = {
            let text_service = self.borrow()?;
            let thread_mgr = text_service.thread_mgr()?;
            unsafe { thread_mgr.GetFocus().ok() }
        };
        self.set_keyboard_disabled_for_document_mgr(focus.as_ref())?;

        Ok(())
    }

    #[macros::anyhow]
    fn OnKillThreadFocus(&self) -> Result<()> {
        self.set_keyboard_disabled_state(true)?;

        Ok(())
    }
}
