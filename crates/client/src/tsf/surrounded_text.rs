// reference to the original code:
// https://github.com/google/mozc/blob/master/src/win32/tip/tip_surrounding_text.cc

use std::{mem::ManuallyDrop, rc::Rc, time::Instant};

use anyhow::Result;
use windows::{
    core::{IUnknown, Interface},
    Win32::UI::TextServices::{
        ITfCompartmentMgr, ITfContext, ITfDocumentMgr, ITfRange,
        GUID_COMPARTMENT_TRANSITORYEXTENSION_PARENT, TF_ANCHOR_END, TF_ANCHOR_START,
        TF_DEFAULT_SELECTION, TF_HALTCOND, TF_HF_OBJECT, TF_SELECTION, TS_SS_TRANSITORY,
    },
};

use crate::{
    engine::{ipc_service::current_input_trace_request_id, state::IMEState},
    extension::utf16_code_unit_len,
};

use super::{edit_session::read_edit_session, factory::TextServiceFactory};

fn preview_end_shift(preview: &str) -> anyhow::Result<i32> {
    Ok(-utf16_code_unit_len(preview)?)
}

#[derive(Default, Debug, PartialEq, Eq)]
pub(crate) struct SurroundingText {
    pub left: String,
    pub right: String,
    pub permitted: bool,
}

#[derive(Debug)]
pub(crate) struct SupersededContext;
impl std::fmt::Display for SupersededContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("surrounding-text request superseded")
    }
}
impl std::error::Error for SupersededContext {}

// Both ranges are bounded before any document read. `excluded` is the whole
// composition/selection, not just the currently selected clause or caret.
pub(crate) fn read_surrounding_ranges(
    cookie: u32,
    excluded: &ITfRange,
    read_right: bool,
    allows: impl Fn(&ITfRange) -> Result<bool>,
) -> Result<SurroundingText> {
    unsafe {
        let halt = TF_HALTCOND {
            pHaltRange: ManuallyDrop::new(None),
            aHaltPos: TF_ANCHOR_START,
            dwFlags: TF_HF_OBJECT,
        };
        let left = excluded.Clone()?;
        left.Collapse(cookie, TF_ANCHOR_START)?;
        let mut shifted = 0;
        left.ShiftStart(cookie, -64, &mut shifted, &halt)?;
        let right = excluded.Clone()?;
        right.Collapse(cookie, TF_ANCHOR_END)?;
        if read_right {
            right.ShiftEnd(cookie, 64, &mut shifted, &halt)?;
        }
        // Check both sides before reading either; failure clears the pair.
        for range in [&left, &right] {
            if !range.IsEmpty(cookie)?.as_bool() && !allows(range)? {
                return Ok(SurroundingText::default());
            }
        }
        fn read(cookie: u32, range: &ITfRange) -> Result<String> {
            let mut units = [0u16; 64];
            let mut length = 0;
            unsafe {
                range.GetText(cookie, 0, &mut units, &mut length)?;
            }
            // A bounded TSF range may cut a surrogate pair. Drop incomplete
            // edge units rather than injecting replacement characters.
            let mut units = &units[..length as usize];
            if units
                .first()
                .is_some_and(|unit| (0xDC00..=0xDFFF).contains(unit))
            {
                units = &units[1..];
            }
            if units
                .last()
                .is_some_and(|unit| (0xD800..=0xDBFF).contains(unit))
            {
                units = &units[..units.len() - 1];
            }
            Ok(String::from_utf16(units)?)
        }
        Ok(SurroundingText {
            left: read(cookie, &left)?,
            right: if read_right {
                read(cookie, &right)?
            } else {
                String::new()
            },
            permitted: true,
        })
    }
}

impl TextServiceFactory {
    fn log_update_context_performance(
        request_id: u64,
        stage: &str,
        start: Instant,
        details: impl Into<String>,
    ) {
        if let Ok(Some(ipc_service)) = IMEState::ipc_service() {
            ipc_service.log_client_performance(
                request_id,
                "update_context",
                stage,
                start.elapsed(),
                details.into(),
            );
        }
    }

