use std::{
    ffi::c_void,
    ptr,
    sync::{
        atomic::{AtomicPtr, AtomicUsize, Ordering},
        Arc, Mutex, MutexGuard, OnceLock,
    },
};

use anyhow::{Context as _, Result};

use windows::{
    core::{Error as WindowsError, GUID},
    Win32::{
        Foundation::{FALSE, HMODULE, MAX_PATH},
        System::LibraryLoader::GetModuleFileNameW,
        UI::TextServices::{
            TF_ATTR_TARGET_CONVERTED, TF_CT_NONE, TF_DA_COLOR, TF_DA_COLOR_0, TF_DISPLAYATTRIBUTE,
            TF_LS_SOLID,
        },
    },
};

const INITIAL_MODULE_PATH_CAPACITY: usize = MAX_PATH as usize;
const MAX_MODULE_PATH_CAPACITY: usize = 32_768;

pub const CLSID_PREFIX: &str = "CLSID\\";
pub const INPROC_SUFFIX: &str = "\\InProcServer32";

pub const SERVICE_NAME: &str = "Azookey";

// ffdefe79-2fc2-11ef-b16b-94e70b2c378c
pub const GUID_TEXT_SERVICE: GUID = GUID::from_u128(0xffdefe79_2fc2_11ef_b16b_94e70b2c378c);
// ffdefe7a-2fc2-11ef-b16b-94e70b2c378c
pub const GUID_PROFILE: GUID = GUID::from_u128(0xffdefe7a_2fc2_11ef_b16b_94e70b2c378c);

// DisplayAttribute用のGUID
pub const GUID_DISPLAY_ATTRIBUTE: GUID = GUID::from_u128(0xffdefe7b_2fc2_11ef_b16b_94e70b2c378c);

// Preserved key for CapsLock input mode toggle.
pub const GUID_PRESERVED_KEY_EISU_CAPSLOCK_ANY_MODIFIER: GUID =
    GUID::from_u128(0xffdefe7c_2fc2_11ef_b16b_94e70b2c378c);

pub const DISPLAY_ATTRIBUTE: TF_DISPLAYATTRIBUTE = TF_DISPLAYATTRIBUTE {
    crText: TF_DA_COLOR {
        r#type: TF_CT_NONE,
        Anonymous: TF_DA_COLOR_0 { nIndex: 0 },
    },
    crBk: TF_DA_COLOR {
        r#type: TF_CT_NONE,
        Anonymous: TF_DA_COLOR_0 { nIndex: 0 },
    },
    lsStyle: TF_LS_SOLID,
    fBoldLine: FALSE,
    crLine: TF_DA_COLOR {
        r#type: TF_CT_NONE,
        Anonymous: TF_DA_COLOR_0 { nIndex: 0 },
    },
    bAttr: TF_ATTR_TARGET_CONVERTED,
};

// You can use any value for this cookie.
pub const TEXTSERVICE_LANGBARITEMSINK_COOKIE: u32 = 0;

static DLL_MODULE_HANDLE: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static DLL_INSTANCE: OnceLock<Mutex<DllModule>> = OnceLock::new();

fn read_module_path(mut read: impl FnMut(&mut [u16]) -> Result<usize>) -> anyhow::Result<String> {
    let mut capacity = INITIAL_MODULE_PATH_CAPACITY;
    loop {
        let mut buffer = vec![0; capacity];
        let length = read(&mut buffer)?;
        if length == 0 {
            anyhow::bail!("GetModuleFileNameW returned an empty module path");
        }
        if length > buffer.len() {
            anyhow::bail!(
                "GetModuleFileNameW returned invalid length {length} for buffer capacity {}",
                buffer.len()
            );
        }
        if length < buffer.len() {
            return String::from_utf16(&buffer[..length])
                .context("Module path returned by GetModuleFileNameW is not valid UTF-16");
        }
        if buffer.len() == MAX_MODULE_PATH_CAPACITY {
            anyhow::bail!(
                "Module path remained truncated at the Windows maximum buffer size of \
                 {MAX_MODULE_PATH_CAPACITY} UTF-16 code units"
            );
        }
        capacity = capacity.saturating_mul(2).min(MAX_MODULE_PATH_CAPACITY);
    }
}

