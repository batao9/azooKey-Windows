use macros::anyhow;
use windows::{
    core::{implement, AsImpl, IUnknown, Interface, HRESULT, VARIANT},
    Win32::{
        Foundation::RECT,
        UI::TextServices::{
            ITfComposition, ITfCompositionSink, ITfContext, ITfContextComposition, ITfEditSession,
            ITfEditSession_Impl, ITfInsertAtSelection, ITfRange, ITfTextInputProcessor,
            GUID_PROP_ATTRIBUTE, TF_AE_NONE, TF_ANCHOR_END, TF_ANCHOR_START, TF_ES_ASYNC,
            TF_ES_READ, TF_ES_READWRITE, TF_ES_SYNC, TF_IAS_QUERYONLY, TF_SELECTION,
            TF_SELECTIONSTYLE, TF_ST_CORRECTION, TF_S_ASYNC, TF_TF_MOVESTART,
        },
    },
};

use std::{cell::Cell, fmt, mem::ManuallyDrop, rc::Rc, time::Instant};

use anyhow::Result;

use crate::{
    engine::{ipc_service::current_input_trace_request_id, state::IMEState},
    extension::StringExt as _,
    globals::GUID_DISPLAY_ATTRIBUTE,
};
use shared::proto::WindowPosition;

use super::factory::TextServiceFactory;

#[derive(Clone, Copy)]
enum CandidateWindowPositionMode {
    Force,
    Throttled,
}

#[implement(ITfEditSession)]
struct EditSession<'a, T> {
    callback: Rc<dyn Fn(u32) -> anyhow::Result<T>>,
    pub result: Cell<Option<T>>,
    phantom: std::marker::PhantomData<&'a T>,
}

#[implement(ITfEditSession)]
struct AsyncEditSession {
    callback: Rc<dyn Fn(u32) -> anyhow::Result<()>>,
    completion: Rc<dyn Fn()>,
    completed: Cell<bool>,
}

impl AsyncEditSession {
    fn complete(&self) {
        if !self.completed.replace(true) {
            (self.completion)();
        }
    }
}

impl Drop for AsyncEditSession {
    fn drop(&mut self) {
        self.complete();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditSessionFailure {
    Request(HRESULT),
    Session(HRESULT),
    UnexpectedAsync,
    CallbackNotCompleted,
}

impl fmt::Display for EditSessionFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Request(hresult) => {
                write!(formatter, "RequestEditSession failed: {hresult:?}")
            }
            Self::Session(hresult) => {
                write!(formatter, "edit session was rejected: {hresult:?}")
            }
            Self::UnexpectedAsync => {
                write!(formatter, "synchronous edit session was deferred")
            }
            Self::CallbackNotCompleted => {
                write!(formatter, "edit session completed without callback result")
            }
        }
    }
}

impl std::error::Error for EditSessionFailure {}

pub(crate) fn is_non_destructive_edit_session_error(error: &anyhow::Error) -> bool {
    error.downcast_ref::<EditSessionFailure>().is_some()
}

fn complete_sync_edit_session<T>(
    request_result: std::result::Result<HRESULT, HRESULT>,
    callback_result: Option<T>,
) -> Result<T> {
    let session_result = request_result
        .map_err(|hresult| anyhow::Error::new(EditSessionFailure::Request(hresult)))?;
    if session_result == TF_S_ASYNC {
        return Err(anyhow::Error::new(EditSessionFailure::UnexpectedAsync));
    }
    session_result
        .ok()
        .map_err(|_| anyhow::Error::new(EditSessionFailure::Session(session_result)))?;
    callback_result.ok_or_else(|| anyhow::Error::new(EditSessionFailure::CallbackNotCompleted))
}

fn complete_async_edit_session_request(
    request_result: std::result::Result<HRESULT, HRESULT>,
) -> Result<()> {
    let session_result = request_result
        .map_err(|hresult| anyhow::Error::new(EditSessionFailure::Request(hresult)))?;
    session_result
        .ok()
        .map_err(|_| anyhow::Error::new(EditSessionFailure::Session(session_result)))
}

