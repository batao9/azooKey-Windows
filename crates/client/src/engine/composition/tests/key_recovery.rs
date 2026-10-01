use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

use super::*;
use crate::{
    engine::{
        client_action::ClientAction, input_mode::InputMode, ipc_service::IPCService,
        state::IMEState,
    },
    tsf::factory::TextServiceFactory,
};
use windows::{
    core::{AsImpl, Interface},
    Win32::{
        UI::Input::KeyboardAndMouse::{GetKeyboardState, SetKeyboardState},
        UI::TextServices::{ITfKeyEventSink, ITfTextInputProcessor},
    },
};

#[path = "key_recovery_context.rs"]
mod test_context;

static TEST_LOCK: Mutex<()> = Mutex::new(());

struct RestoreGlobals {
    ipc_service: Option<IPCService>,
    input_mode: InputMode,
    keyboard_disabled: bool,
    keyboard_state: [u8; 256],
}

impl RestoreGlobals {
    fn new() -> Self {
        let state = IMEState::get().expect("IME state");
        let mut keyboard_state = [0; 256];
        unsafe { GetKeyboardState(&mut keyboard_state).expect("read keyboard state") };
        let restore = Self {
            ipc_service: state.ipc_service.clone(),
            input_mode: state.input_mode.clone(),
            keyboard_disabled: state.keyboard_disabled,
            keyboard_state,
        };
        drop(state);
        IMEState::set_ipc_service(IPCService::recovery_for_test(true)).expect("IPC fixture");
        IMEState::set_input_mode(InputMode::Kana).expect("Kana mode");
        unsafe { SetKeyboardState(&[0; 256]).expect("clear keyboard state") };
        restore
    }
}

impl Drop for RestoreGlobals {
    fn drop(&mut self) {
        if let Ok(mut state) = IMEState::get() {
            state.ipc_service = self.ipc_service.take();
            state.input_mode = self.input_mode.clone();
            state.keyboard_disabled = self.keyboard_disabled;
        }
        unsafe { SetKeyboardState(&self.keyboard_state).expect("restore keyboard state") };
    }
}

fn snapshot(factory: &TextServiceFactory) -> Composition {
    let service = factory.borrow().expect("text service");
    let composition = service.borrow_composition().expect("composition").clone();
    composition
}

fn assert_unchanged(factory: &TextServiceFactory, before: &Composition) {
    let after = snapshot(factory);
    assert_eq!(after.deferred_actions, before.deferred_actions);
    assert_eq!(after.deferred_inputs, before.deferred_inputs);
    assert_eq!(after.deferred_projection, before.deferred_projection);
    assert_eq!(after.raw_input, before.raw_input);
    assert_eq!(after.state, before.state);
}

fn seed_composition(factory: &TextServiceFactory, state: CompositionState, temporary_latin: bool) {
    let service = factory.borrow().expect("text service");
    let mut composition = service.borrow_mut_composition().expect("composition");
    *composition = Composition {
        state,
        temporary_latin,
        deferred_actions: vec![DeferredClientAction {
            action: ClientAction::StartComposition,
            transition: CompositionState::Composing,
        }],
        ..Composition::default()
    };
}

fn queued_actions(factory: &TextServiceFactory) -> Vec<Vec<ClientAction>> {
    snapshot(factory)
        .deferred_inputs
        .iter()
        .filter_map(|event| match event {
            DeferredInputEvent::Actions(actions) => {
                Some(actions.iter().map(|entry| entry.action.clone()).collect())
            }
            DeferredInputEvent::User { .. } => None,
        })
        .collect()
}

