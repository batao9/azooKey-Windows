//! The TypeSafe API key is stored in the current user's Windows credential set.
use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Security::Credentials::{
    CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE,
    CRED_TYPE_GENERIC,
};

const TARGET: PCWSTR = w!("azooKey/TypeSafe/Jev");

pub fn read_jev_api_key() -> Option<String> {
    read_credential(TARGET)
}

fn read_credential(target: PCWSTR) -> Option<String> {
    let mut pointer = std::ptr::null_mut();
    unsafe {
        CredReadW(target, CRED_TYPE_GENERIC, 0, &mut pointer).ok()?;
        let credential: &CREDENTIALW = &*pointer;
        let key = if credential.CredentialBlobSize == 0 || credential.CredentialBlob.is_null() {
            None
        } else {
            let bytes = std::slice::from_raw_parts(
                credential.CredentialBlob,
                credential.CredentialBlobSize as usize,
            );
            String::from_utf8(bytes.to_vec()).ok()
        };
        CredFree(pointer.cast());
        key.filter(|key| !key.is_empty())
    }
}

pub fn save_jev_api_key(key: &str) -> Result<(), String> {
    save_credential(TARGET, key)
}

fn save_credential(target: PCWSTR, key: &str) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() || key.len() > 2048 || key.chars().any(char::is_control) {
        return Err("APIキーの形式が正しくありません".into());
    }
    let credential = CREDENTIALW {
        Type: CRED_TYPE_GENERIC,
        TargetName: PWSTR(target.0.cast_mut()),
        CredentialBlobSize: key.len() as u32,
        CredentialBlob: key.as_ptr().cast_mut(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        ..Default::default()
    };
    unsafe { CredWriteW(&credential, 0) }.map_err(|error| {
        format!(
            "APIキーを保存できませんでした (Windows: {:X})",
            error.code().0
        )
    })
}

pub fn delete_jev_api_key() -> Result<(), String> {
    delete_credential(TARGET)
}

fn delete_credential(target: PCWSTR) -> Result<(), String> {
    match unsafe { CredDeleteW(target, CRED_TYPE_GENERIC, 0) } {
        Ok(()) => Ok(()),
        Err(error) if error.code().0 == 0x80070490_u32 as i32 => Ok(()), // ERROR_NOT_FOUND
        Err(_) => Err("APIキーを削除できませんでした".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires an interactive Windows logon with a credential set"]
    fn credential_roundtrip_and_delete() {
        let name = format!("azooKey/TypeSafe/Jev/test-{}", std::process::id());
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let target = PCWSTR(wide.as_ptr());
        save_credential(target, "test-credential").unwrap();
        assert_eq!(read_credential(target).as_deref(), Some("test-credential"));
        assert!(delete_credential(target).is_ok());
        assert_eq!(read_credential(target), None);
    }

    #[test]
    fn rejects_invalid_keys_before_accessing_windows_credentials() {
        assert!(save_jev_api_key(" ").is_err());
        assert!(save_jev_api_key("key\nline").is_err());
        assert!(save_jev_api_key(&"a".repeat(2049)).is_err());
    }
}