fn sync_edit_session<T>(
    client_id: u32,
    context: ITfContext,
    access: windows::Win32::UI::TextServices::TF_CONTEXT_EDIT_CONTEXT_FLAGS,
    callback: Rc<dyn Fn(u32) -> anyhow::Result<T>>,
) -> Result<T> {
    let session: ITfEditSession = EditSession {
        callback,
        result: Cell::new(None),
        phantom: std::marker::PhantomData,
    }
    .into();

    let request_result =
        unsafe { context.RequestEditSession(client_id, &session, access | TF_ES_SYNC) }
            .map_err(|error| error.code());
    let session: &EditSession<'_, T> =
        unsafe { <ITfEditSession as AsImpl<EditSession<'_, T>>>::as_impl(&session) };
    complete_sync_edit_session(request_result, session.result.take())
}

pub fn read_edit_session<T>(
    client_id: u32,
    context: ITfContext,
    callback: Rc<dyn Fn(u32) -> anyhow::Result<T>>,
) -> Result<T> {
    sync_edit_session(client_id, context, TF_ES_READ, callback)
}

pub fn write_edit_session<T>(
    client_id: u32,
    context: ITfContext,
    callback: Rc<dyn Fn(u32) -> anyhow::Result<T>>,
) -> Result<T> {
    sync_edit_session(client_id, context, TF_ES_READWRITE, callback)
}

fn request_async_edit_session(
    client_id: u32,
    context: ITfContext,
    access: windows::Win32::UI::TextServices::TF_CONTEXT_EDIT_CONTEXT_FLAGS,
    callback: Rc<dyn Fn(u32) -> anyhow::Result<()>>,
    completion: Rc<dyn Fn()>,
) -> Result<()> {
    let session: ITfEditSession = AsyncEditSession {
        callback,
        completion,
        completed: Cell::new(false),
    }
    .into();
    let request_result =
        unsafe { context.RequestEditSession(client_id, &session, access | TF_ES_ASYNC) }
            .map_err(|error| error.code());
    complete_async_edit_session_request(request_result)
}

fn request_async_read_edit_session(
    client_id: u32,
    context: ITfContext,
    callback: Rc<dyn Fn(u32) -> anyhow::Result<()>>,
    completion: Rc<dyn Fn()>,
) -> Result<()> {
    request_async_edit_session(client_id, context, TF_ES_READ, callback, completion)
}

fn request_async_write_edit_session(
    client_id: u32,
    context: ITfContext,
    callback: Rc<dyn Fn(u32) -> anyhow::Result<()>>,
) -> Result<()> {
    request_async_edit_session(
        client_id,
        context,
        TF_ES_READWRITE,
        callback,
        Rc::new(|| {}),
    )
}

impl<'a, T> ITfEditSession_Impl for EditSession_Impl<'a, T> {
    #[anyhow]
    fn DoEditSession(&self, cookie: u32) -> Result<()> {
        let result = (self.callback)(cookie)?;
        self.result.set(Some(result));
        Ok(())
    }
}

impl ITfEditSession_Impl for AsyncEditSession_Impl {
    #[anyhow]
    fn DoEditSession(&self, cookie: u32) -> Result<()> {
        let result = (self.callback)(cookie);
        self.complete();
        result
    }
}

fn close_composition_callback(
    composition: ITfComposition,
    context: ITfContext,
    discard_text: bool,
) -> Rc<dyn Fn(u32) -> anyhow::Result<()>> {
    Rc::new(move |cookie| unsafe {
        let range: ITfRange = composition.GetRange()?;

        if discard_text {
            range.SetText(cookie, TF_ST_CORRECTION, &[])?;
        } else {
            let mut text = vec![0; 1024];
            let mut text_len = 1024;

            let range_new = range.Clone()?;
            range_new.GetText(cookie, TF_TF_MOVESTART, &mut text, &mut text_len)?;

            text = text[..text_len as usize].to_vec();
            range.SetText(cookie, TF_ST_CORRECTION, &text)?;
        }

        let prop = context.GetProperty(&GUID_PROP_ATTRIBUTE)?;
        prop.Clear(cookie, &range)?;

        range.Collapse(cookie, TF_ANCHOR_END)?;
        let selection = TF_SELECTION {
            range: ManuallyDrop::new(Some(range.clone())),
            style: TF_SELECTIONSTYLE {
                ase: TF_AE_NONE,
                fInterimChar: false.into(),
            },
        };

        context.SetSelection(cookie, &[selection])?;
        composition.EndComposition(cookie)?;
        Ok(())
    })
}

fn has_same_com_identity<I: Interface>(left: &I, right: &I) -> bool {
    match (left.cast::<IUnknown>(), right.cast::<IUnknown>()) {
        (Ok(left), Ok(right)) => left.as_raw() == right.as_raw(),
        (Err(left_error), Err(right_error)) => {
            tracing::warn!(?left_error, ?right_error, "Failed to query COM identities");
            false
        }
        (Err(error), _) | (_, Err(error)) => {
            tracing::warn!(?error, "Failed to query COM identity");
            false
        }
    }
}

impl TextServiceFactory {
    fn log_candidate_window_position_performance(
        request_id: u64,
        stage: &str,
        start: Instant,
        details: impl Into<String>,
    ) {
        if let Ok(Some(ipc_service)) = IMEState::ipc_service() {
            ipc_service.log_client_performance(
                request_id,
                "candidate_window_position",
                stage,
                start.elapsed(),
                details.into(),
            );
        }
    }

    fn close_composition(&self, discard_text: bool) -> Result<()> {
        let text_service = self.borrow()?;

        if let Some(composition) = text_service.borrow_composition()?.tip_composition.clone() {
            write_edit_session(
                text_service.tid,
                text_service.context()?,
                close_composition_callback(
                    composition,
                    text_service.context::<ITfContext>()?,
                    discard_text,
                ),
            )?;
        } else {
            tracing::warn!("Composition is not started");
        }

        text_service.borrow_mut_composition()?.tip_composition = None;

        Ok(())
    }

    pub(crate) fn end_composition_async_best_effort(&self) {
        let request_result: Result<()> = (|| {
            let text_service = self.borrow()?;
            let Some(composition) = text_service.borrow_composition()?.tip_composition.clone()
            else {
                return Ok(());
            };
            let context = text_service.context::<ITfContext>()?;
            let tid = text_service.tid;
            request_async_write_edit_session(
                tid,
                context.clone(),
                close_composition_callback(composition, context, false),
            )
        })();

        if let Err(error) = request_result {
            tracing::warn!(?error, "Failed to request best-effort composition end");
        }

        if let Ok(text_service) = self.borrow() {
            if let Ok(mut composition) = text_service.borrow_mut_composition() {
                composition.tip_composition = None;
            }
        }
    }

    #[tracing::instrument]
    pub fn start_composition(&self) -> Result<()> {
        tracing::debug!("start_composition");

        let text_service = self.borrow_mut()?;
        let context = text_service.context()?;
        let context_composition = text_service.context::<ITfContextComposition>()?;
        let sink = text_service.this::<ITfCompositionSink>()?;
        let insert = text_service.context::<ITfInsertAtSelection>()?;

        let tip_exists = {
            let composition = text_service.borrow_composition()?;
            composition.tip_composition.is_some()
        };

        if tip_exists {
            self.end_composition()?;
            return Ok(());
        }

        let composition = write_edit_session::<ITfComposition>(
            text_service.tid,
            context,
            Rc::new({
                move |cookie| unsafe {
                    let range = insert.InsertTextAtSelection(cookie, TF_IAS_QUERYONLY, &[])?;
                    let composition =
                        context_composition.StartComposition(cookie, &range, &sink)?;

                    Ok(composition)
                }
            }),
        )?;

        tracing::debug!("Composition started {composition:?}");
        text_service.borrow_mut_composition()?.tip_composition = Some(composition);

        Ok(())
    }

    #[tracing::instrument]
    pub fn end_composition(&self) -> Result<()> {
        tracing::debug!("end_composition");
        self.close_composition(false)
    }

    #[tracing::instrument]
    pub fn abort_composition(&self) -> Result<()> {
        tracing::debug!("abort_composition");
        self.close_composition(true)
    }

    #[tracing::instrument]
    pub fn discard_composition_text(&self) -> Result<()> {
        tracing::debug!("discard_composition_text");
        self.close_composition(true)
    }

    #[tracing::instrument]
    pub fn set_text(&self, text: &str, subtext: &str) -> Result<()> {
        let text_service = self.borrow()?;

        if let Some(composition) = text_service.borrow_composition()?.tip_composition.clone() {
            write_edit_session(
                text_service.tid,
                text_service.context()?,
                Rc::new({
                    let text_len = text.chars().count() as i32;

                    // unpadded is all you need!
                    let text = format!("{text}{subtext}").as_str().to_wide_16_unpadded();
                    let context = text_service.context::<ITfContext>()?;
                    let display_attribute_atom = text_service.display_attribute_atom.clone();

                    move |cookie| unsafe {
                        let range = composition.GetRange()?;
                        range.SetText(cookie, TF_ST_CORRECTION, &text)?;

                        // first, set the display attribute to the "text" part
                        let text_range = range.Clone()?;
                        text_range.Collapse(cookie, TF_ANCHOR_START)?;
                        let mut shifted: i32 = 0;
                        text_range.ShiftEnd(cookie, text_len, &mut shifted, std::ptr::null())?;
                        let display_attribute = display_attribute_atom.get(&GUID_DISPLAY_ATTRIBUTE);
                        if let Some(display_attribute) = display_attribute {
                            let pvar = VARIANT::from(*display_attribute as i32);
                            let prop = context.GetProperty(&GUID_PROP_ATTRIBUTE)?;
                            prop.SetValue(cookie, &text_range, &pvar)?;
                        }

                        range.Collapse(cookie, TF_ANCHOR_END)?;
                        let selection = TF_SELECTION {
                            range: ManuallyDrop::new(Some(range.clone())),
                            style: TF_SELECTIONSTYLE {
                                ase: TF_AE_NONE,
                                fInterimChar: false.into(),
                            },
                        };

                        context.SetSelection(cookie, &[selection])?;

                        Ok(())
                    }
                }),
            )?;
        } else {
            tracing::warn!("Composition is not started");
        }

        Ok(())
    }

    #[tracing::instrument]
    pub fn shift_start(&self, text: &str, subtext: &str) -> Result<()> {
        let text_service = self.borrow()?;

        if let Some(composition) = text_service.borrow_composition()?.tip_composition.clone() {
            write_edit_session(
                text_service.tid,
                text_service.context()?,
                Rc::new({
                    let text_len = text.chars().count() as i32;
                    let subtext = subtext.to_wide_16_unpadded();
                    let context = text_service.context::<ITfContext>()?;
                    let display_attribute_atom = text_service.display_attribute_atom.clone();

                    move |cookie| unsafe {
                        // first, shift the start of the composition
                        let range = composition.GetRange()?;
                        let mut shifted: i32 = 0;

                        // and clear the display attribute
                        let prop = context.GetProperty(&GUID_PROP_ATTRIBUTE)?;
                        prop.Clear(cookie, &range)?;

                        range.Collapse(cookie, TF_ANCHOR_START)?;
                        range.ShiftStart(cookie, text_len, &mut shifted, std::ptr::null())?;

                        composition.ShiftStart(cookie, &range)?;

                        // then, set the display attribute
                        let range = composition.GetRange()?;

                        range.SetText(cookie, TF_ST_CORRECTION, &subtext)?;

                        let display_attribute = display_attribute_atom.get(&GUID_DISPLAY_ATTRIBUTE);
                        if let Some(display_attribute) = display_attribute {
                            let pvar = VARIANT::from(*display_attribute as i32);
                            let prop = context.GetProperty(&GUID_PROP_ATTRIBUTE)?;
                            prop.SetValue(cookie, &range, &pvar)?;
                        }

                        range.Collapse(cookie, TF_ANCHOR_END)?;
                        let selection = TF_SELECTION {
                            range: ManuallyDrop::new(Some(range)),
                            style: TF_SELECTIONSTYLE {
                                ase: TF_AE_NONE,
                                fInterimChar: false.into(),
                            },
                        };

                        context.SetSelection(cookie, &[selection])?;

                        Ok(())
                    }
                }),
            )?;
        } else {
            tracing::warn!("Composition is not started");
        }

        Ok(())
    }

    fn candidate_window_position_with_mode(
        &self,
        mode: CandidateWindowPositionMode,
    ) -> Result<Option<WindowPosition>> {
        let trace_request_id = current_input_trace_request_id();
        let total_start = trace_request_id.map(|_| Instant::now());
        {
            let mut text_service = match self.borrow_mut() {
                Ok(text_service) => text_service,
                Err(error) => {
                    tracing::warn!(
                        "Skip candidate_window_position due to borrow conflict: {error:?}"
                    );
                    if let (Some(request_id), Some(total_start)) = (trace_request_id, total_start) {
                        Self::log_candidate_window_position_performance(
                            request_id,
                            "total",
                            total_start,
                            format!("status=skipped;reason=borrow_conflict;error={error:?}"),
                        );
                    }
                    return Ok(None);
                }
            };

            let now = Instant::now();
            if matches!(mode, CandidateWindowPositionMode::Throttled)
                && text_service
                    .candidate_window_position_state
                    .should_throttle(now)
            {
                tracing::debug!("Skip throttled candidate_window_position call");
                if let (Some(request_id), Some(total_start)) = (trace_request_id, total_start) {
                    Self::log_candidate_window_position_performance(
                        request_id,
                        "total",
                        total_start,
                        "status=skipped;reason=throttled".to_string(),
                    );
                }
                return Ok(None);
            }

            if !text_service.update_pos_state.try_begin_update(now) {
                tracing::debug!("Skip re-entrant candidate_window_position call");
                if let (Some(request_id), Some(total_start)) = (trace_request_id, total_start) {
                    Self::log_candidate_window_position_performance(
                        request_id,
                        "total",
                        total_start,
                        "status=skipped;reason=reentrant".to_string(),
                    );
                }
                return Ok(None);
            }
            text_service
                .candidate_window_position_state
                .mark_attempt(now);
        }

        let result: Result<Option<WindowPosition>> = (|| {
            let (tid, context, tip_composition) = {
                let text_service = self.borrow()?;
                let composition = text_service.borrow_composition()?;
                (
                    text_service.tid,
                    text_service.context::<ITfContext>()?,
                    composition.tip_composition.clone(),
                )
            };

            if let Some(tip_composition) = tip_composition {
                let position = read_edit_session(
                    tid,
                    context.clone(),
                    Rc::new({
                        let context = context.clone();

                        move |cookie| unsafe {
                            let view = context.GetActiveView()?;
                            let range = tip_composition.GetRange()?;

                            let mut rect = RECT::default();
                            let mut clipped = false.into();
                            view.GetTextExt(cookie, &range, &mut rect, &mut clipped)?;

                            Ok(WindowPosition {
                                top: rect.top,
                                left: rect.left,
                                bottom: rect.bottom,
                                right: rect.right,
                            })
                        }
                    }),
                )?;
                Ok(Some(position))
            } else {
                Ok(None)
            }
        })();

        match self.borrow_mut() {
            Ok(mut text_service) => {
                text_service.update_pos_state.finish_update(Instant::now());
            }
            Err(error) => {
                tracing::warn!("Failed to reset update_pos guard: {error:?}");
            }
        }

        if let (Some(request_id), Some(total_start)) = (trace_request_id, total_start) {
            let details = match &result {
                Ok(position) => format!("status=success;position_present={}", position.is_some()),
                Err(error) => format!("status=error;error={error:?}"),
            };
            Self::log_candidate_window_position_performance(
                request_id,
                "total",
                total_start,
                details,
            );
        }

        match result {
            Ok(position) => Ok(position),
            Err(error) => {
                tracing::warn!("Failed to obtain composition window position: {error:?}");
                Ok(None)
            }
        }
    }

    #[tracing::instrument]
    pub fn candidate_window_position(&self) -> Result<Option<WindowPosition>> {
        self.candidate_window_position_with_mode(CandidateWindowPositionMode::Force)
    }

    pub(crate) fn candidate_window_position_for_update(&self) -> Result<Option<WindowPosition>> {
        self.candidate_window_position_with_mode(CandidateWindowPositionMode::Throttled)
    }

    fn finish_update_pos(&self) {
        match self.borrow_mut() {
            Ok(mut text_service) => {
                text_service.update_pos_state.finish_update(Instant::now());
            }
            Err(error) => {
                tracing::warn!("Failed to reset update_pos guard: {error:?}");
            }
        }
    }

    fn is_current_update_pos_request(
        &self,
        generation: u64,
        context: &ITfContext,
        tip_composition: &ITfComposition,
    ) -> bool {
        let Ok(text_service) = self.borrow() else {
            return false;
        };
        if text_service.update_pos_generation != generation {
            return false;
        }
        let Some(current_context) = text_service.context.as_ref() else {
            return false;
        };
        if !has_same_com_identity(current_context, context) {
            return false;
        }
        text_service
            .borrow_composition()
            .ok()
            .and_then(|composition| composition.tip_composition.clone())
            .is_some_and(|current| has_same_com_identity(&current, tip_composition))
    }

    pub(crate) fn request_update_pos_async(
        &self,
        layout_context: Option<&ITfContext>,
    ) -> Result<()> {
        let (tid, context, tip_composition, this, generation) = {
            let mut text_service = self.borrow_mut()?;
            let tip_composition = text_service.borrow_composition()?.tip_composition.clone();
            let Some(tip_composition) = tip_composition else {
                return Ok(());
            };
            let tid = text_service.tid;
            let context = text_service.context::<ITfContext>()?;
            let this = text_service.this::<ITfTextInputProcessor>()?;
            if layout_context
                .is_some_and(|layout_context| !has_same_com_identity(&context, layout_context))
            {
                tracing::debug!("Skip layout update for a stale TSF context");
                return Ok(());
            }

            let now = Instant::now();
            if text_service
                .candidate_window_position_state
                .should_throttle(now)
            {
                tracing::debug!("Skip throttled asynchronous candidate position update");
                return Ok(());
            }
            if !text_service.update_pos_state.try_begin_update(now) {
                tracing::debug!("Skip re-entrant asynchronous candidate position update");
                return Ok(());
            }
            text_service
                .candidate_window_position_state
                .mark_attempt(now);
            text_service.update_pos_generation = text_service.update_pos_generation.wrapping_add(1);
            let generation = text_service.update_pos_generation;

            (tid, context, tip_composition, this, generation)
        };

        let callback = Rc::new({
            let context = context.clone();
            let tip_composition = tip_composition.clone();
            let this = this.clone();

            move |cookie| {
                let factory = unsafe { this.as_impl() };
                if !factory.is_current_update_pos_request(generation, &context, &tip_composition) {
                    tracing::debug!("Skip stale asynchronous candidate position callback");
                    return Ok(());
                }

                let result: Result<()> = (|| unsafe {
                    let view = context.GetActiveView()?;
                    let range = tip_composition.GetRange()?;

                    let mut rect = RECT::default();
                    let mut clipped = false.into();
                    view.GetTextExt(cookie, &range, &mut rect, &mut clipped)?;

                    if let Some(mut ipc_service) = IMEState::ipc_service()? {
                        let position = WindowPosition {
                            top: rect.top,
                            left: rect.left,
                            bottom: rect.bottom,
                            right: rect.right,
                        };
                        ipc_service.update_candidate_window(
                            None,
                            Some(position),
                            None,
                            None,
                            None,
                        )?;
                        IMEState::set_ipc_service(ipc_service)?;
                    }
                    Ok(())
                })();
                result
            }
        });

        let completion = Rc::new({
            let this = this.clone();
            move || {
                let factory = unsafe { this.as_impl() };
                factory.finish_update_pos();
            }
        });

        request_async_read_edit_session(tid, context, callback, completion)
    }

    #[tracing::instrument]
    pub fn update_pos(&self) -> Result<()> {
        if let Some(position) = self.candidate_window_position_for_update()? {
            if let Some(mut ipc_service) = IMEState::ipc_service()? {
                ipc_service.update_candidate_window(None, Some(position), None, None, None)?;
                IMEState::set_ipc_service(ipc_service)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        complete_async_edit_session_request, complete_sync_edit_session,
        is_non_destructive_edit_session_error, AsyncEditSession, EditSessionFailure,
    };
    use std::{cell::Cell, rc::Rc};
    use windows::{
        core::HRESULT,
        Win32::UI::TextServices::{TF_E_DISCONNECTED, TF_E_LOCKED, TF_S_ASYNC},
    };

    #[test]
    fn sync_edit_session_returns_callback_value_after_both_results_succeed() {
        let value = complete_sync_edit_session(Ok(HRESULT(0)), Some("completed".to_string()))
            .expect("completed callback result");

        assert_eq!(value, "completed");
    }

    #[test]
    fn sync_edit_session_keeps_lock_rejection_distinct_from_request_failure() {
        let error = complete_sync_edit_session::<()>(Ok(TF_E_LOCKED), None)
            .expect_err("lock rejection must fail the session");

        assert_eq!(
            error.downcast_ref::<EditSessionFailure>(),
            Some(&EditSessionFailure::Session(TF_E_LOCKED))
        );
        assert!(is_non_destructive_edit_session_error(&error));
    }

    #[test]
    fn sync_edit_session_rejects_unexpected_async_completion() {
        let error = complete_sync_edit_session::<()>(Ok(TF_S_ASYNC), None)
            .expect_err("synchronous contract must not read a deferred result");

        assert_eq!(
            error.downcast_ref::<EditSessionFailure>(),
            Some(&EditSessionFailure::UnexpectedAsync)
        );
    }

    #[test]
    fn sync_edit_session_reports_context_destruction_as_request_failure() {
        let error = complete_sync_edit_session::<()>(Err(TF_E_DISCONNECTED), None)
            .expect_err("destroyed context must fail the outer request");

        assert_eq!(
            error.downcast_ref::<EditSessionFailure>(),
            Some(&EditSessionFailure::Request(TF_E_DISCONNECTED))
        );
        assert!(is_non_destructive_edit_session_error(&error));
    }

    #[test]
    fn sync_edit_session_requires_callback_completion() {
        let error = complete_sync_edit_session::<()>(Ok(HRESULT(0)), None)
            .expect_err("successful session without callback completion is invalid");

        assert_eq!(
            error.downcast_ref::<EditSessionFailure>(),
            Some(&EditSessionFailure::CallbackNotCompleted)
        );
    }

    #[test]
    fn async_edit_session_runs_work_only_when_tsf_invokes_the_callback() {
        let callback_invoked = Rc::new(Cell::new(false));
        let completion_count = Rc::new(Cell::new(0));
        let session: windows::Win32::UI::TextServices::ITfEditSession = AsyncEditSession {
            callback: Rc::new({
                let callback_invoked = callback_invoked.clone();
                move |_| {
                    callback_invoked.set(true);
                    Ok(())
                }
            }),
            completion: Rc::new({
                let completion_count = completion_count.clone();
                move || completion_count.set(completion_count.get() + 1)
            }),
            completed: Cell::new(false),
        }
        .into();

        assert!(!callback_invoked.get());
        unsafe {
            session
                .DoEditSession(0)
                .expect("async callback should complete");
        }
        assert!(callback_invoked.get());
        assert_eq!(completion_count.get(), 1);
        drop(session);
        assert_eq!(completion_count.get(), 1);
    }

    #[test]
    fn async_edit_session_releases_completion_guard_when_callback_never_runs() {
        let completion_count = Rc::new(Cell::new(0));
        let session: windows::Win32::UI::TextServices::ITfEditSession = AsyncEditSession {
            callback: Rc::new(|_| panic!("callback must not run")),
            completion: Rc::new({
                let completion_count = completion_count.clone();
                move || completion_count.set(completion_count.get() + 1)
            }),
            completed: Cell::new(false),
        }
        .into();

        drop(session);
        assert_eq!(completion_count.get(), 1);
    }

    #[test]
    fn async_edit_session_accepts_deferred_completion_without_reading_a_result() {
        complete_async_edit_session_request(Ok(TF_S_ASYNC))
            .expect("deferred async session is an accepted request");
    }

    #[test]
    fn async_edit_session_reports_request_and_lock_failures_separately() {
        let request_error = complete_async_edit_session_request(Err(TF_E_DISCONNECTED))
            .expect_err("destroyed context must reject the outer request");
        assert_eq!(
            request_error.downcast_ref::<EditSessionFailure>(),
            Some(&EditSessionFailure::Request(TF_E_DISCONNECTED))
        );

        let session_error = complete_async_edit_session_request(Ok(TF_E_LOCKED))
            .expect_err("lock rejection must reject the async session");
        assert_eq!(
            session_error.downcast_ref::<EditSessionFailure>(),
            Some(&EditSessionFailure::Session(TF_E_LOCKED))
        );
    }
}
