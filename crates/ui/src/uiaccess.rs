// obtain useraccess privileges from system applications
// cf: https://github.com/killtimer0/uiaccess/

use std::{error::Error, ffi::c_void, fmt, ptr::addr_of_mut, slice};

use anyhow::{Context, Result};
use windows::{
    core::{Owned, HRESULT, PWSTR},
    Win32::{
        Foundation::{BOOL, ERROR_INSUFFICIENT_BUFFER, ERROR_NO_MORE_FILES, HANDLE, LUID},
        Security::{
            DuplicateTokenEx, GetTokenInformation, LookupPrivilegeValueW, RevertToSelf,
            SecurityAnonymous, SecurityImpersonation, SetTokenInformation, TokenImpersonation,
            TokenPrimary, TokenPrivileges, TokenSessionId, TokenUIAccess, LUID_AND_ATTRIBUTES,
            SE_PRIVILEGE_ENABLED, SE_TCB_NAME, TOKEN_ACCESS_MASK, TOKEN_ADJUST_DEFAULT,
            TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_IMPERSONATE, TOKEN_QUERY,
        },
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
                TH32CS_SNAPPROCESS,
            },
            Environment::GetCommandLineW,
            Threading::{
                CreateProcessAsUserW, ExitProcess, GetCurrentProcess, GetStartupInfoW, OpenProcess,
                OpenProcessToken, SetThreadToken, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION,
                PROCESS_QUERY_LIMITED_INFORMATION, STARTUPINFOW,
            },
        },
    },
};

#[derive(Debug)]
struct WinlogonTokenNotFound {
    session_id: u32,
}

impl fmt::Display for WinlogonTokenNotFound {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "no winlogon token with SeTcbPrivilege found for session {}",
            self.session_id
        )
    }
}

impl Error for WinlogonTokenNotFound {}

struct ThreadImpersonation {
    active: bool,
}

impl ThreadImpersonation {
    fn start(token: HANDLE) -> Result<Self> {
        unsafe {
            SetThreadToken(None, token).context("SetThreadToken failed")?;
        }
        Ok(Self { active: true })
    }

    fn revert(mut self) -> Result<()> {
        unsafe {
            RevertToSelf().context("RevertToSelf failed")?;
        }
        self.active = false;
        Ok(())
    }
}

impl Drop for ThreadImpersonation {
    fn drop(&mut self) {
        if self.active {
            if let Err(error) = unsafe { RevertToSelf() } {
                eprintln!("Warning: Failed to revert UIAccess token impersonation: {error:?}");
            }
        }
    }
}

fn open_current_process_token(desired_access: TOKEN_ACCESS_MASK) -> Result<Owned<HANDLE>> {
    let mut token = Owned::<HANDLE>::default();
    unsafe {
        OpenProcessToken(GetCurrentProcess(), desired_access, &mut *token)
            .context("OpenProcessToken for current process failed")?;
    }
    Ok(token)
}

fn token_session_id(token: HANDLE) -> Result<u32> {
    let mut session_id = 0;
    let mut token_info_length = 0;
    unsafe {
        GetTokenInformation(
            token,
            TokenSessionId,
            Some(addr_of_mut!(session_id) as *mut c_void),
            std::mem::size_of::<u32>() as u32,
            &mut token_info_length,
        )
        .context("GetTokenInformation(TokenSessionId) failed")?;
    }
    Ok(session_id)
}

/// check for ui access
pub fn check_for_ui_access() -> Result<bool> {
    let token = open_current_process_token(TOKEN_QUERY)?;
    let mut token_ui_access: BOOL = false.into();
    let mut token_len = 0;

    unsafe {
        GetTokenInformation(
            *token,
            TokenUIAccess,
            Some(&mut token_ui_access as *mut _ as *mut _),
            std::mem::size_of::<BOOL>() as u32,
            &mut token_len,
        )
        .context("GetTokenInformation(TokenUIAccess) failed")?;
    }

    Ok(token_ui_access.as_bool())
}

fn is_winlogon_executable(executable_name: &[u16]) -> bool {
    let name_length = executable_name
        .iter()
        .position(|character| *character == 0)
        .unwrap_or(executable_name.len());
    String::from_utf16_lossy(&executable_name[..name_length]).eq_ignore_ascii_case("winlogon.exe")
}

