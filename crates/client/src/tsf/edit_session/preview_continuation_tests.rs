//! Range-aware COM fixture for the atomic native preview transition. This tests
//! TSF call ordering/anchors, not the Windows IMM32 notification implementation.
use crate::{globals::GUID_DISPLAY_ATTRIBUTE, tsf::factory::TextServiceFactory};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use windows::{
    core::{implement, AsImpl, Error, IUnknown, GUID, HRESULT, PCWSTR, PWSTR, VARIANT},
    Win32::{
        Foundation::{BOOL, E_NOTIMPL},
        System::Com::IDataObject,
        UI::TextServices::*,
    },
};

type SharedDocument = Rc<RefCell<Document>>;

struct Document {
    text: Vec<u16>,
    start: i32,
    end: i32,
    selection: i32,
    sessions: usize,
    writes: usize,
    ends: usize,
    committed: Vec<String>,
    movement_limit: Option<i32>,
}

fn unsupported<T>() -> windows::core::Result<T> {
    Err(Error::from(E_NOTIMPL))
}

#[implement(ITfRange)]
struct Range {
    document: SharedDocument,
    start: Cell<i32>,
    end: Cell<i32>,
}

fn range(document: &SharedDocument, start: i32, end: i32) -> ITfRange {
    Range {
        document: document.clone(),
        start: Cell::new(start),
        end: Cell::new(end),
    }
    .into()
}

fn anchor(value: Option<&ITfRange>, position: TfAnchor) -> i32 {
    let value: &Range = unsafe { value.unwrap().as_impl() };
    if position == TF_ANCHOR_START {
        value.start.get()
    } else {
        value.end.get()
    }
}

impl ITfRange_Impl for Range_Impl {
    fn GetText(
        &self,
        _: u32,
        flags: u32,
        output: PWSTR,
        capacity: u32,
        length: *mut u32,
    ) -> windows::core::Result<()> {
        let document = self.document.borrow();
        let start = self.start.get() as usize;
        let count = (self.end.get() as usize - start).min(capacity as usize);
        unsafe {
            std::ptr::copy_nonoverlapping(document.text[start..].as_ptr(), output.0, count);
            *length = count as u32;
        }
        if flags & TF_TF_MOVESTART != 0 {
            self.start.set(self.start.get() + count as i32);
        }
        Ok(())
    }
    fn SetText(&self, _: u32, _: u32, text: &PCWSTR, length: i32) -> windows::core::Result<()> {
        let replacement = unsafe { std::slice::from_raw_parts(text.0, length as usize) };
        let start = self.start.get();
        let end = self.end.get();
        let mut document = self.document.borrow_mut();
        document
            .text
            .splice(start as usize..end as usize, replacement.iter().copied());
        document.end += length - (end - start);
        document.writes += 1;
        self.end.set(start + length);
        Ok(())
    }
    fn ShiftStart(
        &self,
        _: u32,
        requested: i32,
        actual: *mut i32,
        _: *const TF_HALTCOND,
    ) -> windows::core::Result<()> {
        let document = self.document.borrow();
        let shift = requested.min(document.movement_limit.unwrap_or(requested));
        let next = (self.start.get() + shift).clamp(0, document.text.len() as i32);
        unsafe {
            *actual = next - self.start.get();
        }
        self.start.set(next);
        self.end.set(self.end.get().max(next));
        Ok(())
    }
    fn ShiftEnd(
        &self,
        _: u32,
        requested: i32,
        actual: *mut i32,
        _: *const TF_HALTCOND,
    ) -> windows::core::Result<()> {
        let next = (self.end.get() + requested).clamp(0, self.document.borrow().text.len() as i32);
        unsafe {
            *actual = next - self.end.get();
        }
        self.end.set(next);
        self.start.set(self.start.get().min(next));
        Ok(())
    }
    fn Collapse(&self, _: u32, position: TfAnchor) -> windows::core::Result<()> {
        if position == TF_ANCHOR_START {
            self.end.set(self.start.get());
        } else {
            self.start.set(self.end.get());
        }
        Ok(())
    }
    fn CompareStart(
        &self,
        _: u32,
        other: Option<&ITfRange>,
        position: TfAnchor,
    ) -> windows::core::Result<i32> {
        Ok(self.start.get() - anchor(other, position))
    }
    fn CompareEnd(
        &self,
        _: u32,
        other: Option<&ITfRange>,
        position: TfAnchor,
    ) -> windows::core::Result<i32> {
        Ok(self.end.get() - anchor(other, position))
    }
    fn IsEmpty(&self, _: u32) -> windows::core::Result<BOOL> {
        Ok((self.start.get() == self.end.get()).into())
    }
    fn Clone(&self) -> windows::core::Result<ITfRange> {
        Ok(range(&self.document, self.start.get(), self.end.get()))
    }
    fn GetFormattedText(&self, _: u32) -> windows::core::Result<IDataObject> {
        unsupported()
    }
    fn GetEmbedded(
        &self,
        _: u32,
        _: *const GUID,
        _: *const GUID,
    ) -> windows::core::Result<IUnknown> {
        unsupported()
    }
    fn InsertEmbedded(&self, _: u32, _: u32, _: Option<&IDataObject>) -> windows::core::Result<()> {
        unsupported()
    }
    fn ShiftStartToRange(
        &self,
        _: u32,
        _: Option<&ITfRange>,
        _: TfAnchor,
    ) -> windows::core::Result<()> {
        unsupported()
    }
    fn ShiftEndToRange(
        &self,
        _: u32,
        _: Option<&ITfRange>,
        _: TfAnchor,
    ) -> windows::core::Result<()> {
        unsupported()
    }
    fn ShiftStartRegion(&self, _: u32, _: TfShiftDir) -> windows::core::Result<BOOL> {
        unsupported()
    }
    fn ShiftEndRegion(&self, _: u32, _: TfShiftDir) -> windows::core::Result<BOOL> {
        unsupported()
    }
    fn IsEqualStart(
        &self,
        _: u32,
        _: Option<&ITfRange>,
        _: TfAnchor,
    ) -> windows::core::Result<BOOL> {
        unsupported()
    }
    fn IsEqualEnd(&self, _: u32, _: Option<&ITfRange>, _: TfAnchor) -> windows::core::Result<BOOL> {
        unsupported()
    }
    fn AdjustForInsert(&self, _: u32, _: u32) -> windows::core::Result<BOOL> {
        unsupported()
    }
    fn GetGravity(&self, _: *mut TfGravity, _: *mut TfGravity) -> windows::core::Result<()> {
        unsupported()
    }
    fn SetGravity(&self, _: u32, _: TfGravity, _: TfGravity) -> windows::core::Result<()> {
        unsupported()
    }
    fn GetContext(&self) -> windows::core::Result<ITfContext> {
        unsupported()
    }
}

