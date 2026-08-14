use crate::globals::GUID_PRESERVED_KEY_EISU_CAPSLOCK_ANY_MODIFIER;

use windows::{
    core::GUID,
    Win32::{
        Foundation::{BOOL, LPARAM, WPARAM},
        UI::TextServices::{ITfContext, ITfKeyEventSink_Impl},
    },
};

use anyhow::Result;

use crate::engine::composition::ReconversionKeyTestResult;

use super::factory::{TextServiceFactory, TextServiceFactory_Impl};

fn resolve_reconversion_key_handling(
    tested_as_owned: bool,
    handling_result: Option<bool>,
) -> Option<bool> {
    tested_as_owned.then_some(true).or(handling_result)
}

fn should_probe_reconversion_on_key(tested_selection: Option<bool>) -> bool {
    tested_selection != Some(false)
}

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

        match self.test_reconversion_key(pic, wparam) {
            Ok(ReconversionKeyTestResult::Owned) => {
                self.remember_reconversion_test_result(wparam, Some(true));
                return Ok(true.into());
            }
            Ok(ReconversionKeyTestResult::MatchedEmpty) => {
                self.remember_reconversion_test_result(wparam, Some(false));
            }
            Ok(ReconversionKeyTestResult::NotHandled) => {
                self.remember_reconversion_test_result(wparam, None);
                return Ok(false.into());
            }
            Ok(ReconversionKeyTestResult::Unrelated) => {
                self.remember_reconversion_test_result(wparam, None);
            }
            Err(error) => {
                self.remember_reconversion_test_result(wparam, None);
                tracing::warn!(
                    ?error,
                    "Reconversion selection probe failed; passing key through"
                );
                return Ok(false.into());
            }
        }

        // this function checks if the key event will be handled by "OnKeyUp" function
        // so we need to return TRUE if we want to handle the key event
        let result = self.process_key(pic, wparam, lparam)?.is_some();

        Ok(result.into())
    }

    #[macros::anyhow]
    #[tracing::instrument]
    fn OnKeyDown(&self, pic: Option<&ITfContext>, wparam: WPARAM, lparam: LPARAM) -> Result<BOOL> {
        self.update_shift_key_state(wparam, true);
        let tested_selection = self.take_reconversion_test_result(wparam);

        // If OnTest already matched Space with an empty selection, do not take a second
        // synchronous edit-session lock. Continue directly through the normal Space path.
        if should_probe_reconversion_on_key(tested_selection) {
            let tested_as_owned = tested_selection == Some(true);
            match self.handle_reconversion_key(pic, wparam) {
                Ok(result) => {
                    if let Some(handled) =
                        resolve_reconversion_key_handling(tested_as_owned, result)
                    {
                        return Ok(handled.into());
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        ?error,
                        tested_as_owned,
                        "Reconversion key handling failed; preserving prior test ownership"
                    );
                    return Ok(tested_as_owned.into());
                }
            }
        }

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
            self.clear_reconversion_test_result();
            self.set_keyboard_disabled_state(true)?;
        }

        Ok(())
    }
}

impl TextServiceFactory_Impl {
    fn remember_reconversion_test_result(&self, wparam: WPARAM, selected: Option<bool>) {
        if let Ok(mut text_service) = self.borrow_mut() {
            text_service.reconversion_test_result = selected.map(|selected| (wparam.0, selected));
        }
    }

    fn take_reconversion_test_result(&self, wparam: WPARAM) -> Option<bool> {
        let Ok(mut text_service) = self.borrow_mut() else {
            return None;
        };
        let result = text_service
            .reconversion_test_result
            .filter(|(key, _)| *key == wparam.0)
            .map(|(_, selected)| selected);
        text_service.reconversion_test_result = None;
        result
    }

    fn clear_reconversion_test_result(&self) {
        if let Ok(mut text_service) = self.borrow_mut() {
            text_service.reconversion_test_result = None;
        }
    }

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

#[cfg(test)]
mod tests {
    use super::{resolve_reconversion_key_handling, should_probe_reconversion_on_key};

    #[test]
    fn key_claimed_after_selection_probe_is_consumed_on_later_failure() {
        assert_eq!(resolve_reconversion_key_handling(true, None), Some(true));
        assert_eq!(
            resolve_reconversion_key_handling(true, Some(false)),
            Some(true)
        );
        assert_eq!(resolve_reconversion_key_handling(false, None), None);
        assert_eq!(
            resolve_reconversion_key_handling(false, Some(false)),
            Some(false)
        );
    }

    #[test]
    fn matched_empty_space_skips_the_second_selection_probe() {
        assert!(!should_probe_reconversion_on_key(Some(false)));
        assert!(should_probe_reconversion_on_key(Some(true)));
        assert!(should_probe_reconversion_on_key(None));
    }
}