fn winlogon_process_ids() -> Result<Vec<u32>> {
    let snapshot = unsafe {
        Owned::new(
            CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
                .context("CreateToolhelp32Snapshot failed")?,
        )
    };
    let mut process_entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut process_ids = Vec::new();

    unsafe {
        Process32FirstW(*snapshot, &mut process_entry).context("Process32FirstW failed")?;
        loop {
            if is_winlogon_executable(&process_entry.szExeFile) {
                process_ids.push(process_entry.th32ProcessID);
            }
            match Process32NextW(*snapshot, &mut process_entry) {
                Ok(()) => {}
                Err(error) if error.code() == HRESULT::from_win32(ERROR_NO_MORE_FILES.0) => {
                    break;
                }
                Err(error) => return Err(error).context("Process32NextW failed"),
            }
        }
    }

    Ok(process_ids)
}

fn select_token_for_session<I, T, F>(
    process_ids: I,
    requested_session_id: u32,
    mut inspect_token: F,
) -> Option<T>
where
    I: IntoIterator<Item = u32>,
    F: FnMut(u32) -> Option<(u32, T)>,
{
    process_ids.into_iter().find_map(|process_id| {
        let (token_session_id, token) = inspect_token(process_id)?;
        (token_session_id == requested_session_id).then_some(token)
    })
}

fn token_has_enabled_privilege(token: HANDLE, privilege_luid: LUID) -> Result<bool> {
    let mut token_info_length = 0;
    let size_result =
        unsafe { GetTokenInformation(token, TokenPrivileges, None, 0, &mut token_info_length) };
    match size_result {
        Err(error) if error.code() == HRESULT::from_win32(ERROR_INSUFFICIENT_BUFFER.0) => {}
        Err(error) => {
            return Err(error).context("GetTokenInformation(TokenPrivileges) size failed")
        }
        Ok(()) => anyhow::bail!("GetTokenInformation(TokenPrivileges) returned no size"),
    }

    let word_size = std::mem::size_of::<usize>();
    let word_count = (token_info_length as usize).div_ceil(word_size);
    let mut token_info = vec![0_usize; word_count];
    unsafe {
        GetTokenInformation(
            token,
            TokenPrivileges,
            Some(token_info.as_mut_ptr().cast()),
            token_info_length,
            &mut token_info_length,
        )
        .context("GetTokenInformation(TokenPrivileges) failed")?;
    }

    anyhow::ensure!(
        token_info_length as usize >= std::mem::size_of::<u32>(),
        "TokenPrivileges response is shorter than its header"
    );
    let privilege_count = unsafe { *token_info.as_ptr().cast::<u32>() as usize };
    let privileges_offset = std::mem::size_of::<u32>();
    let privileges_size = privilege_count
        .checked_mul(std::mem::size_of::<LUID_AND_ATTRIBUTES>())
        .and_then(|size| privileges_offset.checked_add(size))
        .context("TokenPrivileges response size overflow")?;
    anyhow::ensure!(
        privileges_size <= token_info_length as usize,
        "TokenPrivileges response is shorter than its privilege array"
    );
    let privileges = unsafe {
        slice::from_raw_parts(
            token_info
                .as_ptr()
                .cast::<u8>()
                .add(privileges_offset)
                .cast::<LUID_AND_ATTRIBUTES>(),
            privilege_count,
        )
    };

    Ok(privileges.iter().any(|privilege| {
        privilege.Luid == privilege_luid && privilege.Attributes.contains(SE_PRIVILEGE_ENABLED)
    }))
}

fn open_winlogon_token(
    process_id: u32,
    tcb_privilege_luid: LUID,
) -> Result<Option<(u32, Owned<HANDLE>)>> {
    let process = unsafe {
        Owned::new(
            OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id)
                .with_context(|| format!("OpenProcess({process_id}) failed"))?,
        )
    };
    let mut token = Owned::<HANDLE>::default();
    unsafe {
        OpenProcessToken(*process, TOKEN_QUERY | TOKEN_DUPLICATE, &mut *token)
            .with_context(|| format!("OpenProcessToken({process_id}) failed"))?;
    }
    if !token_has_enabled_privilege(*token, tcb_privilege_luid)
        .with_context(|| format!("SeTcbPrivilege query for process {process_id} failed"))?
    {
        return Ok(None);
    }

    Ok(Some((token_session_id(*token)?, token)))
}

