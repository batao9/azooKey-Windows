//! Apartment-bound TSF candidate element. Do not use `#[implement]` here: in
//! windows-implement 0.58 it advertises IAgileObject even for Rc-backed objects.

use std::{
    ffi::c_void,
    ptr::{self, NonNull},
    rc::Rc,
    sync::{
        atomic::{AtomicU32, AtomicUsize, Ordering},
        Arc,
    },
};

use windows::{
    core::{
        ComObject, ComObjectInner, Error, IUnknown, IUnknownImpl, Interface, Result, BSTR, GUID,
        HRESULT,
    },
    Win32::{
        Foundation::{
            BOOL, E_FAIL, E_INVALIDARG, E_NOINTERFACE, E_POINTER, RPC_E_WRONG_THREAD, S_FALSE,
        },
        System::Threading::GetCurrentThreadId,
        UI::TextServices::{
            ITfCandidateListUIElement, ITfCandidateListUIElement_Impl,
            ITfCandidateListUIElement_Vtbl, ITfDocumentMgr, ITfUIElement, ITfUIElement_Impl,
        },
    },
};

use super::candidate_ui::{element_pages_changed, element_show, ElementState};

const CANDIDATE_UI_GUID: GUID = GUID::from_u128(0x52e5ea5b_667a_4b82_885c_d80d7d08c848);

// The interface pointer is the address of this first field. All three supported
// IIDs share this single pointer, so QI(IUnknown) has a stable COM identity.
#[repr(C)]
pub(super) struct CandidateElement {
    vtable: *const ITfCandidateListUIElement_Vtbl,
    references: AtomicU32,
    owner_thread: u32,
    state: Rc<ElementState>,
    // Declared after state so document-manager references are released before
    // the DLL becomes unloadable, including references retained by the host.
    _module: ModuleReference,
}

struct ModuleReference(Arc<AtomicUsize>);
impl Drop for ModuleReference {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl CandidateElement {
    const VTABLE: ITfCandidateListUIElement_Vtbl = ITfCandidateListUIElement_Vtbl::new::<Self, 0>();

    pub(super) fn create(
        state: Rc<ElementState>,
        module_references: Arc<AtomicUsize>,
    ) -> ITfCandidateListUIElement {
        module_references.fetch_add(1, Ordering::SeqCst);
        let element = Box::new(Self {
            vtable: &Self::VTABLE,
            references: AtomicU32::new(1),
            owner_thread: unsafe { GetCurrentThreadId() },
            state,
            _module: ModuleReference(module_references),
        });
        // Box owns the initial COM reference; Release reclaims it at zero.
        unsafe { ITfCandidateListUIElement::from_raw(Box::into_raw(element).cast()) }
    }

    fn check_thread(&self) -> Result<()> {
        if unsafe { GetCurrentThreadId() } == self.owner_thread {
            Ok(())
        } else {
            Err(Error::from_hresult(RPC_E_WRONG_THREAD))
        }
    }
}

// The generated method thunks dispatch to these traits, but IUnknown below
// deliberately exposes only classic COM interfaces (not WinRT/agile/FTM).
impl ITfUIElement_Impl for CandidateElement {
    fn GetDescription(&self) -> Result<BSTR> {
        self.check_thread()?;
        Ok(BSTR::from("azooKey candidates"))
    }

    fn GetGUID(&self) -> Result<GUID> {
        self.check_thread()?;
        Ok(CANDIDATE_UI_GUID)
    }

    fn Show(&self, show: BOOL) -> Result<()> {
        self.check_thread()?;
        element_show(&self.state, show.as_bool())
    }

    fn IsShown(&self) -> Result<BOOL> {
        self.check_thread()?;
        Ok(BOOL::from(self.state.shown.get()))
    }
}

impl ITfCandidateListUIElement_Impl for CandidateElement {
    fn GetUpdatedFlags(&self) -> Result<u32> {
        self.check_thread()?;
        Ok(self.state.reported_flags.get())
    }

    fn GetDocumentMgr(&self) -> Result<ITfDocumentMgr> {
        self.check_thread()?;
        self.state
            .document_mgr
            .try_borrow()
            .map_err(|_| Error::from_hresult(E_FAIL))?
            .clone()
            .ok_or_else(|| Error::from_hresult(E_FAIL))
    }

    fn GetCount(&self) -> Result<u32> {
        self.check_thread()?;
        let snapshot = self
            .state
            .snapshot
            .try_borrow()
            .map_err(|_| Error::from_hresult(E_FAIL))?;
        u32::try_from(snapshot.candidates.len()).map_err(|_| Error::from_hresult(E_FAIL))
    }

    fn GetSelection(&self) -> Result<u32> {
        self.check_thread()?;
        let snapshot = self
            .state
            .snapshot
            .try_borrow()
            .map_err(|_| Error::from_hresult(E_FAIL))?;
        u32::try_from(snapshot.selected_index).map_err(|_| Error::from_hresult(E_FAIL))
    }

