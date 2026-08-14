use windows::{
    core::{Error, IUnknown, Interface, BSTR, GUID},
    Win32::{
        Foundation::{BOOL, E_FAIL, E_INVALIDARG, E_NOINTERFACE, E_NOTIMPL},
        UI::TextServices::{
            ITfCandidateList, ITfFnReconversion, ITfFnReconversion_Impl, ITfFunctionProvider_Impl,
            ITfFunction_Impl, ITfRange,
        },
    },
};

use crate::globals::GUID_TEXT_SERVICE;

use super::factory::TextServiceFactory_Impl;

fn windows_error(hresult: windows::core::HRESULT) -> Error {
    Error::from_hresult(hresult)
}

impl ITfFunctionProvider_Impl for TextServiceFactory_Impl {
    fn GetType(&self) -> windows::core::Result<GUID> {
        Ok(GUID_TEXT_SERVICE)
    }

    fn GetDescription(&self) -> windows::core::Result<BSTR> {
        Ok(BSTR::from("azooKey reconversion"))
    }

    fn GetFunction(
        &self,
        rguid: *const GUID,
        riid: *const GUID,
    ) -> windows::core::Result<IUnknown> {
        let Some(rguid) = (unsafe { rguid.as_ref() }) else {
            return Err(windows_error(E_INVALIDARG));
        };
        let Some(riid) = (unsafe { riid.as_ref() }) else {
            return Err(windows_error(E_INVALIDARG));
        };
        if rguid != &GUID::from_u128(0) || riid != &ITfFnReconversion::IID {
            return Err(windows_error(E_NOINTERFACE));
        }

        let text_service = self.borrow().map_err(|error| {
            tracing::warn!(?error, "Failed to borrow text service for GetFunction");
            windows_error(E_FAIL)
        })?;
        let function = text_service.this::<ITfFnReconversion>().map_err(|error| {
            tracing::warn!(?error, "Failed to expose ITfFnReconversion");
            windows_error(E_NOINTERFACE)
        })?;
        // GetFunction's out parameter is typed as IUnknown**, but COM requires
        // the returned pointer to be the interface requested by riid. Upcast
        // without QueryInterface so the ITfFnReconversion vtable is preserved.
        Ok(function.into())
    }
}

impl ITfFunction_Impl for TextServiceFactory_Impl {
    fn GetDisplayName(&self) -> windows::core::Result<BSTR> {
        Ok(BSTR::from("azooKey reconversion"))
    }
}

impl ITfFnReconversion_Impl for TextServiceFactory_Impl {
    fn QueryRange(
        &self,
        prange: Option<&ITfRange>,
        ppnewrange: *mut Option<ITfRange>,
        pfconvertable: *mut BOOL,
    ) -> windows::core::Result<()> {
        if pfconvertable.is_null() {
            return Err(windows_error(E_INVALIDARG));
        }
        unsafe { pfconvertable.write(false.into()) };

        let Some(range) = prange else {
            return Err(windows_error(E_INVALIDARG));
        };
        if !self.standard_reconversion_is_enabled() {
            if !ppnewrange.is_null() {
                // ppnewrange is a COM out parameter and is not initialized by
                // the caller, so write without trying to drop its old value.
                unsafe { ppnewrange.write(None) };
            }
            return Ok(());
        }

        if !ppnewrange.is_null() {
            unsafe { ppnewrange.write(Some(range.Clone()?)) };
        }
        unsafe { pfconvertable.write(true.into()) };
        Ok(())
    }

    fn GetReconversion(
        &self,
        _prange: Option<&ITfRange>,
    ) -> windows::core::Result<ITfCandidateList> {
        // azooKey owns its candidate window; Reconvert starts that UI path.
        Err(windows_error(E_NOTIMPL))
    }

    fn Reconvert(&self, prange: Option<&ITfRange>) -> windows::core::Result<()> {
        let Some(range) = prange else {
            return Err(windows_error(E_INVALIDARG));
        };
        let context = unsafe { range.GetContext()? };
        match self.handle_standard_reconversion(&context, range) {
            Ok(true) => Ok(()),
            Ok(false) => Err(windows_error(E_FAIL)),
            Err(error) => {
                tracing::warn!(?error, "Standard TSF reconversion failed");
                Err(windows_error(E_FAIL))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::factory::TextServiceFactory;
    use super::*;
    use windows::Win32::UI::TextServices::ITfFunctionProvider;

    #[test]
    fn get_function_returns_the_requested_interface_pointer() -> anyhow::Result<()> {
        let provider = TextServiceFactory::create::<ITfFunctionProvider>()?;
        let returned =
            unsafe { provider.GetFunction(&GUID::from_u128(0), &ITfFnReconversion::IID)? };
        let expected = provider.cast::<ITfFnReconversion>()?;

        assert_eq!(returned.as_raw(), expected.as_raw());
        Ok(())
    }
}