fn duplicate_winlogon_token(
    session_id: u32,
    desired_access: TOKEN_ACCESS_MASK,
) -> Result<Owned<HANDLE>> {
    let mut tcb_privilege_luid = LUID::default();
    unsafe {
        LookupPrivilegeValueW(None, SE_TCB_NAME, &mut tcb_privilege_luid)
            .context("LookupPrivilegeValueW(SeTcbPrivilege) failed")?;
    }

    let token = select_token_for_session(winlogon_process_ids()?, session_id, |process_id| {
        match open_winlogon_token(process_id, tcb_privilege_luid) {
            Ok(token) => token,
            Err(error) => {
                eprintln!(
                    "Warning: Skipping winlogon process {process_id} while acquiring UIAccess: {error:#}"
                );
                None
            }
        }
    })
    .ok_or(WinlogonTokenNotFound { session_id })?;

    let mut duplicated_token = Owned::<HANDLE>::default();
    unsafe {
        DuplicateTokenEx(
            *token,
            desired_access,
            None,
            SecurityImpersonation,
            TokenImpersonation,
            &mut *duplicated_token,
        )
        .context("DuplicateTokenEx for winlogon token failed")?;
    }
    Ok(duplicated_token)
}

fn create_uiaccess_token() -> Result<Owned<HANDLE>> {
    let token_self = open_current_process_token(TOKEN_QUERY | TOKEN_DUPLICATE)?;
    let session_id = token_session_id(*token_self)?;
    let system_token = duplicate_winlogon_token(session_id, TOKEN_IMPERSONATE)?;
    let impersonation = ThreadImpersonation::start(*system_token)?;

    let mut uiaccess_token = Owned::<HANDLE>::default();
    unsafe {
        DuplicateTokenEx(
            *token_self,
            TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_ASSIGN_PRIMARY | TOKEN_ADJUST_DEFAULT,
            None,
            SecurityAnonymous,
            TokenPrimary,
            &mut *uiaccess_token,
        )
        .context("DuplicateTokenEx for UIAccess token failed")?;
    }
    let ui_access: BOOL = true.into();
    unsafe {
        SetTokenInformation(
            *uiaccess_token,
            TokenUIAccess,
            &ui_access as *const _ as *mut _,
            std::mem::size_of::<BOOL>() as u32,
        )
        .context("SetTokenInformation(TokenUIAccess) failed")?;
    }
    impersonation.revert()?;

    Ok(uiaccess_token)
}

pub fn prepare_uiaccess_token() -> Result<()> {
    if check_for_ui_access()? {
        println!("UIAccess is already enabled");
        return Ok(());
    }

    let token = match create_uiaccess_token() {
        Ok(token) => token,
        Err(error) if error.downcast_ref::<WinlogonTokenNotFound>().is_some() => {
            eprintln!("Warning: {error:#}; continuing candidate UI startup without UIAccess");
            return Ok(());
        }
        Err(error) => return Err(error),
    };

    let mut startup_info = STARTUPINFOW::default();
    let mut process_info = PROCESS_INFORMATION::default();

    unsafe {
        GetStartupInfoW(&mut startup_info);
        CreateProcessAsUserW(
            *token,
            None,
            PWSTR(GetCommandLineW().as_ptr() as *mut u16),
            None,
            None,
            false,
            PROCESS_CREATION_FLAGS::default(),
            None,
            None,
            &startup_info,
            &mut process_info,
        )
        .context("CreateProcessAsUserW for UIAccess process failed")?;

        let process_handle = Owned::new(process_info.hProcess);
        let thread_handle = Owned::new(process_info.hThread);
        println!("Process created with UIAccess token");
        drop(process_handle);
        drop(thread_handle);
        drop(token);
        ExitProcess(0);
    }
}

#[cfg(test)]
mod tests {
    use super::{is_winlogon_executable, select_token_for_session};

    #[test]
    fn winlogon_name_match_is_exact_and_case_insensitive() {
        let mut name = "WinLogon.ExE".encode_utf16().collect::<Vec<_>>();
        name.extend([0, 0]);
        assert!(is_winlogon_executable(&name));

        let similar_name = "not-winlogon.exe".encode_utf16().collect::<Vec<_>>();
        assert!(!is_winlogon_executable(&similar_name));
    }

    #[test]
    fn token_selection_skips_an_earlier_winlogon_from_another_session() {
        let candidates = [(101, 1), (202, 7), (303, 7)];
        let mut inspected_processes = Vec::new();

        let selected = select_token_for_session(
            candidates.map(|(process_id, _)| process_id),
            7,
            |process_id| {
                inspected_processes.push(process_id);
                candidates
                    .iter()
                    .find(|(candidate_id, _)| *candidate_id == process_id)
                    .map(|(_, session_id)| (*session_id, process_id))
            },
        );

        assert_eq!(selected, Some(202));
        assert_eq!(inspected_processes, vec![101, 202]);
    }

    #[test]
    fn token_selection_returns_none_when_no_current_session_token_is_available() {
        let selected = select_token_for_session([101, 202], 7, |process_id| match process_id {
            101 => None,
            202 => Some((1, process_id)),
            _ => unreachable!(),
        });

        assert_eq!(selected, None);
    }
}