    fn GetString(&self, index: u32) -> Result<BSTR> {
        self.check_thread()?;
        let snapshot = self
            .state
            .snapshot
            .try_borrow()
            .map_err(|_| Error::from_hresult(E_FAIL))?;
        snapshot
            .candidates
            .get(index as usize)
            .map(|text| BSTR::from(text.as_str()))
            .ok_or_else(|| Error::from_hresult(E_INVALIDARG))
    }

    fn GetPageIndex(&self, indices: *mut u32, capacity: u32, page_count: *mut u32) -> Result<()> {
        self.check_thread()?;
        if page_count.is_null() || (capacity != 0 && indices.is_null()) {
            return Err(Error::from_hresult(E_POINTER));
        }
        let pages = self
            .state
            .pages
            .try_borrow()
            .map_err(|_| Error::from_hresult(E_FAIL))?;
        unsafe {
            *page_count = pages.len() as u32;
            // TSF permits a count-only query, including a null zero-length buffer.
            if capacity > 0 {
                ptr::copy_nonoverlapping(
                    pages.as_ptr(),
                    indices,
                    pages.len().min(capacity as usize),
                );
            }
        }
        if (capacity as usize) < pages.len() {
            // Preserve this successful-but-incomplete HRESULT at the raw COM ABI.
            return Err(Error::from_hresult(S_FALSE));
        }
        Ok(())
    }

    fn SetPageIndex(&self, indices: *const u32, count: u32) -> Result<()> {
        self.check_thread()?;
        let candidate_count = self.GetCount()?;
        if count == 0 || count > candidate_count {
            return Err(Error::from_hresult(E_INVALIDARG));
        }
        if indices.is_null() {
            return Err(Error::from_hresult(E_POINTER));
        }
        let indices = unsafe { std::slice::from_raw_parts(indices, count as usize) };
        if indices[0] != 0
            || indices
                .last()
                .is_some_and(|index| *index >= candidate_count)
            || indices.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(Error::from_hresult(E_INVALIDARG));
        }
        *self
            .state
            .pages
            .try_borrow_mut()
            .map_err(|_| Error::from_hresult(E_FAIL))? = indices.to_vec();
        element_pages_changed(&self.state)
    }

    fn GetCurrentPage(&self) -> Result<u32> {
        self.check_thread()?;
        let selected = self.GetSelection()?;
        let pages = self
            .state
            .pages
            .try_borrow()
            .map_err(|_| Error::from_hresult(E_FAIL))?;
        Ok(pages
            .partition_point(|index| *index <= selected)
            .saturating_sub(1) as u32)
    }
}

// The generated vtable uses IUnknownImpl for its three base methods. Its
// interface offset is zero: no IInspectable identity or weak-reference table.
impl IUnknownImpl for CandidateElement {
    type Impl = Self;

    fn get_impl(&self) -> &Self {
        self
    }
    fn get_impl_mut(&mut self) -> &mut Self {
        self
    }
    fn into_inner(self) -> Self {
        self
    }

    unsafe fn QueryInterface(&self, iid: *const GUID, interface: *mut *mut c_void) -> HRESULT {
        if interface.is_null() {
            return E_POINTER;
        }
        *interface = ptr::null_mut();
        if iid.is_null() {
            return E_POINTER;
        }
        if *iid != IUnknown::IID
            && *iid != ITfUIElement::IID
            && *iid != ITfCandidateListUIElement::IID
        {
            return E_NOINTERFACE;
        }
        *interface = self as *const Self as *mut c_void;
        self.AddRef();
        HRESULT(0)
    }

    fn AddRef(&self) -> u32 {
        self.references.fetch_add(1, Ordering::Relaxed) + 1
    }

    unsafe fn Release(this: *mut Self) -> u32 {
        let remaining = (*this).references.fetch_sub(1, Ordering::Release) - 1;
        if remaining == 0 {
            std::sync::atomic::fence(Ordering::Acquire);
            // COM apartment rules require the final reference to be released on
            // the owning apartment (including after cross-apartment marshaling).
            drop(Box::from_raw(this));
        }
        remaining
    }

    fn is_reference_count_one(&self) -> bool {
        self.references.load(Ordering::Acquire) == 1
    }

    unsafe fn GetTrustLevel(&self, value: *mut i32) -> HRESULT {
        if value.is_null() {
            E_POINTER
        } else {
            E_NOINTERFACE
        }
    }

    unsafe fn from_inner_ref(inner: &Self) -> &Self {
        inner
    }

    fn to_object(&self) -> ComObject<Self> {
        self.AddRef();
        unsafe { ComObject::from_raw(NonNull::new_unchecked(self as *const Self as *mut Self)) }
    }

    const INNER_OFFSET_IN_POINTERS: usize = 0;
}

impl ComObjectInner for CandidateElement {
    type Outer = Self;

