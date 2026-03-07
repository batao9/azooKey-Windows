use std::{
    path::{Path, PathBuf},
    process::Command,
};

use windows::{
    core::{w, IUnknown, Interface as _, BSTR, GUID, PCWSTR},
    Win32::{
        Foundation::{BOOL, E_INVALIDARG, LPARAM, POINT, RECT, WPARAM},
        System::Ole::CONNECT_E_CANNOTCONNECT,
        UI::{
            TextServices::{
                ITfLangBarItemButton_Impl, ITfLangBarItemSink, ITfLangBarItem_Impl, ITfMenu,
                ITfSource_Impl, TfLBIClick, GUID_LBI_INPUTMODE, TF_LANGBARITEMINFO,
                TF_LBI_CLK_LEFT, TF_LBI_CLK_RIGHT, TF_LBI_STYLE_BTN_BUTTON,
            },
            WindowsAndMessaging::{
                AppendMenuW, CreatePopupMenu, DestroyMenu, GetForegroundWindow, LoadImageW,
                PostMessageW, SetForegroundWindow, TrackPopupMenu, HICON, HMENU, IMAGE_ICON,
                LR_DEFAULTCOLOR, MF_STRING, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_NULL,
            },
        },
    },
};

use crate::{
    engine::{
        client_action::ClientAction, composition::CompositionState, input_mode::InputMode,
        state::IMEState, theme::get_theme,
    },
    globals::{DllModule, GUID_TEXT_SERVICE, TEXTSERVICE_LANGBARITEMSINK_COOKIE},
};

use anyhow::{Context as _, Result};

use super::factory::TextServiceFactory_Impl;

const INFO: TF_LANGBARITEMINFO = TF_LANGBARITEMINFO {
    clsidService: GUID_TEXT_SERVICE,
    guidItem: GUID_LBI_INPUTMODE,
    dwStyle: TF_LBI_STYLE_BTN_BUTTON,
    ulSort: 0,
    szDescription: [0; 32],
};

const SETTINGS_MENU_ID: usize = 1;
const SETTINGS_APP_FILENAME: &str = "Azookey.exe";

// you need to implement these three interfaces to create a language bar item
// if not, you will get E_FAIL error in ITfLangBarItemMgr::AddItem

impl TextServiceFactory_Impl {
    fn toggle_input_mode(&self) -> Result<()> {
        let mode = {
            let ime_mode = &IMEState::get()?.input_mode;
            match ime_mode {
                InputMode::Latin => InputMode::Kana,
                InputMode::Kana => InputMode::Latin,
            }
        };

        let actions = vec![ClientAction::SetIMEMode(mode)];
        self.handle_action(&actions, CompositionState::None)?;

        Ok(())
    }

    fn handle_right_click(&self, pt: &POINT) -> Result<()> {
        match show_settings_menu(pt) {
            Ok(Some(command)) if command == SETTINGS_MENU_ID as u32 => {
                if let Err(error) = launch_settings_app() {
                    tracing::warn!(?error, "Failed to launch settings app");
                }
            }
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(?error, "Failed to show settings menu");
            }
        }

        Ok(())
    }
}

impl ITfLangBarItem_Impl for TextServiceFactory_Impl {
    #[macros::anyhow]
    fn GetInfo(&self, p_info: *mut TF_LANGBARITEMINFO) -> Result<()> {
        unsafe {
            *p_info = INFO;
        }
        Ok(())
    }

    #[macros::anyhow]
    fn GetStatus(&self) -> Result<u32> {
        Ok(0)
    }

    #[macros::anyhow]
    fn Show(&self, _f_show: BOOL) -> Result<()> {
        Ok(())
    }

    // this will be shown as a tooltip when you hover the language bar item
    #[macros::anyhow]
    fn GetTooltipString(&self) -> Result<BSTR> {
        Ok(BSTR::default())
    }
}

impl ITfLangBarItemButton_Impl for TextServiceFactory_Impl {
    #[macros::anyhow]
    fn OnClick(&self, click: TfLBIClick, pt: &POINT, _prcarea: *const RECT) -> Result<()> {
        match click {
            TF_LBI_CLK_LEFT => self.toggle_input_mode()?,
            TF_LBI_CLK_RIGHT => self.handle_right_click(pt)?,
            _ => {}
        }

        Ok(())
    }

    // this method should not be called
    #[macros::anyhow]
    fn InitMenu(&self, _pmenu: Option<&ITfMenu>) -> Result<()> {
        Ok(())
    }

