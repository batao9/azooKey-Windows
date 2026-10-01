use std::rc::Rc;

use windows::{
    core::{implement, Error, GUID, HRESULT, VARIANT},
    Win32::{
        Foundation::{BOOL, E_NOTIMPL},
        UI::TextServices::{
            IEnumTfContextViews, IEnumTfProperties, ITfCompartment, ITfCompartmentMgr,
            ITfCompartmentMgr_Impl, ITfCompartment_Impl, ITfContext, ITfContextView,
            ITfContext_Impl, ITfDocumentMgr, ITfEditSession, ITfProperty, ITfRange, ITfRangeBackup,
            ITfReadOnlyProperty, TF_CONTEXT_EDIT_CONTEXT_FLAGS, TF_SELECTION, TS_STATUS,
        },
    },
};

fn not_implemented<T>() -> windows::core::Result<T> {
    Err(Error::from(E_NOTIMPL))
}

#[implement(ITfContext, ITfCompartmentMgr)]
pub(super) struct TestContext {
    disabled: bool,
    selection_probe: Option<Rc<dyn Fn()>>,
}

pub(super) fn new(disabled: bool) -> ITfContext {
    TestContext {
        disabled,
        selection_probe: None,
    }
    .into()
}

pub(super) fn new_with_selection_probe(observer: Rc<dyn Fn()>) -> ITfContext {
    TestContext {
        disabled: false,
        selection_probe: Some(observer),
    }
    .into()
}

impl ITfContext_Impl for TestContext_Impl {
    fn RequestEditSession(
        &self,
        _: u32,
        _: Option<&ITfEditSession>,
        _: TF_CONTEXT_EDIT_CONTEXT_FLAGS,
    ) -> windows::core::Result<HRESULT> {
        if let Some(observer) = &self.selection_probe {
            observer();
        }
        not_implemented()
    }

    fn InWriteSession(&self, _: u32) -> windows::core::Result<BOOL> {
        not_implemented()
    }

    fn GetSelection(
        &self,
        _: u32,
        _: u32,
        _: u32,
        _: *mut TF_SELECTION,
        _: *mut u32,
    ) -> windows::core::Result<()> {
        not_implemented()
    }

    fn SetSelection(&self, _: u32, _: u32, _: *const TF_SELECTION) -> windows::core::Result<()> {
        not_implemented()
    }

    fn GetStart(&self, _: u32) -> windows::core::Result<ITfRange> {
        not_implemented()
    }

    fn GetEnd(&self, _: u32) -> windows::core::Result<ITfRange> {
        not_implemented()
    }

    fn GetActiveView(&self) -> windows::core::Result<ITfContextView> {
        not_implemented()
    }

    fn EnumViews(&self) -> windows::core::Result<IEnumTfContextViews> {
        not_implemented()
    }

    fn GetStatus(&self) -> windows::core::Result<TS_STATUS> {
        not_implemented()
    }

    fn GetProperty(&self, _: *const GUID) -> windows::core::Result<ITfProperty> {
        not_implemented()
    }

    fn GetAppProperty(&self, _: *const GUID) -> windows::core::Result<ITfReadOnlyProperty> {
        not_implemented()
    }

    fn TrackProperties(
        &self,
        _: *const *const GUID,
        _: u32,
        _: *const *const GUID,
        _: u32,
    ) -> windows::core::Result<ITfReadOnlyProperty> {
        not_implemented()
    }

    fn EnumProperties(&self) -> windows::core::Result<IEnumTfProperties> {
        not_implemented()
    }

    fn GetDocumentMgr(&self) -> windows::core::Result<ITfDocumentMgr> {
        not_implemented()
    }

    fn CreateRangeBackup(
        &self,
        _: u32,
        _: Option<&ITfRange>,
    ) -> windows::core::Result<ITfRangeBackup> {
        not_implemented()
    }
}

impl ITfCompartmentMgr_Impl for TestContext_Impl {
    fn GetCompartment(&self, _: *const GUID) -> windows::core::Result<ITfCompartment> {
        Ok(TestCompartment {
            disabled: self.disabled,
        }
        .into())
    }

    fn ClearCompartment(&self, _: u32, _: *const GUID) -> windows::core::Result<()> {
        not_implemented()
    }

    fn EnumCompartments(&self) -> windows::core::Result<windows::Win32::System::Com::IEnumGUID> {
        not_implemented()
    }
}

#[implement(ITfCompartment)]
struct TestCompartment {
    disabled: bool,
}

impl ITfCompartment_Impl for TestCompartment_Impl {
    fn SetValue(&self, _: u32, _: *const VARIANT) -> windows::core::Result<()> {
        not_implemented()
    }

    fn GetValue(&self) -> windows::core::Result<VARIANT> {
        Ok(VARIANT::from(if self.disabled { 1_i32 } else { 0_i32 }))
    }
}
