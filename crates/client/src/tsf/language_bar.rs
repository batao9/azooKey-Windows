use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

use windows::{
    core::{w, IUnknown, Interface as _, BSTR, GUID, PCWSTR},
    Win32::{
        Foundation::{
            BOOL, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS, E_INVALIDARG, HWND,
            LPARAM, POINT, RECT, WPARAM,
        },
        Graphics::Gdi::HBITMAP,
        System::{
            Ole::CONNECT_E_CANNOTCONNECT,
            Registry::{
                RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, REG_ROUTINE_FLAGS,
                REG_VALUE_TYPE, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6464KEY,
            },
        },
        UI::{
            TextServices::{
                ITfLangBarItemButton_Impl, ITfLangBarItemSink, ITfLangBarItem_Impl, ITfMenu,
                ITfSource_Impl, TfLBIClick, GUID_LBI_INPUTMODE, TF_LANGBARITEMINFO,
                TF_LBI_CLK_LEFT, TF_LBI_CLK_RIGHT, TF_LBI_STYLE_BTN_BUTTON, TF_LBI_STYLE_BTN_MENU,
            },
            WindowsAndMessaging::{
                AppendMenuW, CreatePopupMenu, DestroyMenu, GetAncestor, GetForegroundWindow,
                LoadImageW, PostMessageW, SetForegroundWindow, TrackPopupMenu, WindowFromPoint,
                GA_ROOT, HICON, HMENU, IMAGE_ICON, LR_DEFAULTCOLOR, MF_STRING, TPM_NONOTIFY,
                TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_NULL,
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
    dwStyle: TF_LBI_STYLE_BTN_BUTTON | TF_LBI_STYLE_BTN_MENU,
    ulSort: 0,
    szDescription: [0; 32],
};

const SETTINGS_MENU_ID: usize = 1;
const SETTINGS_APP_DIRNAME: &str = "Azookey";
const SETTINGS_APP_FILENAME: &str = "frontend.exe";
const SETTINGS_APP_UNINSTALL_SUBKEY: PCWSTR =
    w!(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Azookey");
const SETTINGS_APP_INSTALL_LOCATION_VALUE: PCWSTR = w!("InstallLocation");
const SETTINGS_APP_MAIN_BINARY_NAME_VALUE: PCWSTR = w!("MainBinaryName");

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
                launch_settings_app_with_logging();
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

    #[macros::anyhow]
    fn InitMenu(&self, pmenu: Option<&ITfMenu>) -> Result<()> {
        if let Some(menu) = pmenu {
            add_settings_menu_item(menu)?;
        }

        Ok(())
    }

    #[macros::anyhow]
    fn OnMenuSelect(&self, w_id: u32) -> Result<()> {
        if w_id == SETTINGS_MENU_ID as u32 {
            launch_settings_app_with_logging();
        }

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

fn launch_settings_app_with_logging() {
    if let Err(error) = launch_settings_app() {
        tracing::warn!(?error, "Failed to launch settings app");
    }
}

fn add_settings_menu_item(menu: &ITfMenu) -> Result<()> {
    let text: Vec<u16> = "設定".encode_utf16().collect();

    unsafe {
        menu.AddMenuItem(
            SETTINGS_MENU_ID as u32,
            0,
            HBITMAP::default(),
            HBITMAP::default(),
            &text,
            std::ptr::null_mut(),
        )?;
    }

    Ok(())
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

        let hwnd = resolve_menu_owner_window(*pt);
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

fn resolve_menu_owner_window(pt: POINT) -> HWND {
    unsafe {
        let hwnd = WindowFromPoint(pt);
        if !hwnd.0.is_null() {
            let root = GetAncestor(hwnd, GA_ROOT);
            if !root.0.is_null() {
                return root;
            }

            return hwnd;
        }

        GetForegroundWindow()
    }
}

fn launch_settings_app() -> Result<()> {
    let settings_path = resolve_settings_app_path()?;
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

fn resolve_settings_app_path() -> Result<PathBuf> {
    if let Some(settings_path) = resolve_settings_app_path_from_registry()? {
        return Ok(settings_path);
    }

    let local_app_data = env::var_os("LOCALAPPDATA").context("LOCALAPPDATA is not set")?;
    let fallback_install_location = Path::new(&local_app_data).join(SETTINGS_APP_DIRNAME);
    let fallback_install_location = fallback_install_location.to_string_lossy();

    resolve_settings_app_path_from_install_location(
        fallback_install_location.as_ref(),
        SETTINGS_APP_FILENAME,
    )
}

fn resolve_settings_app_path_from_registry() -> Result<Option<PathBuf>> {
    for (hkey, flags) in [
        (HKEY_CURRENT_USER, REG_ROUTINE_FLAGS(0)),
        (HKEY_LOCAL_MACHINE, REG_ROUTINE_FLAGS(0)),
        (HKEY_LOCAL_MACHINE, RRF_SUBKEY_WOW6464KEY),
    ] {
        match resolve_settings_app_path_from_uninstall_key(hkey, flags) {
            Ok(Some(settings_path)) => return Ok(Some(settings_path)),
            Ok(None) => {}
            Err(error) => {
                tracing::debug!(?error, "Skip invalid settings app install metadata");
            }
        }
    }

    Ok(None)
}

fn resolve_settings_app_path_from_uninstall_key(
    hkey: HKEY,
    flags: REG_ROUTINE_FLAGS,
) -> Result<Option<PathBuf>> {
    let install_location = match read_registry_string(
        hkey,
        SETTINGS_APP_UNINSTALL_SUBKEY,
        SETTINGS_APP_INSTALL_LOCATION_VALUE,
        flags,
    )? {
        Some(install_location) => install_location,
        None => return Ok(None),
    };

    let main_binary_name = read_registry_string(
        hkey,
        SETTINGS_APP_UNINSTALL_SUBKEY,
        SETTINGS_APP_MAIN_BINARY_NAME_VALUE,
        flags,
    )?
    .unwrap_or_else(|| SETTINGS_APP_FILENAME.to_string());

    let settings_path =
        resolve_settings_app_path_from_install_location(&install_location, &main_binary_name)?;

    Ok(Some(settings_path))
}

fn resolve_settings_app_path_from_install_location(
    install_location: &str,
    main_binary_name: &str,
) -> Result<PathBuf> {
    let install_location = trim_registry_string(install_location);
    let main_binary_name = trim_registry_string(main_binary_name);

    if install_location.is_empty() {
        anyhow::bail!("Settings app install location is empty");
    }

    if main_binary_name.is_empty() {
        anyhow::bail!("Settings app main binary name is empty");
    }

    Ok(Path::new(install_location).join(main_binary_name))
}

fn trim_registry_string(value: &str) -> &str {
    value.trim().trim_matches('"')
}

fn read_registry_string(
    hkey: HKEY,
    subkey: PCWSTR,
    value: PCWSTR,
    flags: REG_ROUTINE_FLAGS,
) -> Result<Option<String>> {
    let flags = flags | RRF_RT_REG_SZ;
    let mut value_type = REG_VALUE_TYPE::default();
    let mut data_size = 0u32;

    let status = unsafe {
        RegGetValueW(
            hkey,
            subkey,
            value,
            flags,
            Some(&mut value_type),
            None,
            Some(&mut data_size),
        )
    };

    if status == ERROR_FILE_NOT_FOUND || status == ERROR_PATH_NOT_FOUND {
        return Ok(None);
    }

    if status != ERROR_SUCCESS {
        anyhow::bail!("Failed to read registry value: {:?}", status);
    }

    if data_size == 0 {
        return Ok(Some(String::new()));
    }

    let mut data = vec![0u16; ((data_size + 1) / 2) as usize];
    let status = unsafe {
        RegGetValueW(
            hkey,
            subkey,
            value,
            flags,
            Some(&mut value_type),
            Some(data.as_mut_ptr().cast()),
            Some(&mut data_size),
        )
    };

    if status != ERROR_SUCCESS {
        anyhow::bail!("Failed to read registry value data: {:?}", status);
    }

    let mut len = (data_size as usize) / 2;
    if data.get(len.saturating_sub(1)) == Some(&0) {
        len = len.saturating_sub(1);
    }

    let value = String::from_utf16(&data[..len]).context("Registry value is not valid UTF-16")?;

    Ok(Some(value))
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
    use super::{resolve_settings_app_path_from_install_location, trim_registry_string};
    use std::path::PathBuf;

    #[test]
    fn resolve_settings_app_path_from_install_location_uses_recorded_install_root() {
        let resolved =
            resolve_settings_app_path_from_install_location("D:/Apps/Azookey", "frontend.exe")
                .expect("path should resolve");

        assert_eq!(resolved, PathBuf::from("D:/Apps/Azookey/frontend.exe"));
    }

    #[test]
    fn resolve_settings_app_path_from_install_location_rejects_empty_path() {
        let result = resolve_settings_app_path_from_install_location("", "frontend.exe");

        assert!(result.is_err());
    }

    #[test]
    fn resolve_settings_app_path_from_install_location_trims_registry_quotes() {
        let resolved = resolve_settings_app_path_from_install_location(
            "\"C:/Users/test/AppData/Local/Azookey\"",
            "\"frontend.exe\"",
        )
        .expect("quoted path should resolve");

        assert_eq!(
            resolved,
            PathBuf::from("C:/Users/test/AppData/Local/Azookey/frontend.exe")
        );
    }

    #[test]
    fn trim_registry_string_removes_wrapping_quotes_only() {
        assert_eq!(trim_registry_string("  \"Azookey\"  "), "Azookey");
    }
}
