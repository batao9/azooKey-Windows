//! Explicit cloud ranking. COM objects and completion callbacks stay on the TSF thread.
use super::*;
use std::{cell::RefCell, sync::mpsc, time::Instant};
use windows::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{KillTimer, SetTimer},
};

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

    pub(super) fn jev_composition_eligible(composition: &Composition) -> bool {
        !composition.temporary_latin
            && composition.reconversion_original.is_none()
            && composition.fixed_prefix.is_empty()
            && composition.clause_snapshots.is_empty()
            && composition.future_clause_snapshots.is_empty()
            && !composition.raw_input.is_empty()
            && !composition.current_clause_is_split_derived
    }

    pub(super) fn begin_jev_conversion(&self) -> Result<()> {
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
                || !Self::jev_composition_eligible(&state)
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
        let context_allowed = self.current_context_allows_remote_scoring(&composition);
        if let Some(request_id) = current_input_trace_request_id() {
            Self::log_client_performance(request_id, "jev_start", "eligibility", Duration::ZERO,
                format!("enabled={enabled};keyboard_disabled={keyboard_disabled};context_allowed={context_allowed}"));
        }
        if !context_allowed {
            return Ok(());
        }
        let mapping = choice_indices(&candidates, raw_input.chars().count());
        if let Some(request_id) = current_input_trace_request_id() {
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
    fn current_context_allows_remote_scoring(&self, composition: &ITfComposition) -> bool {
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
                        let range = unsafe { composition.GetRange()? };
                        stage.set("property");
                        let property = unsafe { context.GetAppProperty(&GUID_PROP_INPUTSCOPE)? };
                        stage.set("value");
                        let value = unsafe { property.GetValue(cookie, &range)? };
                        // No input-scope property is normal for ordinary text controls.
                        if value.is_empty() {
                            return Ok(true);
                        }
                        stage.set("scope");
                        let input_scope = IUnknown::try_from(&value)?.cast::<ITfInputScope>()?;
                        let mut scopes_ptr = std::ptr::null_mut();
                        let mut count = 0;
                        unsafe {
                            input_scope.GetInputScopes(&mut scopes_ptr, &mut count)?;
                            let allowed = if count == 0 {
                                true
                            } else if scopes_ptr.is_null() {
                                false
                            } else {
                                !std::slice::from_raw_parts(scopes_ptr, count as usize)
                                    .iter()
                                    .copied()
                                    .any(Self::is_sensitive_input_scope)
                            };
                            CoTaskMemFree(Some(scopes_ptr.cast()));
                            Ok(allowed)
                        }
                    }
                }),
            )
        };
        let result = query();
        if let Some(request_id) = current_input_trace_request_id() {
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
        assert!(TextServiceFactory::jev_composition_eligible(&composition));
        composition.reconversion_original = Some("記者".into());
        assert!(!TextServiceFactory::jev_composition_eligible(&composition));
        composition.reconversion_original = None;
        composition.fixed_prefix = "新聞".into();
        assert!(!TextServiceFactory::jev_composition_eligible(&composition));
        composition.fixed_prefix.clear();
        composition.temporary_latin = true;
        assert!(!TextServiceFactory::jev_composition_eligible(&composition));
    }
}
