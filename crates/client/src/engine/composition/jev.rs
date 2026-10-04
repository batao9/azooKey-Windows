//! Explicit cloud ranking. COM objects and completion callbacks stay on the TSF thread.
use super::*;
use std::{cell::RefCell, mem::ManuallyDrop, sync::mpsc, time::Instant};
use windows::Win32::{
    Foundation::HWND,
    UI::{
        TextServices::{IEnumTfPropertyValue, TF_ANCHOR_START, TF_PROPERTYVAL},
        WindowsAndMessaging::{KillTimer, SetTimer},
    },
};

fn input_scope_allows_remote_scoring(value: &windows::core::VARIANT) -> Result<bool> {
    if value.is_empty() {
        return Ok(true);
    }
    let input_scope = IUnknown::try_from(value)?.cast::<ITfInputScope>()?;
    let mut scopes_ptr = std::ptr::null_mut();
    let mut count = 0;
    unsafe {
        let result = input_scope.GetInputScopes(&mut scopes_ptr, &mut count);
        let allowed = result.map(|()| {
            count == 0
                || (!scopes_ptr.is_null()
                    && !std::slice::from_raw_parts(scopes_ptr, count as usize)
                        .iter()
                        .copied()
                        .any(TextServiceFactory::is_sensitive_input_scope))
        });
        CoTaskMemFree(Some(scopes_ptr.cast()));
        Ok(allowed?)
    }
}

fn tracked_input_scope_allows_remote_scoring(value: &windows::core::VARIANT) -> Result<bool> {
    let values = IUnknown::try_from(value)?.cast::<IEnumTfPropertyValue>()?;
    let mut items = [TF_PROPERTYVAL::default()];
    let mut fetched = 0;
    let result = unsafe { values.Next(&mut items, &mut fetched) };
    // TF_PROPERTYVAL does not release its ManuallyDrop VARIANT.
    let value = unsafe { ManuallyDrop::take(&mut items[0].varValue) };
    result?;
    anyhow::ensure!(
        fetched == 1 && items[0].guidId == GUID_PROP_INPUTSCOPE,
        "Missing tracked input-scope property"
    );
    input_scope_allows_remote_scoring(&value)
}

struct Pending {
    timer: usize,
    started: Instant,
    receiver: mpsc::Receiver<Option<usize>>,
    task: tokio::task::JoinHandle<()>,
    owner: ITfTextInputProcessor,
    context: ITfContext,
    composition: ITfComposition,
    raw_input: String,
    candidates: Candidates,
    mapping: Vec<usize>,
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.task.abort();
        unsafe {
            let _ = KillTimer(None, self.timer);
        }
    }
}

thread_local! {
    static PENDING: RefCell<Option<Pending>> = const { RefCell::new(None) };
}

fn choice_indices(candidates: &Candidates, full_count: usize) -> Vec<usize> {
    let mut seen = HashSet::new();
    candidates
        .texts
        .iter()
        .enumerate()
        .filter_map(|(i, text)| {
            (candidates.corresponding_count.get(i).copied() == i32::try_from(full_count).ok()
                && candidates.sub_texts.get(i).is_some_and(String::is_empty)
                && candidates.candidate_ids.get(i).is_some()
                && !text.is_empty()
                && seen.insert(text.clone()))
            .then_some(i)
        })
        .take(16)
        .collect()
}

fn promote(candidates: &mut Candidates, index: usize) -> bool {
    if index >= candidates.texts.len()
        || index >= candidates.sub_texts.len()
        || index >= candidates.corresponding_count.len()
        || index >= candidates.candidate_ids.len()
    {
        return false;
    }
    candidates.texts[..=index].rotate_right(1);
    candidates.sub_texts[..=index].rotate_right(1);
    candidates.corresponding_count[..=index].rotate_right(1);
    candidates.candidate_ids[..=index].rotate_right(1);
    true
}