unsafe impl Sync for DllModule {}
unsafe impl Send for DllModule {}

#[derive(Debug)]
pub struct DllModule {
    pub ref_count: Arc<AtomicUsize>,
}

impl DllModule {
    pub fn new() -> Self {
        Self {
            ref_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn set_module_handle(hinst: HMODULE) {
        DLL_MODULE_HANDLE.store(hinst.0, Ordering::Release);
    }

    pub fn module_handle() -> Result<HMODULE> {
        let handle = DLL_MODULE_HANDLE.load(Ordering::Acquire);
        if handle.is_null() {
            Err(anyhow::anyhow!("Dll module handle is not initialized"))
        } else {
            Ok(HMODULE(handle))
        }
    }

    pub fn initialize() -> Result<()> {
        Self::module_handle()?;
        DLL_INSTANCE.get_or_init(|| Mutex::new(DllModule::new()));
        Ok(())
    }

    pub fn get() -> Result<MutexGuard<'static, DllModule>> {
        Self::initialize()?;
        DLL_INSTANCE
            .get()
            .ok_or_else(|| anyhow::anyhow!("DllModule is not initialized"))?
            .lock()
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    }

    pub fn get_path() -> anyhow::Result<String> {
        let module_handle = Self::module_handle()?;
        read_module_path(|buffer| {
            let length = unsafe { GetModuleFileNameW(module_handle, buffer) };
            if length == 0 {
                Err(anyhow::Error::new(WindowsError::from_win32()))
                    .context("GetModuleFileNameW failed")
            } else {
                Ok(length as usize)
            }
        })
    }

    pub fn add_ref(&mut self) -> usize {
        self.ref_count.fetch_add(1, Ordering::SeqCst)
    }

    pub fn release(&mut self) -> usize {
        self.ref_count.fetch_sub(1, Ordering::SeqCst)
    }

    #[allow(dead_code)]
    pub fn can_unload(&self) -> bool {
        self.ref_count.load(Ordering::SeqCst) == 0
    }
}

#[cfg(test)]
mod tests {
    use super::{read_module_path, MAX_MODULE_PATH_CAPACITY};

    #[test]
    fn win32_wide_boundary_module_path_grows_past_max_path_without_truncation() {
        let expected = format!(r"C:\{}\azookey.dll", "深い😀パス\\".repeat(80));
        let encoded = expected.encode_utf16().collect::<Vec<_>>();
        assert!(encoded.len() > 260);
        let mut calls = 0;

        let actual = read_module_path(|buffer| {
            calls += 1;
            if encoded.len() >= buffer.len() {
                buffer.copy_from_slice(&encoded[..buffer.len()]);
                Ok(buffer.len())
            } else {
                buffer[..encoded.len()].copy_from_slice(&encoded);
                Ok(encoded.len())
            }
        })
        .expect("long module path should be returned completely");

        assert_eq!(actual, expected);
        assert!(calls > 1);
    }

    #[test]
    fn win32_wide_boundary_module_path_propagates_win32_failure() {
        let error = read_module_path(|_| anyhow::bail!("simulated Win32 failure"))
            .expect_err("failure must not be accepted as a module path");

        assert!(error.to_string().contains("simulated Win32 failure"));
    }

    #[test]
    fn win32_wide_boundary_module_path_rejects_persistent_truncation() {
        let error = read_module_path(|buffer| Ok(buffer.len()))
            .expect_err("persistent truncation must not be accepted as a module path");

        assert!(error.to_string().contains("remained truncated"));
        assert!(error
            .to_string()
            .contains(&MAX_MODULE_PATH_CAPACITY.to_string()));
    }
}