    fn into_object(self) -> ComObject<Self> {
        unsafe { ComObject::from_raw(NonNull::new_unchecked(Box::into_raw(Box::new(self)))) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use windows::core::{imp::IAgileObject, IInspectable};

    fn state() -> Rc<ElementState> {
        Rc::new(ElementState {
            snapshot: RefCell::new(Rc::new(super::super::candidate_ui::Snapshot {
                candidates: vec!["one".into(), "two".into()],
                selected_index: 1,
                ..Default::default()
            })),
            document_mgr: RefCell::new(None),
            pages: RefCell::new(vec![0]),
            reported_flags: Cell::new(0),
            show: Cell::new(false),
            shown: Cell::new(false),
        })
    }

    #[test]
    fn no_agile_claim_and_stable_unknown_identity() {
        let module = Arc::new(AtomicUsize::new(0));
        let element = CandidateElement::create(state(), module.clone());
        assert_eq!(module.load(Ordering::SeqCst), 1);
        assert_eq!(
            element.cast::<IAgileObject>().unwrap_err().code(),
            E_NOINTERFACE
        );
        assert_eq!(
            element.cast::<IInspectable>().unwrap_err().code(),
            E_NOINTERFACE
        );
        let marshal_iid = GUID::from_u128(0x00000003_0000_0000_c000_000000000046);
        let mut marshal = ptr::null_mut();
        assert_eq!(
            unsafe {
                (element.vtable().base__.base__.QueryInterface)(
                    element.as_raw(),
                    &marshal_iid,
                    &mut marshal,
                )
            },
            E_NOINTERFACE
        );
        assert!(marshal.is_null());
        let unknown: IUnknown = element.cast().unwrap();
        let ui: ITfUIElement = element.cast().unwrap();
        assert_eq!(unknown.as_raw(), ui.cast::<IUnknown>().unwrap().as_raw());
        assert_eq!(
            unknown.as_raw(),
            element.cast::<IUnknown>().unwrap().as_raw()
        );
        let mut out = ptr::dangling_mut::<c_void>();
        let vtbl = element.vtable();
        assert_eq!(
            unsafe { (vtbl.base__.base__.QueryInterface)(element.as_raw(), ptr::null(), &mut out) },
            E_POINTER
        );
        assert!(out.is_null());
        assert_eq!(
            unsafe {
                (vtbl.base__.base__.QueryInterface)(
                    element.as_raw(),
                    &IUnknown::IID,
                    ptr::null_mut(),
                )
            },
            E_POINTER
        );
        drop(element);
        assert_eq!(module.load(Ordering::SeqCst), 1);
        drop(ui);
        drop(unknown);
        assert_eq!(module.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn pages_and_indices_are_validated() {
        let element = CandidateElement::create(state(), Arc::new(AtomicUsize::new(0)));
        let vtbl = element.vtable();
        let mut count = 0;
        assert_eq!(
            unsafe { (vtbl.GetPageIndex)(element.as_raw(), ptr::null_mut(), 0, &mut count) },
            S_FALSE
        );
        assert_eq!(count, 1);
        unsafe {
            element.SetPageIndex(&[0, 1]).unwrap();
        }
        let mut indices = [u32::MAX; 2];
        assert_eq!(
            unsafe { (vtbl.GetPageIndex)(element.as_raw(), indices.as_mut_ptr(), 1, &mut count) },
            S_FALSE
        );
        assert_eq!(indices, [0, u32::MAX]);
        assert_eq!(count, 2);
        assert_eq!(
            unsafe { (vtbl.GetPageIndex)(element.as_raw(), indices.as_mut_ptr(), 2, &mut count) },
            HRESULT(0)
        );
        assert_eq!(indices, [0, 1]);
        assert_eq!(count, 2);
        assert_eq!(
            unsafe { (vtbl.GetPageIndex)(element.as_raw(), ptr::null_mut(), 1, &mut count) },
            E_POINTER
        );
        assert_eq!(
            unsafe { (vtbl.GetPageIndex)(element.as_raw(), ptr::null_mut(), 0, ptr::null_mut()) },
            E_POINTER
        );
        assert_eq!(
            unsafe { element.GetString(2).unwrap_err().code() },
            E_INVALIDARG
        );
        assert_eq!(
            unsafe { element.SetPageIndex(&[0, 2]).unwrap_err().code() },
            E_INVALIDARG
        );
        assert_eq!(unsafe { element.GetCurrentPage().unwrap() }, 1);
    }

    #[test]
    fn direct_foreign_thread_call_rejected_before_state_access() {
        let state = state();
        let element = CandidateElement::create(state.clone(), Arc::new(AtomicUsize::new(0)));
        let borrow = state.snapshot.borrow_mut();
        // Pass only the raw address to the test worker; COM interface wrappers
        // are intentionally !Send. Keep the owning reference alive until join.
        let address = element.as_raw() as usize;
        let result = std::thread::spawn(move || {
            let raw = address as *mut c_void;
            let vtbl = unsafe { *(raw as *const *const ITfCandidateListUIElement_Vtbl) };
            let mut count = 0;
            unsafe { ((*vtbl).GetCount)(raw, &mut count) }
        })
        .join()
        .unwrap();
        assert_eq!(result, RPC_E_WRONG_THREAD);
        drop(borrow);
        assert_eq!(unsafe { element.GetCount().unwrap() }, 2);
    }
}
