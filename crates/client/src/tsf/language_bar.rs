use std::{
    env,
    os::windows::ffi::OsStrExt as _,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, Ordering},
};

use windows::{
    core::{w, IUnknown, Interface as _, BSTR, GUID, PCWSTR},
    Win32::{
        Foundation::{
            GetLastError, BOOL, ERROR_CLASS_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND,
            ERROR_PATH_NOT_FOUND, ERROR_SUCCESS, E_INVALIDARG, HINSTANCE, HWND, LPARAM, LRESULT,
            POINT, RECT, WPARAM,
        },
        System::{
            Ole::CONNECT_E_CANNOTCONNECT,
            Registry::{
                RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, REG_ROUTINE_FLAGS,
                REG_VALUE_TYPE, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6432KEY, RRF_SUBKEY_WOW6464KEY,
            },
        },
        UI::{
            Shell::ShellExecuteW,
            TextServices::{
                ITfLangBarItemButton_Impl, ITfLangBarItemSink, ITfLangBarItem_Impl, ITfMenu,
                ITfSource_Impl, TfLBIClick, GUID_LBI_INPUTMODE, TF_LANGBARITEMINFO,
                TF_LBI_CLK_LEFT, TF_LBI_CLK_RIGHT, TF_LBI_STATUS_DISABLED, TF_LBI_STYLE_BTN_BUTTON,
            },
            WindowsAndMessaging::{
                AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu,
                DestroyWindow, LoadImageW, PostMessageW, RegisterClassW, SetForegroundWindow,
                TrackPopupMenu, UnregisterClassW, HICON, HMENU, IMAGE_ICON, LR_DEFAULTCOLOR,
                MF_STRING, SW_SHOWNORMAL, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_NULL,
                WNDCLASSW, WS_EX_TOOLWINDOW, WS_POPUP,
            },
        },
    },
};