fn fake_replay(
    factory: &TextServiceFactory,
    operations: &mut Vec<ClientAction>,
    raw_input: &mut String,
    committed: &mut Vec<String>,
) -> anyhow::Result<()> {
    let mut first_execute = true;
    factory.replay_deferred_user_actions(|actions, transition, _| {
        let composition = snapshot(factory);
        let mut transition = transition;
        if first_execute {
            first_execute = false;
            if let Some(last) = composition.deferred_actions.last() {
                transition = last.transition.clone();
            }
            operations.extend(
                composition
                    .deferred_actions
                    .iter()
                    .map(|entry| entry.action.clone()),
            );
            let service = factory.borrow()?;
            service.borrow_mut_composition()?.deferred_actions.clear();
        } else {
            operations.extend_from_slice(actions);
        }
        for action in actions {
            match action {
                ClientAction::AppendText(text) => raw_input.push_str(text),
                ClientAction::RemoveText => {
                    raw_input.pop();
                }
                ClientAction::EndComposition => {
                    committed.push(raw_input.clone());
                    raw_input.clear();
                }
                _ => {}
            }
        }
        let service = factory.borrow()?;
        let mut composition = service.borrow_mut_composition()?;
        composition.state = transition;
        composition.raw_input.clone_from(raw_input);
        Ok(())
    })
}