#[implement(ITfComposition)]
struct NativeComposition {
    document: SharedDocument,
}
impl ITfComposition_Impl for NativeComposition_Impl {
    fn GetRange(&self) -> windows::core::Result<ITfRange> {
        let document = self.document.borrow();
        Ok(range(&self.document, document.start, document.end))
    }
    fn ShiftStart(&self, _: u32, next: Option<&ITfRange>) -> windows::core::Result<()> {
        let next = anchor(next, TF_ANCHOR_START);
        let mut document = self.document.borrow_mut();
        let prefix =
            String::from_utf16(&document.text[document.start as usize..next as usize]).unwrap();
        document.committed.push(prefix);
        document.start = next;
        Ok(())
    }
    fn ShiftEnd(&self, _: u32, _: Option<&ITfRange>) -> windows::core::Result<()> {
        unsupported()
    }
    fn EndComposition(&self, _: u32) -> windows::core::Result<()> {
        self.document.borrow_mut().ends += 1;
        Ok(())
    }
}

#[implement(ITfProperty)]
struct Property;
impl ITfReadOnlyProperty_Impl for Property_Impl {
    fn GetType(&self) -> windows::core::Result<GUID> {
        Ok(GUID_PROP_ATTRIBUTE)
    }
    fn EnumRanges(
        &self,
        _: u32,
        _: *mut Option<IEnumTfRanges>,
        _: Option<&ITfRange>,
    ) -> windows::core::Result<()> {
        unsupported()
    }
    fn GetValue(&self, _: u32, _: Option<&ITfRange>) -> windows::core::Result<VARIANT> {
        unsupported()
    }
    fn GetContext(&self) -> windows::core::Result<ITfContext> {
        unsupported()
    }
}
impl ITfProperty_Impl for Property_Impl {
    fn FindRange(
        &self,
        _: u32,
        _: Option<&ITfRange>,
        _: *mut Option<ITfRange>,
        _: TfAnchor,
    ) -> windows::core::Result<()> {
        unsupported()
    }
    fn SetValueStore(
        &self,
        _: u32,
        _: Option<&ITfRange>,
        _: Option<&ITfPropertyStore>,
    ) -> windows::core::Result<()> {
        unsupported()
    }
    fn SetValue(
        &self,
        _: u32,
        _: Option<&ITfRange>,
        _: *const VARIANT,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn Clear(&self, _: u32, _: Option<&ITfRange>) -> windows::core::Result<()> {
        Ok(())
    }
}

#[implement(ITfContext)]
struct Context {
    document: SharedDocument,
}
impl ITfContext_Impl for Context_Impl {
    fn RequestEditSession(
        &self,
        _: u32,
        session: Option<&ITfEditSession>,
        flags: TF_CONTEXT_EDIT_CONTEXT_FLAGS,
    ) -> windows::core::Result<HRESULT> {
        assert_eq!(flags, TF_ES_READWRITE | TF_ES_SYNC);
        self.document.borrow_mut().sessions += 1;
        Ok(unsafe { session.unwrap().DoEditSession(1) }
            .err()
            .map_or(HRESULT(0), |error| error.code()))
    }
    fn SetSelection(
        &self,
        _: u32,
        count: u32,
        selection: *const TF_SELECTION,
    ) -> windows::core::Result<()> {
        assert_eq!(count, 1);
        self.document.borrow_mut().selection =
            anchor(unsafe { (*selection).range.as_ref() }, TF_ANCHOR_END);
        Ok(())
    }
    fn GetProperty(&self, _: *const GUID) -> windows::core::Result<ITfProperty> {
        Ok(Property.into())
    }
    fn InWriteSession(&self, _: u32) -> windows::core::Result<BOOL> {
        unsupported()
    }
    fn GetSelection(
        &self,
        _: u32,
        _: u32,
        _: u32,
        _: *mut TF_SELECTION,
        _: *mut u32,
    ) -> windows::core::Result<()> {
        unsupported()
    }
    fn GetStart(&self, _: u32) -> windows::core::Result<ITfRange> {
        unsupported()
    }
    fn GetEnd(&self, _: u32) -> windows::core::Result<ITfRange> {
        unsupported()
    }
    fn GetActiveView(&self) -> windows::core::Result<ITfContextView> {
        unsupported()
    }
    fn EnumViews(&self) -> windows::core::Result<IEnumTfContextViews> {
        unsupported()
    }
    fn GetStatus(&self) -> windows::core::Result<TS_STATUS> {
        unsupported()
    }
    fn GetAppProperty(&self, _: *const GUID) -> windows::core::Result<ITfReadOnlyProperty> {
        unsupported()
    }
    fn TrackProperties(
        &self,
        _: *const *const GUID,
        _: u32,
        _: *const *const GUID,
        _: u32,
    ) -> windows::core::Result<ITfReadOnlyProperty> {
        unsupported()
    }
    fn EnumProperties(&self) -> windows::core::Result<IEnumTfProperties> {
        unsupported()
    }
    fn GetDocumentMgr(&self) -> windows::core::Result<ITfDocumentMgr> {
        unsupported()
    }
    fn CreateRangeBackup(
        &self,
        _: u32,
        _: Option<&ITfRange>,
    ) -> windows::core::Result<ITfRangeBackup> {
        unsupported()
    }
}

fn fixture(prefix: &str, old: &str) -> (ITfTextInputProcessor, SharedDocument) {
    let start = prefix.encode_utf16().count() as i32;
    let end = start + old.encode_utf16().count() as i32;
    let document = Rc::new(RefCell::new(Document {
        text: format!("{prefix}{old}後").encode_utf16().collect(),
        start,
        end,
        selection: end,
        sessions: 0,
        writes: 0,
        ends: 0,
        committed: Vec::new(),
        movement_limit: None,
    }));
    let processor = TextServiceFactory::create::<ITfTextInputProcessor>().unwrap();
    let factory: &TextServiceFactory = unsafe { processor.as_impl() };
    {
        let mut service = factory.borrow_mut().unwrap();
        service.context = Some(
            Context {
                document: document.clone(),
            }
            .into(),
        );
        service.borrow_mut_composition().unwrap().tip_composition = Some(
            NativeComposition {
                document: document.clone(),
            }
            .into(),
        );
        service
            .display_attribute_atom
            .insert(GUID_DISPLAY_ATTRIBUTE, 1);
    }
    (processor, document)
}

#[test]
fn preview_continuation_commits_frozen_display_in_one_native_write_and_preserves_it_on_later_input()
{
    for (prefix, old, frozen) in [
        ("", "api", "api"),
        ("前😀文", "日本語", "ニホンゴ"),
        ("前", "日本語です", "ニホンゴです"),
    ] {
        let (processor, document) = fixture(prefix, old);
        let factory: &TextServiceFactory = unsafe { processor.as_impl() };
        factory.commit_preview_and_continue(frozen, "w").unwrap();
        {
            let state = document.borrow();
            assert_eq!(state.sessions, 1);
            assert_eq!(state.writes, 1);
            assert_eq!(state.ends, 0);
            assert_eq!(state.committed, [frozen]);
            assert_eq!(
                state.start as usize,
                prefix.encode_utf16().count() + frozen.encode_utf16().count()
            );
            assert_eq!(
                String::from_utf16(&state.text[state.start as usize..state.end as usize]).unwrap(),
                "w"
            );
        }
        factory.set_text("を使う", "").unwrap();
        let state = document.borrow();
        assert_eq!(
            String::from_utf16(&state.text).unwrap(),
            format!("{prefix}{frozen}を使う後")
        );
        assert_eq!(state.committed, [frozen]);
    }
}

#[test]
fn recovered_candidate_repaint_cannot_precede_atomic_frozen_preview_continuation() {
    use crate::engine::client_action::{ClientAction, PreviewInputKind};
    let (processor, document) = fixture("前😀文", "ニホンゴです");
    let factory: &TextServiceFactory = unsafe { processor.as_impl() };
    let continuation = ClientAction::CommitPreviewAndAppend {
        committed: "ニホンゴです".into(),
        text: "w".into(),
        kind: PreviewInputKind::Romaji,
        temporary_latin: false,
    };
    let mut preview = "ニホンゴです".to_string();
    let mut ui_refreshes = 0;
    // Repeated preparation attempts must not replace the selected script with
    // recovered candidate zero, or publish any native write before commitment.
    for _ in 0..2 {
        TextServiceFactory::refresh_recovered_display_before_actions(Some(&continuation), || {
            preview = "日本語です".into();
            factory.set_text(&preview, "")?;
            ui_refreshes += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(document.borrow().writes, 0);
        assert_eq!(preview, "ニホンゴです");
        assert_eq!(ui_refreshes, 0);
    }
    factory.commit_preview_and_continue(&preview, "w").unwrap();
    assert_eq!(document.borrow().sessions, 1);
    assert_eq!(document.borrow().writes, 1);
    assert_eq!(document.borrow().committed, ["ニホンゴです"]);

    let (processor, normal) = fixture("前", "かな");
    let factory: &TextServiceFactory = unsafe { processor.as_impl() };
    let append = ClientAction::AppendText("w".into());
    TextServiceFactory::refresh_recovered_display_before_actions(Some(&append), || {
        factory.set_text("仮名", "")?;
        ui_refreshes += 1;
        Ok(())
    })
    .unwrap();
    assert_eq!(normal.borrow().writes, 1);
    assert_eq!(ui_refreshes, 1);
}

#[test]
fn preview_continuation_short_native_shift_does_not_commit_and_retry_cannot_duplicate_prefix() {
    let (processor, document) = fixture("前😀文", "api");
    let factory: &TextServiceFactory = unsafe { processor.as_impl() };
    document.borrow_mut().movement_limit = Some(1);
    assert!(factory.commit_preview_and_continue("api", "w").is_err());
    assert!(document.borrow().committed.is_empty());
    document.borrow_mut().movement_limit = None;
    factory.commit_preview_and_continue("api", "w").unwrap();
    let state = document.borrow();
    assert_eq!(state.sessions, 2);
    assert_eq!(state.writes, 2);
    assert_eq!(state.committed, ["api"]);
    assert_eq!(String::from_utf16(&state.text).unwrap(), "前😀文apiw後");
}
