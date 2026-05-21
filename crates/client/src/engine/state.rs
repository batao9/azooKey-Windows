use std::sync::{LazyLock, Mutex, MutexGuard};

use windows::{
    core::Interface as _,
    Win32::UI::TextServices::{ITfCompartmentMgr, ITfContext, GUID_COMPARTMENT_KEYBOARD_DISABLED},
};

use super::{input_mode::InputMode, ipc_service::IPCService};

#[derive(Debug)]
pub struct IMEState {
    pub ipc_service: Option<IPCService>,
    pub input_mode: InputMode,
    pub keyboard_disabled: bool,
}

pub static IME_STATE: LazyLock<Mutex<IMEState>> = LazyLock::new(|| {
    tracing::debug!("Creating IMEState");
    Mutex::new(IMEState {
        ipc_service: None,
        input_mode: InputMode::default(),
        keyboard_disabled: false,
    })
});

impl IMEState {
    pub fn get() -> anyhow::Result<MutexGuard<'static, IMEState>> {
        Ok(IME_STATE.lock().unwrap_or_else(|poisoned| {
            tracing::error!("IME state mutex was poisoned; recovering state");
            poisoned.into_inner()
        }))
    }

    pub fn ipc_service() -> anyhow::Result<Option<IPCService>> {
        Ok(Self::get()?.ipc_service.clone())
    }

    pub fn set_ipc_service(ipc_service: IPCService) -> anyhow::Result<()> {
        Self::get()?.ipc_service = Some(ipc_service);
        Ok(())
    }

    pub fn input_mode() -> anyhow::Result<InputMode> {
        Ok(Self::get()?.input_mode.clone())
    }

    pub fn set_input_mode(input_mode: InputMode) -> anyhow::Result<()> {
        Self::get()?.input_mode = input_mode;
        Ok(())
    }

    pub fn keyboard_disabled() -> anyhow::Result<bool> {
        Ok(Self::get()?.keyboard_disabled)
    }

    pub fn set_keyboard_disabled_and_clone_ipc(
        disabled: bool,
    ) -> anyhow::Result<(bool, Option<IPCService>)> {
        let mut state = Self::get()?;
        let changed = state.keyboard_disabled != disabled;
        state.keyboard_disabled = disabled;
        let ipc_service = if disabled {
            state.ipc_service.clone()
        } else {
            None
        };

        Ok((changed, ipc_service))
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