unsafe extern "system" fn poll(_window: HWND, _message: u32, id: usize, _time: u32) {
    // Release the TLS borrow before invoking TSF: edit sessions can reenter us.
    let ready = PENDING.with(|pending| {
        let mut pending = pending.borrow_mut();
        let current = pending.as_ref()?;
        if current.timer != id {
            return None;
        }
        match current.receiver.try_recv() {
            Ok(selected) => Some((pending.take()?, selected)),
            Err(mpsc::TryRecvError::Disconnected) => Some((pending.take()?, None)),
            Err(mpsc::TryRecvError::Empty)
                if current.started.elapsed() >= Duration::from_secs(2) =>
            {
                Some((pending.take()?, None))
            }
            Err(mpsc::TryRecvError::Empty) => None,
        }
    });
    let Some((pending, selected)) = ready else {
        return;
    };
    unsafe {
        let _ = KillTimer(None, pending.timer);
    }
    let Some(index) = selected.and_then(|index| pending.mapping.get(index).copied()) else {
        return;
    };
    let factory = unsafe { pending.owner.as_impl() };
    if factory.apply_jev_result(&pending, index).is_err() {
        tracing::debug!("Jev completion unavailable; retain KKC candidates");
    }
}

impl TextServiceFactory {
    pub(crate) fn cancel_jev_conversion(&self) {
        // Drop outside the RefCell borrow because releasing the COM owner can reenter TSF.
        let pending = PENDING.with(|pending| pending.borrow_mut().take());
        drop(pending);
    }

    pub(super) fn jev_composition_eligible(
        composition: &Composition,
        disabled_context_observed: bool,
    ) -> bool {
        !disabled_context_observed
            && !composition.temporary_latin
            && composition.reconversion_original.is_none()
            && composition.fixed_prefix.is_empty()
            && composition.clause_snapshots.is_empty()
            && composition.future_clause_snapshots.is_empty()
            && !composition.raw_input.is_empty()
            && !composition.current_clause_is_split_derived
    }

    pub(super) fn begin_jev_conversion(&self) -> Result<()> {
        let trace_request_id = current_input_trace_request_id();
        let enabled = IMEState::app_config_snapshot()?
            .app_config()
            .general
            .jev_conversion;
        let keyboard_disabled = IMEState::keyboard_disabled().unwrap_or(true);
        if !enabled || keyboard_disabled {
            return Ok(());
        }
        let (owner, context, composition, raw_input, candidates, preview) = {
            let service = self.borrow()?;
            let state = service.borrow_composition()?;
            if state.state != CompositionState::Previewing
                || !Self::jev_composition_eligible(&state, service.disabled_context_observed)
            {
                return Ok(());
            }
            (
                service.this::<ITfTextInputProcessor>()?,
                service.context::<ITfContext>()?,
                state.tip_composition.clone().context("No composition")?,
                state.raw_input.clone(),
                state.candidates.clone(),
                state.preview.clone(),
            )
        };
        let context_allowed =
            self.current_context_allows_remote_scoring(&composition, trace_request_id);
        if let Some(request_id) = trace_request_id {
            Self::log_client_performance(request_id, "jev_start", "eligibility", Duration::ZERO,
                format!("enabled={enabled};keyboard_disabled={keyboard_disabled};context_allowed={context_allowed}"));
        }
        if !context_allowed {
            return Ok(());
        }
        let mapping = choice_indices(&candidates, raw_input.chars().count());
        if let Some(request_id) = trace_request_id {
            Self::log_client_performance(
                request_id,
                "jev_start",
                "choices",
                Duration::ZERO,
                format!(
                    "raw_input_count={};candidate_count={};choice_count={}",
                    raw_input.chars().count(),
                    candidates.texts.len(),
                    mapping.len()
                ),
            );
        }
        if mapping.len() < 2 {
            return Ok(());
        }
        // Read surrounding text only after the fail-closed input-scope check.
        // A failed fresh read must never send context cached from another document.
        let Some(external_context) = self.update_context(&preview)? else {
            return Ok(());
        };
        let ipc = IMEState::ipc_service()?.context("No IPC service")?;
        let (sender, receiver) = mpsc::channel();
        let timer = unsafe { SetTimer(None, 0, 25, Some(poll)) };
        anyhow::ensure!(timer != 0, "Cannot schedule Jev completion");
        let task = ipc.rerank_candidates_background(
            shared::proto::RerankCandidatesRequest {
                reading: candidates.hiragana.clone(),
                context: external_context,
                candidates: mapping
                    .iter()
                    .map(|&index| candidates.texts[index].clone())
                    .collect(),
                allow_remote: true,
                request_id: timer as u64,
            },
            sender,
        );
        PENDING.with(|pending| {
            *pending.borrow_mut() = Some(Pending {
                timer,
                started: Instant::now(),
                receiver,
                task,
                owner,
                context,
                composition,
                raw_input,
                candidates,
                mapping,
            })
        });
        Ok(())
    }