use crate::{
    engine::{
        client_action::ClientAction, composition::CompositionState, input_mode::InputMode,
        state::IMEState, theme::get_theme,
    },
    extension::StringExt as _,
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
const SETTINGS_APP_DIRNAME: &str = "Azookey";
const SETTINGS_APP_FILENAME: &str = "frontend.exe";
const SETTINGS_APP_INNO_UNINSTALL_SUBKEY: PCWSTR = w!(
    r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{80B746D4-D74D-4345-8F81-47E06BCAB515}_is1"
);
const SETTINGS_APP_LEGACY_NSIS_UNINSTALL_SUBKEY: PCWSTR =
    w!(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Azookey");
const SETTINGS_APP_INSTALL_LOCATION_VALUE: PCWSTR = w!("InstallLocation");
const SETTINGS_APP_MAIN_BINARY_NAME_VALUE: PCWSTR = w!("MainBinaryName");

static MENU_OWNER_WINDOW_CLASS_SEQUENCE: AtomicU32 = AtomicU32::new(0);

// you need to implement these three interfaces to create a language bar item
// if not, you will get E_FAIL error in ITfLangBarItemMgr::AddItem

impl TextServiceFactory_Impl {
    fn toggle_input_mode(&self) -> Result<()> {
        let mode = match IMEState::input_mode()? {
            InputMode::Latin => InputMode::Kana,
            InputMode::Kana => InputMode::Latin,
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
        if IMEState::keyboard_disabled()? {
            Ok(TF_LBI_STATUS_DISABLED)
        } else {
            Ok(0)
        }
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
        if IMEState::keyboard_disabled()? {
            return Ok(());
        }

        match click {
            TF_LBI_CLK_LEFT => self.toggle_input_mode()?,
            TF_LBI_CLK_RIGHT => self.handle_right_click(pt)?,
            _ => {}
        }

        Ok(())
    }

    #[macros::anyhow]
    fn InitMenu(&self, _pmenu: Option<&ITfMenu>) -> Result<()> {
        Ok(())
    }

    #[macros::anyhow]
    fn OnMenuSelect(&self, _w_id: u32) -> Result<()> {
        Ok(())
    }

    #[macros::anyhow]
    fn GetIcon(&self) -> Result<HICON> {
        let dll_module = DllModule::get()?;
        let keyboard_disabled = IMEState::keyboard_disabled()?;
        let input_mode = if keyboard_disabled {
            InputMode::Latin
        } else {
            IMEState::input_mode()?
        };
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

struct MenuOwnerWindow {
    hwnd: HWND,
    class_name: Vec<u16>,
    hinstance: HINSTANCE,
}

impl Drop for MenuOwnerWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
            let _ = UnregisterClassW(PCWSTR(self.class_name.as_ptr()), self.hinstance);
        }
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
        let owner = create_menu_owner_window()?;
        let menu = PopupMenu(CreatePopupMenu()?);
        AppendMenuW(menu.0, MF_STRING, SETTINGS_MENU_ID, w!("設定"))?;

        let _ = SetForegroundWindow(owner.hwnd);

        let selected = TrackPopupMenu(
            menu.0,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
            pt.x,
            pt.y,
            0,
            owner.hwnd,
            None,
        )
        .0 as u32;

        let _ = PostMessageW(owner.hwnd, WM_NULL, WPARAM(0), LPARAM(0));

        if selected == 0 {
            Ok(None)
        } else {
            Ok(Some(selected))
        }
    }
}

unsafe extern "system" fn menu_owner_window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn create_menu_owner_window() -> Result<MenuOwnerWindow> {
    let dll_module = DllModule::get()?;
    let hmodule = dll_module.hinst.context("Dll instance not found")?;
    let hinstance = HINSTANCE(hmodule.0);

    unsafe {
        for _ in 0..32 {
            let class_name = menu_owner_window_class_name(hmodule);
            let window_class = WNDCLASSW {
                lpfnWndProc: Some(menu_owner_window_proc),
                hInstance: hinstance,
                lpszClassName: PCWSTR(class_name.as_ptr()),
                ..Default::default()
            };

            let atom = RegisterClassW(&window_class);
            if atom == 0 {
                let error = GetLastError();
                if error == ERROR_CLASS_ALREADY_EXISTS {
                    continue;
                }

                anyhow::bail!("Failed to register menu owner window class: {:?}", error);
            }

            let hwnd = match CreateWindowExW(
                WS_EX_TOOLWINDOW,
                PCWSTR(class_name.as_ptr()),
                w!(""),
                WS_POPUP,
                0,
                0,
                0,
                0,
                HWND::default(),
                HMENU::default(),
                hinstance,
                None,
            ) {
                Ok(hwnd) => hwnd,
                Err(error) => {
                    let _ = UnregisterClassW(PCWSTR(class_name.as_ptr()), hinstance);
                    return Err(error).context("Failed to create menu owner window");
                }
            };

            return Ok(MenuOwnerWindow {
                hwnd,
                class_name,
                hinstance,
            });
        }

        anyhow::bail!("Failed to register unique menu owner window class")
    }
}

fn menu_owner_window_class_name(hmodule: windows::Win32::Foundation::HMODULE) -> Vec<u16> {
    let sequence = MENU_OWNER_WINDOW_CLASS_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!(
        "AzookeyLangBarMenuOwner-{}-{:#x}-{}",
        std::process::id(),
        hmodule.0 as usize,
        sequence
    )
    .as_str()
    .to_wide_16()
}

fn launch_settings_app() -> Result<()> {
    let settings_app = resolve_settings_app_path()?;
    let settings_path = settings_app.path;
    let install_dir = settings_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .context("Settings app directory not found")?;

    if !settings_path.is_file() {
        anyhow::bail!("Settings app not found: {}", settings_path.display());
    }

    shell_execute_open(&settings_path, install_dir).with_context(|| {
        format!(
            "Failed to launch settings app from {}: {}",
            settings_app.source,
            settings_path.display()
        )
    })?;

    Ok(())
}

fn shell_execute_open(settings_path: &Path, install_dir: &Path) -> Result<()> {
    let settings_path = path_to_wide(settings_path);
    let install_dir = path_to_wide(install_dir);

    let result = unsafe {
        ShellExecuteW(
            HWND::default(),
            w!("open"),
            PCWSTR(settings_path.as_ptr()),
            PCWSTR::null(),
            PCWSTR(install_dir.as_ptr()),
            SW_SHOWNORMAL,
        )
    };

    if result.0 as isize <= 32 {
        anyhow::bail!("ShellExecuteW failed: code={}", result.0 as isize);
    }

    Ok(())
}

fn path_to_wide(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SettingsAppPath {
    path: PathBuf,
    source: &'static str,
}

fn resolve_settings_app_path() -> Result<SettingsAppPath> {
    let mut candidates = resolve_settings_app_path_candidates()?;

    if let Some(local_app_data) = env::var_os("LOCALAPPDATA") {
        candidates.push(SettingsAppPath {
            path: Path::new(&local_app_data)
                .join(SETTINGS_APP_DIRNAME)
                .join(SETTINGS_APP_FILENAME),
            source: "LOCALAPPDATA fallback",
        });
    } else {
        tracing::debug!("LOCALAPPDATA is not set; skip settings app fallback path");
    }

    select_existing_settings_app_path(candidates, Path::is_file)
}

fn resolve_settings_app_path_candidates() -> Result<Vec<SettingsAppPath>> {
    let mut candidates = Vec::new();

    for (hkey, flags, subkey, source) in [
        (
            HKEY_CURRENT_USER,
            REG_ROUTINE_FLAGS(0),
            SETTINGS_APP_INNO_UNINSTALL_SUBKEY,
            "HKCU Inno uninstall key",
        ),
        (
            HKEY_CURRENT_USER,
            RRF_SUBKEY_WOW6464KEY,
            SETTINGS_APP_INNO_UNINSTALL_SUBKEY,
            "HKCU 64-bit Inno uninstall key",
        ),
        (
            HKEY_CURRENT_USER,
            RRF_SUBKEY_WOW6432KEY,
            SETTINGS_APP_INNO_UNINSTALL_SUBKEY,
            "HKCU 32-bit Inno uninstall key",
        ),
        (
            HKEY_LOCAL_MACHINE,
            REG_ROUTINE_FLAGS(0),
            SETTINGS_APP_INNO_UNINSTALL_SUBKEY,
            "HKLM Inno uninstall key",
        ),
        (
            HKEY_LOCAL_MACHINE,
            RRF_SUBKEY_WOW6464KEY,
            SETTINGS_APP_INNO_UNINSTALL_SUBKEY,
            "HKLM 64-bit Inno uninstall key",
        ),
        (
            HKEY_LOCAL_MACHINE,
            RRF_SUBKEY_WOW6432KEY,
            SETTINGS_APP_INNO_UNINSTALL_SUBKEY,
            "HKLM 32-bit Inno uninstall key",
        ),
        (
            HKEY_CURRENT_USER,
            REG_ROUTINE_FLAGS(0),
            SETTINGS_APP_LEGACY_NSIS_UNINSTALL_SUBKEY,
            "HKCU legacy NSIS uninstall key",
        ),
        (
            HKEY_CURRENT_USER,
            RRF_SUBKEY_WOW6464KEY,
            SETTINGS_APP_LEGACY_NSIS_UNINSTALL_SUBKEY,
            "HKCU 64-bit legacy NSIS uninstall key",
        ),
        (
            HKEY_CURRENT_USER,
            RRF_SUBKEY_WOW6432KEY,
            SETTINGS_APP_LEGACY_NSIS_UNINSTALL_SUBKEY,
            "HKCU 32-bit legacy NSIS uninstall key",
        ),
        (
            HKEY_LOCAL_MACHINE,
            REG_ROUTINE_FLAGS(0),
            SETTINGS_APP_LEGACY_NSIS_UNINSTALL_SUBKEY,
            "HKLM legacy NSIS uninstall key",
        ),
        (
            HKEY_LOCAL_MACHINE,
            RRF_SUBKEY_WOW6464KEY,
            SETTINGS_APP_LEGACY_NSIS_UNINSTALL_SUBKEY,
            "HKLM 64-bit legacy NSIS uninstall key",
        ),
        (
            HKEY_LOCAL_MACHINE,
            RRF_SUBKEY_WOW6432KEY,
            SETTINGS_APP_LEGACY_NSIS_UNINSTALL_SUBKEY,
            "HKLM 32-bit legacy NSIS uninstall key",
        ),
    ] {
        match resolve_settings_app_path_from_uninstall_key(hkey, flags, subkey, source) {
            Ok(Some(settings_path)) => candidates.push(settings_path),
            Ok(None) => {}
            Err(error) => {
                tracing::debug!(?error, source, "Skip invalid settings app install metadata");
            }
        }
    }

    Ok(candidates)
}

fn resolve_settings_app_path_from_uninstall_key(
    hkey: HKEY,
    flags: REG_ROUTINE_FLAGS,
    subkey: PCWSTR,
    source: &'static str,
) -> Result<Option<SettingsAppPath>> {
    let install_location =
        match read_registry_string(hkey, subkey, SETTINGS_APP_INSTALL_LOCATION_VALUE, flags)? {
            Some(install_location) => install_location,
            None => return Ok(None),
        };

    let main_binary_name =
        read_registry_string(hkey, subkey, SETTINGS_APP_MAIN_BINARY_NAME_VALUE, flags)?
            .unwrap_or_else(|| SETTINGS_APP_FILENAME.to_string());

    let settings_path =
        resolve_settings_app_path_from_install_location(&install_location, &main_binary_name)?;

    Ok(Some(SettingsAppPath {
        path: settings_path,
        source,
    }))
}

fn select_existing_settings_app_path(
    candidates: Vec<SettingsAppPath>,
    exists: impl Fn(&Path) -> bool,
) -> Result<SettingsAppPath> {
    let mut missing_candidates = Vec::new();
    for candidate in candidates {
        if exists(&candidate.path) {
            return Ok(candidate);
        }

        missing_candidates.push(candidate);
    }

    if missing_candidates.is_empty() {
        anyhow::bail!(
            "Settings app not found because no install metadata or LOCALAPPDATA fallback candidate is available"
        );
    }

    let candidate_list = missing_candidates
        .iter()
        .map(|candidate| format!("{}={}", candidate.source, candidate.path.display()))
        .collect::<Vec<_>>()
        .join(", ");

    anyhow::bail!("Settings app not found in candidates: {candidate_list}")
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
    use super::{
        resolve_settings_app_path_from_install_location, select_existing_settings_app_path,
        trim_registry_string, SettingsAppPath,
    };
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

    #[test]
    fn select_existing_settings_app_path_skips_missing_registry_candidate() {
        let candidates = vec![
            SettingsAppPath {
                path: PathBuf::from("C:/Old/Azookey/frontend.exe"),
                source: "HKCU Inno uninstall key",
            },
            SettingsAppPath {
                path: PathBuf::from("C:/Users/test/AppData/Local/Azookey/frontend.exe"),
                source: "LOCALAPPDATA fallback",
            },
        ];

        let selected = select_existing_settings_app_path(candidates, |path| {
            path == PathBuf::from("C:/Users/test/AppData/Local/Azookey/frontend.exe")
        })
        .expect("existing fallback should be selected");

        assert_eq!(selected.source, "LOCALAPPDATA fallback");
        assert_eq!(
            selected.path,
            PathBuf::from("C:/Users/test/AppData/Local/Azookey/frontend.exe")
        );
    }

    #[test]
    fn select_existing_settings_app_path_reports_all_missing_candidates() {
        let candidates = vec![SettingsAppPath {
            path: PathBuf::from("C:/Old/Azookey/frontend.exe"),
            source: "HKCU Inno uninstall key",
        }];

        let error = select_existing_settings_app_path(candidates, |_| false)
            .expect_err("missing candidates should fail");

        assert!(error
            .to_string()
            .contains("HKCU Inno uninstall key=C:/Old/Azookey/frontend.exe"));
    }

    #[test]
    fn select_existing_settings_app_path_reports_empty_candidates() {
        let error = select_existing_settings_app_path(Vec::new(), |_| false)
            .expect_err("empty candidates should fail");

        assert!(error
            .to_string()
            .contains("no install metadata or LOCALAPPDATA fallback candidate is available"));
    }
}