#[test]
fn pending_recovery_callbacks_replay_keys_once_and_preserve_shift_chords() {
    let _lock = TEST_LOCK.lock().unwrap();
    let _restore = RestoreGlobals::new();

    unsafe {
        let context = test_context::new(false);
        let processor =
            TextServiceFactory::create::<ITfTextInputProcessor>().expect("text service");
        let factory = processor.as_impl();
        seed_composition(factory, CompositionState::None, false);
        let sink: ITfKeyEventSink = processor.cast().expect("key event sink");

        let start = Instant::now();
        for (index, (key, action)) in [
            (0x41, UserAction::Input('a')),
            (0x42, UserAction::Input('b')),
            (0x08, UserAction::Backspace),
            (0x0D, UserAction::Enter),
        ]
        .into_iter()
        .enumerate()
        {
            let before_test = snapshot(factory);
            let lparam = LPARAM(1 | (1 << 30));
            assert!(sink
                .OnTestKeyDown(Some(&context), WPARAM(key), lparam)
                .expect("test key")
                .as_bool());
            assert_unchanged(factory, &before_test);
            assert!(sink
                .OnKeyDown(Some(&context), WPARAM(key), lparam)
                .expect("handle key")
                .as_bool());
            let after = snapshot(factory);
            assert_eq!(after.deferred_actions.len(), 1);
            assert_eq!(after.deferred_inputs.len(), index + 1);
            let DeferredInputEvent::User { input, .. } = &after.deferred_inputs[index] else {
                panic!("expected user key event")
            };
            assert_eq!(input.action, action);
        }
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "callbacks blocked: {:?}",
            start.elapsed()
        );
        let ipc = IMEState::ipc_service().expect("IPC").expect("fixture");
        assert!(ipc.recovery_pending());
        assert!(!ipc.recovery_restart_ready());

        let (mut operations, mut raw_input, mut committed) =
            (Vec::new(), String::new(), Vec::new());
        fake_replay(factory, &mut operations, &mut raw_input, &mut committed).expect("fake replay");
        assert_eq!(
            operations,
            [
                ClientAction::StartComposition,
                ClientAction::AppendText("a".into()),
                ClientAction::AppendText("b".into()),
                ClientAction::RemoveText,
                ClientAction::CommitLearning {
                    scope: LearningCommitScope::Composition,
                    kind: LearningCommitKind::Normal,
                    was_temporary_latin: false,
                },
                ClientAction::EndComposition,
            ]
        );
        assert_eq!(committed, ["a"]);
        assert!(raw_input.is_empty());
        assert!(!factory.has_deferred_input().expect("drained queue"));
        let final_composition = snapshot(factory);
        assert_eq!(final_composition.state, CompositionState::None);
        assert!(final_composition.deferred_projection.is_none());
        let count = operations.len();
        fake_replay(factory, &mut operations, &mut raw_input, &mut committed)
            .expect("second replay");
        assert_eq!(operations.len(), count);

        // A ready restart must claim even an unowned key so replay can precede it.
        ipc.complete_restart_for_test();
        seed_composition(factory, CompositionState::None, false);
        let before_failed_replay_test = snapshot(factory);
        assert!(sink
            .OnTestKeyDown(Some(&context), WPARAM(0x41), LPARAM(0))
            .expect("test key before failed reconstruction")
            .as_bool());
        assert_unchanged(factory, &before_failed_replay_test);
        assert!(sink
            .OnKeyDown(Some(&context), WPARAM(0x41), LPARAM(0))
            .expect("preserve key after failed reconstruction")
            .as_bool());
        let preserved = snapshot(factory);
        assert_eq!(
            preserved.deferred_actions,
            before_failed_replay_test.deferred_actions
        );
        assert_eq!(preserved.deferred_inputs.len(), 1);
        let DeferredInputEvent::User { input, .. } = &preserved.deferred_inputs[0] else {
            panic!("the callback should preserve one physical key")
        };
        assert_eq!(input.action, UserAction::Input('a'));
        assert!(ipc.recovery_pending());
        assert!(ipc.recovery_restart_ready());

        seed_composition(factory, CompositionState::None, false);
        assert!(factory.deferred_input_ready().expect("ready queue"));
        let before_ready_test = snapshot(factory);
        assert!(sink
            .OnTestKeyDown(Some(&context), WPARAM(0x71), LPARAM(0))
            .expect("ready F2 test")
            .as_bool());
        assert_unchanged(factory, &before_ready_test);
        fake_replay(factory, &mut operations, &mut raw_input, &mut committed)
            .expect("ready fake replay");
        assert!(!factory.has_deferred_input().expect("ready queue drained"));
        assert!(!sink
            .OnTestKeyDown(Some(&context), WPARAM(0x71), LPARAM(0))
            .expect("unowned F2 test")
            .as_bool());
        assert!(!sink
            .OnKeyDown(Some(&context), WPARAM(0x71), LPARAM(0))
            .expect("unowned F2 key")
            .as_bool());

        // Generation zero is healthy, not a stalled restart. Exercise the real
        // Handle replay path with a local action that needs neither a host edit nor RPC.
        IMEState::set_ipc_service(IPCService::recovery_for_test(false))
            .expect("healthy IPC fixture");
        {
            let service = factory.borrow().expect("text service");
            let mut composition = service.borrow_mut_composition().expect("composition");
            *composition = Composition {
                temporary_latin: true,
                deferred_actions: vec![DeferredClientAction {
                    action: ClientAction::SetTemporaryLatin(false),
                    transition: CompositionState::None,
                }],
                ..Composition::default()
            };
        }
        assert!(factory
            .deferred_input_ready()
            .expect("healthy queue is ready"));
        assert!(sink
            .OnTestKeyDown(Some(&context), WPARAM(0x71), LPARAM(0))
            .expect("claim healthy replay")
            .as_bool());
        assert!(!sink
            .OnKeyDown(Some(&context), WPARAM(0x71), LPARAM(0))
            .expect("replay then pass F2 through")
            .as_bool());
        assert!(!factory
            .has_deferred_input()
            .expect("healthy replay drained"));
        assert!(!snapshot(factory).temporary_latin);

        // A disabled host rejects ownership and prevents deferred mode changes from executing.
        let mut state = IMEState::get().expect("IME state");
        state.ipc_service = None;
        drop(state);
        {
            let service = factory.borrow().expect("text service");
            let mut composition = service.borrow_mut_composition().expect("composition");
            *composition = Composition {
                deferred_actions: vec![DeferredClientAction {
                    action: ClientAction::SetIMEMode(InputMode::Latin),
                    transition: CompositionState::None,
                }],
                ..Composition::default()
            };
        }
        let disabled_context = test_context::new(true);
        let before_disabled_test = snapshot(factory);
        assert!(!sink
            .OnTestKeyDown(Some(&disabled_context), WPARAM(0x41), LPARAM(0))
            .expect("disabled-host key test")
            .as_bool());
        assert_unchanged(factory, &before_disabled_test);
        // Skip the language-bar notification of this unactivated test factory.
        IMEState::get().expect("IME state").keyboard_disabled = true;
        assert!(!sink
            .OnKeyDown(Some(&disabled_context), WPARAM(0x41), LPARAM(0))
            .expect("disabled-host key handling")
            .as_bool());
        assert_eq!(IMEState::input_mode().expect("input mode"), InputMode::Kana);
        assert!(!factory
            .has_deferred_input()
            .expect("disabled work cancelled"));
        // This unactivated factory has no language bar. Reset the admission
        // state between fixtures rather than exercising unrelated activation UI.
        IMEState::get().expect("IME state").keyboard_disabled = false;

        IMEState::set_ipc_service(IPCService::recovery_for_test(true))
            .expect("pending IPC fixture");
        seed_composition(factory, CompositionState::Composing, true);
        let shift = WPARAM(0x10);
        let before_shift_test = snapshot(factory);
        assert!(sink
            .OnTestKeyDown(Some(&context), shift, LPARAM(0))
            .expect("test Shift")
            .as_bool());
        assert_unchanged(factory, &before_shift_test);
        assert!(sink
            .OnKeyDown(Some(&context), shift, LPARAM(0))
            .expect("handle Shift")
            .as_bool());
        assert_eq!(
            queued_actions(factory),
            [vec![ClientAction::SetTemporaryLatinShiftPending(true)]]
        );
        let before_chord_f2 = snapshot(factory);
        assert!(!sink
            .OnTestKeyDown(Some(&context), WPARAM(0x71), LPARAM(0))
            .expect("test F2 chord")
            .as_bool());
        assert_unchanged(factory, &before_chord_f2);
        let before_chord_up = snapshot(factory);
        assert!(sink
            .OnTestKeyUp(Some(&context), shift, LPARAM(0))
            .expect("test Shift-up")
            .as_bool());
        assert_unchanged(factory, &before_chord_up);
        assert!(sink
            .OnKeyUp(Some(&context), shift, LPARAM(0))
            .expect("handle chord Shift-up")
            .as_bool());
        assert_eq!(snapshot(factory).deferred_inputs.len(), 2);
        assert_eq!(
            queued_actions(factory).last().unwrap().as_slice(),
            &[ClientAction::SetTemporaryLatinShiftPending(false)]
        );

        seed_composition(factory, CompositionState::Composing, true);
        {
            let mut service = factory.borrow_mut().expect("text service");
            service.shift_key_down = false;
            service.shift_key_used_in_chord = false;
        }
        assert!(sink
            .OnTestKeyDown(Some(&context), shift, LPARAM(0))
            .expect("test Shift-only")
            .as_bool());
        assert!(sink
            .OnKeyDown(Some(&context), shift, LPARAM(0))
            .expect("handle Shift-only")
            .as_bool());
        assert_eq!(snapshot(factory).deferred_inputs.len(), 1);
        let before_shift_only_up = snapshot(factory);
        assert!(sink
            .OnTestKeyUp(Some(&context), shift, LPARAM(0))
            .expect("test Shift-only up")
            .as_bool());
        assert_unchanged(factory, &before_shift_only_up);
        assert!(sink
            .OnKeyUp(Some(&context), shift, LPARAM(0))
            .expect("handle Shift-only up")
            .as_bool());
        assert_eq!(snapshot(factory).deferred_inputs.len(), 2);
        assert_eq!(
            queued_actions(factory).last().unwrap().as_slice(),
            &[
                ClientAction::SetTemporaryLatin(false),
                ClientAction::SetTemporaryLatinShiftPending(false),
            ]
        );
    }
}