    fn apply_jev_result(&self, pending: &Pending, index: usize) -> Result<()> {
        if !IMEState::app_config_snapshot()?
            .app_config()
            .general
            .jev_conversion
            || IMEState::keyboard_disabled().unwrap_or(true)
        {
            return Ok(());
        }
        {
            let service = self.borrow()?;
            if !has_same_com_identity(&service.context::<ITfContext>()?, &pending.context) {
                return Ok(());
            }
            let mut composition = service.borrow_mut_composition()?;
            if composition.state != CompositionState::Previewing
                || !Self::jev_composition_eligible(&composition, service.disabled_context_observed)
                || composition.raw_input != pending.raw_input
                || composition.candidates != pending.candidates
                || !composition
                    .tip_composition
                    .as_ref()
                    .is_some_and(|tip| has_same_com_identity(tip, &pending.composition))
            {
                return Ok(());
            }
            if !promote(&mut composition.candidates, index) {
                return Ok(());
            }
        }
        self.handle_action(
            &[ClientAction::SetSelection(SetSelectionType::Number(0))],
            CompositionState::Previewing,
        )
    }
    fn current_context_allows_remote_scoring(
        &self,
        composition: &ITfComposition,
        trace_request_id: Option<u64>,
    ) -> bool {
        let stage = Rc::new(std::cell::Cell::new("context"));
        let query = || -> Result<bool> {
            let (tid, context) = {
                let service = self.borrow()?;
                (service.tid, service.context::<ITfContext>()?)
            };
            if keyboard_disabled_from_context(&context) {
                return Ok(false);
            }
            stage.set("edit_session");
            let composition = composition.clone();
            read_edit_session::<bool>(
                tid,
                context.clone(),
                Rc::new({
                    let stage = stage.clone();
                    move |cookie| {
                        stage.set("composition_range");
                        let range = unsafe { composition.GetRange()?.Clone()? };
                        unsafe { range.Collapse(cookie, TF_ANCHOR_START)? };
                        stage.set("property");
                        let property =
                            unsafe { context.TrackProperties(&[], &[&GUID_PROP_INPUTSCOPE])? };
                        // Query one insertion point rather than relying on the host's
                        // optional FindNextAttrTransition / EnumRanges implementation.
                        stage.set("value");
                        let tracker_value = unsafe { property.GetValue(cookie, &range) };
                        if let Some(request_id) = trace_request_id {
                            Self::log_client_performance(
                                request_id,
                                "jev_start",
                                "scope_api",
                                Duration::ZERO,
                                format!(
                                    "get_value_error={:?};variant_type={:?}",
                                    tracker_value.as_ref().err().map(|error| error.code()),
                                    tracker_value.as_ref().ok().map(|value| unsafe {
                                        value.as_raw().Anonymous.Anonymous.vt
                                    })
                                ),
                            );
                        }
                        let tracker_value = tracker_value?;
                        stage.set("scope");
                        tracked_input_scope_allows_remote_scoring(&tracker_value)
                    }
                }),
            )
        };
        let result = query();
        if let Some(request_id) = trace_request_id {
            Self::log_client_performance(
                request_id,
                "jev_start",
                "input_scope",
                Duration::ZERO,
                format!(
                    "stage={};result={:?}",
                    stage.get(),
                    result.as_ref().map_err(|error| error.to_string())
                ),
            );
        }
        result.unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::{
        core::{implement, BSTR, GUID, VARIANT},
        Win32::{
            Foundation::{E_FAIL, E_NOTIMPL},
            System::Com::CoTaskMemAlloc,
            UI::TextServices::{IEnumTfPropertyValue_Impl, ITfInputScope_Impl, IS_DEFAULT},
        },
    };

    #[implement(ITfInputScope)]
    struct TestInputScope {
        scopes: Vec<InputScope>,
        fail: bool,
    }

    impl ITfInputScope_Impl for TestInputScope_Impl {
        fn GetInputScopes(
            &self,
            output: *mut *mut InputScope,
            count: *mut u32,
        ) -> windows::core::Result<()> {
            if self.fail {
                return Err(E_FAIL.into());
            }
            unsafe {
                *count = self.scopes.len() as u32;
                *output = CoTaskMemAlloc(std::mem::size_of_val(self.scopes.as_slice())).cast();
                assert!(!output.read().is_null());
                std::ptr::copy_nonoverlapping(self.scopes.as_ptr(), *output, self.scopes.len());
            }
            Ok(())
        }
        fn GetPhrase(&self, _: *mut *mut BSTR, _: *mut u32) -> windows::core::Result<()> {
            Err(E_NOTIMPL.into())
        }
        fn GetRegularExpression(&self) -> windows::core::Result<BSTR> {
            Err(E_NOTIMPL.into())
        }
        fn GetSRGS(&self) -> windows::core::Result<BSTR> {
            Err(E_NOTIMPL.into())
        }
        fn GetXML(&self) -> windows::core::Result<BSTR> {
            Err(E_NOTIMPL.into())
        }
    }

    #[implement(IEnumTfPropertyValue)]
    struct TestTrackedScope {
        value: VARIANT,
        guid: GUID,
        fetched: u32,
        fail: bool,
    }

    impl IEnumTfPropertyValue_Impl for TestTrackedScope_Impl {
        fn Clone(&self) -> windows::core::Result<IEnumTfPropertyValue> {
            Err(E_NOTIMPL.into())
        }
        fn Next(
            &self,
            count: u32,
            output: *mut TF_PROPERTYVAL,
            fetched: *mut u32,
        ) -> windows::core::Result<()> {
            assert_eq!(count, 1);
            unsafe {
                *fetched = self.fetched;
                (*output).guidId = self.guid;
                (*output).varValue = ManuallyDrop::new(self.value.clone());
            }
            if self.fail {
                Err(E_FAIL.into())
            } else {
                Ok(())
            }
        }
        fn Reset(&self) -> windows::core::Result<()> {
            Err(E_NOTIMPL.into())
        }
        fn Skip(&self, _: u32) -> windows::core::Result<()> {
            Err(E_NOTIMPL.into())
        }
    }

    #[test]
    fn jev_tracked_scope_requires_matching_readable_property_and_blocks_sensitive_values() {
        assert!(!tracked_input_scope_allows_remote_scoring(&VARIANT::default()).unwrap_or(false));
        for (guid, fetched, fail, allowed) in [
            (GUID_PROP_INPUTSCOPE, 1, false, true),
            (GUID::zeroed(), 1, false, false),
            (GUID_PROP_INPUTSCOPE, 0, false, false),
            (GUID_PROP_INPUTSCOPE, 1, true, false),
        ] {
            let tracker: IEnumTfPropertyValue = TestTrackedScope {
                value: VARIANT::default(),
                guid,
                fetched,
                fail,
            }
            .into();
            let value = VARIANT::from(tracker.cast::<IUnknown>().unwrap());
            assert_eq!(
                tracked_input_scope_allows_remote_scoring(&value).unwrap_or(false),
                allowed
            );
        }
        for scope in [IS_PASSWORD, IS_NUMERIC_PASSWORD, IS_PRIVATE] {
            let input_scope: ITfInputScope = TestInputScope {
                scopes: vec![IS_DEFAULT, scope],
                fail: false,
            }
            .into();
            let tracker: IEnumTfPropertyValue = TestTrackedScope {
                value: VARIANT::from(input_scope.cast::<IUnknown>().unwrap()),
                guid: GUID_PROP_INPUTSCOPE,
                fetched: 1,
                fail: false,
            }
            .into();
            let value = VARIANT::from(tracker.cast::<IUnknown>().unwrap());
            assert!(!tracked_input_scope_allows_remote_scoring(&value).unwrap());
        }
    }

    #[test]
    fn jev_scope_gate_accepts_ordinary_text_and_denies_sensitive_or_unknown_scopes() {
        assert!(input_scope_allows_remote_scoring(&VARIANT::default()).unwrap());
        assert!(!input_scope_allows_remote_scoring(&VARIANT::from(42)).unwrap_or(false));
        for (scopes, fail, allowed) in [
            (vec![IS_DEFAULT], false, true),
            (vec![IS_DEFAULT, IS_PASSWORD], false, false),
            (vec![IS_NUMERIC_PASSWORD], false, false),
            (vec![IS_PRIVATE], false, false),
            (vec![IS_DEFAULT], true, false),
        ] {
            let scope: ITfInputScope = TestInputScope { scopes, fail }.into();
            let value = VARIANT::from(scope.cast::<IUnknown>().unwrap());
            assert_eq!(
                input_scope_allows_remote_scoring(&value).unwrap_or(false),
                allowed
            );
        }
    }

    fn candidates() -> Candidates {
        Candidates {
            texts: vec!["記者", "汽車", "記者", "木"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            sub_texts: vec!["", "", "", "しゃ"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            hiragana: "きしゃ".into(),
            corresponding_count: vec![5, 5, 5, 2],
            candidate_ids: vec![11, 12, 13, 14],
        }
    }

    #[test]
    fn jev_full_choices_deduplicate_and_use_raw_input_count() {
        assert_eq!(choice_indices(&candidates(), 5), vec![0, 1]);
        assert!(choice_indices(&candidates(), 3).is_empty());
        let mut broken = candidates();
        broken.candidate_ids.clear();
        assert!(choice_indices(&broken, 5).is_empty());
    }

    #[test]
    fn jev_promotion_preserves_candidate_identity_and_baseline() {
        let mut values = candidates();
        assert!(promote(&mut values, 1));
        assert_eq!(values.texts, ["汽車", "記者", "記者", "木"]);
        assert_eq!(values.candidate_ids, [12, 11, 13, 14]);
        assert_eq!(values.corresponding_count, [5, 5, 5, 2]);
        assert_eq!(values.sub_texts, ["", "", "", "しゃ"]);
        let before = values.clone();
        assert!(!promote(&mut values, 9));
        assert_eq!(values, before);
    }

    #[test]
    fn jev_only_initial_explicit_conversion_requests_cloud() {
        let mut config = AppConfig::default();
        config.general.jev_conversion = true;
        let mut composition = Composition {
            state: CompositionState::Composing,
            raw_input: "kisha".into(),
            candidates: candidates(),
            ..Default::default()
        };
        for action in [UserAction::Space, UserAction::Tab, UserAction::Reconvert] {
            let (_, actions) = TextServiceFactory::plan_actions_for_user_action(
                &composition,
                &action,
                &InputMode::Kana,
                false,
                &config,
                false,
            )
            .unwrap();
            assert!(actions.contains(&ClientAction::StartJevConversion));
            assert_eq!(
                actions[0],
                ClientAction::SetSelection(SetSelectionType::Number(0))
            );
        }
        let (_, typing_actions) = TextServiceFactory::plan_actions_for_user_action(
            &composition,
            &UserAction::Input('a'),
            &InputMode::Kana,
            false,
            &config,
            false,
        )
        .unwrap();
        assert!(!typing_actions.contains(&ClientAction::StartJevConversion));

        composition.state = CompositionState::Previewing;
        let (_, actions) = TextServiceFactory::plan_actions_for_user_action(
            &composition,
            &UserAction::Space,
            &InputMode::Kana,
            false,
            &config,
            false,
        )
        .unwrap();
        assert!(!actions.contains(&ClientAction::StartJevConversion));
        assert!(actions.contains(&ClientAction::SetSelection(SetSelectionType::Down)));
        composition.state = CompositionState::Composing;
        config.general.jev_conversion = false;
        let (_, actions) = TextServiceFactory::plan_actions_for_user_action(
            &composition,
            &UserAction::Space,
            &InputMode::Kana,
            false,
            &config,
            false,
        )
        .unwrap();
        assert!(!actions.contains(&ClientAction::StartJevConversion));
        assert!(actions.contains(&ClientAction::SetSelection(SetSelectionType::Down)));
    }

    #[test]
    fn jev_reconversion_and_split_compositions_are_ineligible() {
        let mut composition = Composition {
            raw_input: "kisha".into(),
            ..Default::default()
        };
        assert!(TextServiceFactory::jev_composition_eligible(
            &composition,
            false
        ));
        composition.reconversion_original = Some("記者".into());
        assert!(!TextServiceFactory::jev_composition_eligible(
            &composition,
            false
        ));
        composition.reconversion_original = None;
        composition.fixed_prefix = "新聞".into();
        assert!(!TextServiceFactory::jev_composition_eligible(
            &composition,
            false
        ));
        composition.fixed_prefix.clear();
        composition.temporary_latin = true;
        assert!(!TextServiceFactory::jev_composition_eligible(
            &composition,
            false
        ));
    }

    #[test]
    fn jev_completion_rejects_disabled_context_observed_without_handle_callback() {
        let composition = Composition {
            state: CompositionState::Previewing,
            raw_input: "kisha".into(),
            candidates: candidates(),
            ..Default::default()
        };
        assert!(TextServiceFactory::jev_composition_eligible(
            &composition,
            false
        ));
        // A rejected OnTestKey event only records this flag: the cached keyboard
        // state and composition snapshot can remain unchanged until Handle.
        assert!(!TextServiceFactory::jev_composition_eligible(
            &composition,
            true
        ));
    }
}