    fn to_parent_document_if_exists(
        &self,
        document_manager: Option<ITfDocumentMgr>,
    ) -> Result<ITfDocumentMgr> {
        let document_manager = match document_manager {
            Some(doc_mgr) => doc_mgr,
            None => return Err(anyhow::anyhow!("Document manager is null")),
        };

        unsafe {
            // Get top context
            let context = match document_manager.GetTop() {
                Ok(ctx) => ctx,
                Err(_) => return Ok(document_manager),
            };

            // Get status
            let status = match context.GetStatus() {
                Ok(s) => s,
                Err(_) => return Ok(document_manager),
            };

            // Check if context is transitory
            if (status.dwStaticFlags & TS_SS_TRANSITORY) != TS_SS_TRANSITORY {
                return Ok(document_manager);
            }

            // Get compartment manager
            let compartment_mgr = match document_manager.cast::<ITfCompartmentMgr>() {
                Ok(mgr) => mgr,
                Err(_) => return Ok(document_manager),
            };

            // Get compartment
            let compartment = match compartment_mgr
                .GetCompartment(&GUID_COMPARTMENT_TRANSITORYEXTENSION_PARENT)
            {
                Ok(comp) => comp,
                Err(_) => return Ok(document_manager),
            };

            // Get value
            let variant = match compartment.GetValue() {
                Ok(var) => var,
                Err(_) => return Ok(document_manager),
            };

            // Use a cloned IUnknown from VARIANT to avoid invalid reference-count handling.
            // If this is not VT_UNKNOWN (or null), treat it as "parent not available".
            let variant_unk = match IUnknown::try_from(&variant) {
                Ok(unk) => unk,
                Err(_) => return Ok(document_manager),
            };

            match variant_unk.cast::<ITfDocumentMgr>() {
                Ok(parent_doc_mgr) => Ok(parent_doc_mgr),
                Err(_) => Ok(document_manager),
            }
        }
    }

    fn to_parent_context_if_exists(&self, context: Option<ITfContext>) -> Result<ITfContext> {
        let context = match context {
            Some(ctx) => ctx,
            None => return Err(anyhow::anyhow!("Context is null")),
        };

        unsafe {
            // Get document manager
            let document_mgr = match context.GetDocumentMgr() {
                Ok(doc_mgr) => doc_mgr,
                Err(_) => return Ok(context),
            };

            // Get parent document
            let parent_doc_mgr = self.to_parent_document_if_exists(Some(document_mgr))?;

            // Get top context from parent document
            let parent_context = match parent_doc_mgr.GetTop() {
                Ok(ctx) => ctx,
                Err(_) => return Ok(context),
            };

            Ok(parent_context)
        }
    }

    pub fn update_context(&self, preview: &str) -> Result<Option<String>> {
        self.update_context_with_scope_check(preview, false, None)
    }

    pub(crate) fn update_context_for_preview_continuation(
        &self,
        current_display: &str,
        committed: &str,
    ) -> Result<Option<String>> {
        self.update_context_with_scope_check(current_display, false, Some(committed))
    }

    pub(crate) fn update_context_for_jev(&self, preview: &str) -> Result<Option<String>> {
        self.update_context_with_scope_check(preview, true, None)
    }

