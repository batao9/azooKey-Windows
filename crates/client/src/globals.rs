use std::{
    ffi::c_void,
    ptr,
    sync::{
        atomic::{AtomicPtr, AtomicUsize, Ordering},
        Arc, Mutex, MutexGuard, OnceLock,
    },
};

use anyhow::Result;

use windows::{
    core::GUID,
    Win32::{
        Foundation::{FALSE, HMODULE, MAX_PATH},
        System::LibraryLoader::GetModuleFileNameW,
        UI::TextServices::{
            TF_ATTR_TARGET_CONVERTED, TF_CT_NONE, TF_DA_COLOR, TF_DA_COLOR_0, TF_DISPLAYATTRIBUTE,
            TF_LS_SOLID,
        },
    },
};

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
        let path = {
            let mut buffer: [u16; MAX_PATH as usize] = [0; MAX_PATH as usize];
            let length = unsafe { GetModuleFileNameW(Self::module_handle()?, &mut buffer) };

            String::from_utf16_lossy(&buffer[..length as usize])
        };
        Ok(path)
    }

    pub fn add_ref(&mut self) -> usize {
        self.ref_count.fetch_add(1, Ordering::SeqCst)
    }

    pub fn release(&mut self) -> usize {
        self.ref_count.fetch_sub(1, Ordering::SeqCst)
    }

    pub fn can_unload(&self) -> bool {
        self.ref_count.load(Ordering::SeqCst) <= 0
    }
}
