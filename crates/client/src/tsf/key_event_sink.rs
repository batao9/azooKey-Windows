use crate::globals::GUID_PRESERVED_KEY_EISU_CAPSLOCK_ANY_MODIFIER;

use windows::{
    core::GUID,
    Win32::{
        Foundation::{BOOL, LPARAM, WPARAM},
        UI::TextServices::{ITfContext, ITfKeyEventSink_Impl},
    },
};

use anyhow::Result;

use super::factory::{TextServiceFactory, TextServiceFactory_Impl};

// sink (aka event listener) for key events
impl ITfKeyEventSink_Impl for TextServiceFactory_Impl {
    #[macros::anyhow]
    #[tracing::instrument]
    fn OnTestKeyDown(
        &self,
        pic: Option<&ITfContext>,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Result<BOOL> {
        self.update_shift_key_state(wparam, true);

        // this function checks if the key event will be handled by "OnKeyUp" function
        // so we need to return TRUE if we want to handle the key event
        let result = self.process_key(pic, wparam, lparam)?.is_some();

        Ok(result.into())
    }

    #[macros::anyhow]
    #[tracing::instrument]
    fn OnKeyDown(&self, pic: Option<&ITfContext>, wparam: WPARAM, lparam: LPARAM) -> Result<BOOL> {
        self.update_shift_key_state(wparam, true);

        // this function is called when a key is pressed
        // we can handle key events here
        let result = self.handle_key(pic, wparam, lparam)?;

        Ok(result.into())
    }

    #[macros::anyhow]
    fn OnTestKeyUp(
        &self,
        pic: Option<&ITfContext>,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Result<BOOL> {
        let result = self.process_key_up(pic, wparam, lparam)?.is_some();
        self.update_shift_key_state(wparam, false);
        Ok(result.into())
    }

    #[macros::anyhow]
    fn OnKeyUp(&self, pic: Option<&ITfContext>, wparam: WPARAM, lparam: LPARAM) -> Result<BOOL> {
        let result = self.handle_key_up(pic, wparam, lparam)?;
        self.update_shift_key_state(wparam, false);
        Ok(result.into())
    }

    #[macros::anyhow]
    fn OnPreservedKey(&self, pic: Option<&ITfContext>, rguid: *const GUID) -> Result<BOOL> {
        let Some(rguid) = (unsafe { rguid.as_ref() }) else {
            return Ok(false.into());
        };

        if *rguid != GUID_PRESERVED_KEY_EISU_CAPSLOCK_ANY_MODIFIER {
            return Ok(false.into());
        }

        let handled = self.handle_preserved_eisu_shortcut(pic)?;
        Ok(handled.into())
    }

    #[macros::anyhow]
    fn OnSetFocus(&self, fforeground: BOOL) -> Result<()> {
        if !fforeground.as_bool() {
            self.clear_tracked_modifier_key_state();
            self.set_keyboard_disabled_state(true)?;
        }

        Ok(())
    }
}

impl TextServiceFactory_Impl {
    fn update_shift_key_state(&self, wparam: WPARAM, is_down: bool) {
        if !TextServiceFactory::is_shift_key(wparam) {
            return;
        }

        if let Ok(mut text_service) = self.borrow_mut() {
            text_service.shift_key_down = is_down;
        }
    }

    fn clear_tracked_modifier_key_state(&self) {
        if let Ok(mut text_service) = self.borrow_mut() {
            text_service.shift_key_down = false;
        }
    }
}
