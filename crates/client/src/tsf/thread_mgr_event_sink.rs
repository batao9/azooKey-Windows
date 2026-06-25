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

fn ensure_ipc_service_for_tsf_event(event: &str) {
    match IMEState::ensure_ipc_service() {
        Ok(true) => tracing::debug!(event, "Initialized IPC service during TSF event"),
        Ok(false) => {}
        Err(error) => tracing::warn!(?error, event, "IPC service is unavailable during TSF event"),
    }
}

impl TextServiceFactory {
    pub fn set_keyboard_disabled_state(&self, disabled: bool) -> Result<()> {
        let (changed, ipc_service) = IMEState::set_keyboard_disabled_and_clone_ipc(disabled)?;

        if let Some(mut ipc_service) = ipc_service {
            if let Ok(delivery) =
                ipc_service.update_candidate_window(Some(false), None, Some(vec![]), Some(0), None)
            {
                self.remember_candidate_window_visibility_if_sent(delivery, Some(false));
            }

            IMEState::set_ipc_service(ipc_service)?;
        }

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
        ensure_ipc_service_for_tsf_event("OnSetFocus");

        let actions = vec![ClientAction::EndComposition];
        if IMEState::ipc_service()?.is_some() {
            self.handle_action(&actions, CompositionState::None)?;
        } else {
            tracing::debug!(
                "Skipping focus composition cleanup because IPC service is unavailable"
            );
        }

        if focus.is_none() {
            let mut text_service = self.borrow_mut()?;
            let _ = text_service.unadvise_text_layout_sink();
            text_service.context = None;
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
        ensure_ipc_service_for_tsf_event("OnSetThreadFocus");
        self.set_keyboard_disabled_for_document_mgr(focus.as_ref())?;

        Ok(())
    }

    #[macros::anyhow]
    fn OnKillThreadFocus(&self) -> Result<()> {
        self.set_keyboard_disabled_state(true)?;

        Ok(())
    }
}
