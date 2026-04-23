use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex, MutexGuard},
};

use windows::{
    core::{Interface as _, GUID},
    Win32::UI::TextServices::{ITfCompartmentMgr, ITfContext, GUID_COMPARTMENT_KEYBOARD_DISABLED},
};

use super::{input_mode::InputMode, ipc_service::IPCService};

#[derive(Debug)]
pub struct IMEState {
    pub ipc_service: Option<IPCService>,
    pub input_mode: InputMode,
    pub keyboard_disabled: bool,
    pub cookies: HashMap<GUID, u32>,
    pub context: Option<ITfContext>,
}

pub static IME_STATE: LazyLock<Mutex<IMEState>> = LazyLock::new(|| {
    tracing::debug!("Creating IMEState");
    Mutex::new(IMEState {
        ipc_service: None,
        input_mode: InputMode::default(),
        keyboard_disabled: false,
        cookies: HashMap::new(),
        context: None,
    })
});
unsafe impl Sync for IMEState {}
unsafe impl Send for IMEState {}

impl IMEState {
    pub fn get() -> anyhow::Result<MutexGuard<'static, IMEState>> {
        match IME_STATE.try_lock() {
            Ok(guard) => Ok(guard),
            Err(e) => anyhow::bail!("Failed to lock state: {:?}", e),
        }
    }
}

pub fn keyboard_disabled_from_context(context: &ITfContext) -> bool {
    unsafe {
        let Ok(compartment_mgr) = context.cast::<ITfCompartmentMgr>() else {
            return false;
        };
        let Ok(compartment) = compartment_mgr.GetCompartment(&GUID_COMPARTMENT_KEYBOARD_DISABLED)
        else {
            return false;
        };
        let Ok(value) = compartment.GetValue() else {
            return false;
        };

        i32::try_from(&value)
            .map(|value| value != 0)
            .unwrap_or(false)
    }
}