    fn update_context_with_scope_check(
        &self,
        preview: &str,
        for_jev: bool,
        committed: Option<&str>,
    ) -> Result<Option<String>> {
        let trace_request_id = current_input_trace_request_id();
        let total_start = trace_request_id.map(|_| Instant::now());
        let result: Result<Option<String>> = (|| unsafe {
            let (tid, source_context, parent_context, composition, generation) = {
                let text_service = self.borrow()?;
                let context = text_service.context::<ITfContext>()?;
                let parent_context = self.to_parent_context_if_exists(Some(context.clone()))?;
                let composition = text_service.borrow_composition()?.tip_composition.clone();
                (
                    text_service.tid,
                    context,
                    parent_context,
                    composition,
                    text_service.surrounding_text_context_state.generation,
                )
            };

            let edit_session_start = trace_request_id.map(|_| Instant::now());
            let preview_end_shift = preview_end_shift(preview)?;
            let read_result = read_edit_session::<SurroundingText>(
                tid,
                parent_context.clone(),
                Rc::new({
                    move |cookie| {
                        // 2. Get the selection from the parent context.
                        let mut pselection: [TF_SELECTION; 1] = [TF_SELECTION::default()];
                        let mut pfetched = 0;
                        parent_context.GetSelection(
                            cookie,
                            TF_DEFAULT_SELECTION,
                            &mut pselection,
                            &mut pfetched,
                        )?;

                        if pfetched == 0 {
                            anyhow::bail!("Missing context selection");
                        }

                        let range = match pselection[0].range.as_ref() {
                            Some(range) => range.Clone()?,
                            None => anyhow::bail!("Missing context selection range"),
                        };
                        if !Self::context_input_scope_allows_text(&parent_context, cookie, &range)?
                        {
                            return Ok(SurroundingText::default());
                        }
                        let excluded = if let Some(composition) = composition.as_ref() {
                            let composition_range = composition.GetRange()?;
                            if composition_range.GetContext()?.cast::<IUnknown>()?.as_raw()
                                == parent_context.cast::<IUnknown>()?.as_raw()
                            {
                                composition_range
                            } else {
                                // A transitory context's parent has its own caret.
                                let mut shifted = 0;
                                range.ShiftStart(
                                    cookie,
                                    preview_end_shift,
                                    &mut shifted,
                                    std::ptr::null(),
                                )?;
                                anyhow::ensure!(
                                    shifted == preview_end_shift,
                                    "Incomplete parent composition range"
                                );
                                range
                            }
                        } else {
                            range
                        };
                        read_surrounding_ranges(cookie, &excluded, !for_jev, |span| {
                            Self::jev_context_range_allows_remote_scoring(
                                &parent_context,
                                cookie,
                                span,
                            )
                        })
                    }
                }),
            );
            // No deferred read may repopulate a newer focus/composition.
            {
                let current = self.borrow()?;
                if !(current.surrounding_text_context_state.generation == generation
                    && current
                        .context::<ITfContext>()?
                        .cast::<IUnknown>()?
                        .as_raw()
                        == source_context.cast::<IUnknown>()?.as_raw())
                {
                    return Err(SupersededContext.into());
                }
            }
            let read_succeeded = read_result.is_ok();
            let mut surrounding = read_result.unwrap_or_default();
            let mut preceding_text = std::mem::take(&mut surrounding.left);
            if let (Some(request_id), Some(edit_session_start)) =
                (trace_request_id, edit_session_start)
            {
                Self::log_update_context_performance(
                    request_id,
                    "edit_session",
                    edit_session_start,
                    format!(
                        "status=success;preview_len={};preceding_text_present=true",
                        preview.chars().count()
                    ),
                );
            }

            // Jev passes this fresh value explicitly; it must not touch KKC's cache
            // or issue the normal synchronous set_context RPC.
            if for_jev {
                return Ok((read_succeeded && surrounding.permitted).then_some(preceding_text));
            }
            if let Some(committed) = committed.filter(|_| surrounding.permitted) {
                preceding_text.push_str(committed);
            }

            let Some(mut ipc_service) = IMEState::ipc_service()? else {
                return Ok(Some(preceding_text));
            };

            let connection_id = ipc_service.connection_id();
            let should_set_context = {
                let text_service = self.borrow_mut()?;
                text_service.surrounding_text_context_state.should_send(
                    ipc_service.context_cache_key(),
                    &preceding_text,
                    &surrounding.right,
                )
            };
            if !should_set_context {
                if let Some(request_id) = trace_request_id {
                    Self::log_update_context_performance(
                        request_id,
                        "set_context",
                        Instant::now(),
                        format!(
                            "status=skipped;reason=unchanged;preview_len={};context_len={};connection_id={connection_id}",
                            preview.chars().count(),
                            preceding_text.chars().count()
                        ),
                    );
                }
                return Ok(Some(preceding_text));
            }

            ipc_service.set_context(preceding_text.clone(), surrounding.right.clone())?;
            {
                let mut text_service = self.borrow_mut()?;
                if text_service.surrounding_text_context_state.generation != generation {
                    return Err(SupersededContext.into());
                }
                text_service.surrounding_text_context_state.remember(
                    ipc_service.context_cache_key(),
                    &preceding_text,
                    &surrounding.right,
                );
            }
            IMEState::set_ipc_service(ipc_service)?;

            Ok(Some(preceding_text))
        })();

        if let (Some(request_id), Some(total_start)) = (trace_request_id, total_start) {
            let details = match &result {
                Ok(_) => format!("status=success;preview_len={}", preview.chars().count()),
                Err(error) => format!(
                    "status=error;preview_len={};error={error:?}",
                    preview.chars().count()
                ),
            };
            Self::log_update_context_performance(request_id, "total", total_start, details);
        }

        if let Err(error) = &result {
            tracing::warn!("Failed to update surrounded text context: {error:?}");
        }

        // Read errors are already converted to an explicit empty pair. RPC or
        // stale-generation failures must abort this action, not use old context.
        result
    }
}

#[cfg(test)]
mod tests {
    use super::preview_end_shift;

    #[test]
    fn utf16_tsf_boundary_surrounded_text_excludes_preview_by_code_units() {
        assert_eq!(preview_end_shift("😀かな").unwrap(), -4);
        assert_eq!(preview_end_shift("か😀な").unwrap(), -4);
        assert_eq!(preview_end_shift("かな𠮷").unwrap(), -4);
    }
}