    // this method should not be called
    #[macros::anyhow]
    fn OnMenuSelect(&self, _w_id: u32) -> Result<()> {
        Ok(())
    }

    #[macros::anyhow]
    fn GetIcon(&self) -> Result<HICON> {
        let dll_module = DllModule::get()?;
        let state = &IMEState::get()?;
        let input_mode = &state.input_mode;
        let theme = get_theme()?;

        let icon_id = match input_mode {
            InputMode::Kana => {
                if theme {
                    102
                } else {
                    104
                }
            }
            InputMode::Latin => {
                if theme {
                    103
                } else {
                    105
                }
            }
        };

        unsafe {
            let handle = LoadImageW(
                dll_module.hinst.context("Dll instance not found")?,
                PCWSTR(icon_id as *mut u16),
                IMAGE_ICON,
                0,
                0,
                LR_DEFAULTCOLOR,
            )?;

            Ok(HICON(handle.0))
        }
    }

    #[macros::anyhow]
    fn GetText(&self) -> Result<BSTR> {
        Ok(BSTR::default())
    }
}

fn show_settings_menu(pt: &POINT) -> Result<Option<u32>> {
    struct PopupMenu(HMENU);

    impl Drop for PopupMenu {
        fn drop(&mut self) {
            unsafe {
                let _ = DestroyMenu(self.0);
            }
        }
    }

    unsafe {
        let menu = PopupMenu(CreatePopupMenu()?);
        AppendMenuW(menu.0, MF_STRING, SETTINGS_MENU_ID, w!("設定"))?;

        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return Ok(None);
        }

        let _ = SetForegroundWindow(hwnd);

        let selected = TrackPopupMenu(
            menu.0,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
            pt.x,
            pt.y,
            0,
            hwnd,
            None,
        )
        .0 as u32;

        let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));

        if selected == 0 {
            Ok(None)
        } else {
            Ok(Some(selected))
        }
    }
}

fn launch_settings_app() -> Result<()> {
    let dll_path = PathBuf::from(DllModule::get_path()?);
    let settings_path = resolve_settings_app_path(&dll_path)?;
    let install_dir = settings_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .context("Settings app directory not found")?;

    if !settings_path.is_file() {
        anyhow::bail!("Settings app not found: {}", settings_path.display());
    }

    Command::new(&settings_path)
        .current_dir(install_dir)
        .spawn()
        .with_context(|| format!("Failed to spawn {}", settings_path.display()))?;

    Ok(())
}

fn resolve_settings_app_path(dll_path: &Path) -> Result<PathBuf> {
    let install_dir = dll_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .context("DLL parent directory not found")?;

    Ok(install_dir.join(SETTINGS_APP_FILENAME))
}

impl ITfSource_Impl for TextServiceFactory_Impl {
    #[macros::anyhow]
    fn AdviseSink(&self, riid: *const GUID, punk: Option<&IUnknown>) -> Result<u32> {
        let riid = unsafe { *riid };

        if riid != ITfLangBarItemSink::IID {
            return Err(anyhow::Error::new(windows_core::Error::from_hresult(
                E_INVALIDARG,
            )));
        }

        if punk.is_none() {
            return Err(anyhow::Error::new(windows_core::Error::from_hresult(
                E_INVALIDARG,
            )));
        }

        Ok(TEXTSERVICE_LANGBARITEMSINK_COOKIE)
    }

    #[macros::anyhow]
    fn UnadviseSink(&self, dw_cookie: u32) -> Result<()> {
        if dw_cookie != TEXTSERVICE_LANGBARITEMSINK_COOKIE {
            return Err(anyhow::Error::new(windows_core::Error::from_hresult(
                CONNECT_E_CANNOTCONNECT,
            )));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_settings_app_path;
    use std::path::Path;

    #[test]
    fn resolve_settings_app_path_uses_dll_parent_directory() {
        let dll_path = Path::new("C:/Users/test/AppData/Roaming/Azookey/azookey.dll");

        let resolved = resolve_settings_app_path(dll_path).expect("path should resolve");

        assert_eq!(
            resolved,
            Path::new("C:/Users/test/AppData/Roaming/Azookey/Azookey.exe")
        );
    }

    #[test]
    fn resolve_settings_app_path_rejects_dll_path_without_parent() {
        let dll_path = Path::new("azookey.dll");

        let result = resolve_settings_app_path(dll_path);

        assert!(result.is_err());
    }
}
