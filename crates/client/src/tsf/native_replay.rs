//! Recovery-only continuation after the host has had a message turn to consume
//! an IMM32 partial result. No timer, wait, worker thread, or per-key window.
use std::{
    cell::{Cell, RefCell},
    fmt,
    rc::{Rc, Weak},
    sync::atomic::{AtomicUsize, Ordering},
};
use windows::{
    core::{w, Error, PCWSTR},
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowLongPtrW, PostMessageW,
            RegisterClassW, SetWindowLongPtrW, UnregisterClassW, CS_IME, GWLP_USERDATA, HMENU,
            HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_NCDESTROY, WNDCLASSW,
        },
    },
};

const PREPARE: u32 = WM_APP + 0x203;
const RUN: u32 = WM_APP + 0x204;
static SEQUENCE: AtomicUsize = AtomicUsize::new(1);

#[derive(Default)]
pub(crate) struct NativeDispatch {
    depth: Cell<usize>,
    replay: RefCell<Weak<State>>,
}

impl fmt::Debug for NativeDispatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativeDispatch")
            .field("depth", &self.depth.get())
            .finish()
    }
}

impl NativeDispatch {
    pub(crate) fn enter(self: &Rc<Self>) -> NativeDispatchGuard {
        self.depth.set(self.depth.get() + 1);
        NativeDispatchGuard(self.clone())
    }
}

pub(crate) struct NativeDispatchGuard(Rc<NativeDispatch>);
impl Drop for NativeDispatchGuard {
    fn drop(&mut self) {
        self.0.depth.set(self.0.depth.get() - 1);
        if self.0.depth.get() == 0 {
            let replay = self.0.replay.borrow().upgrade();
            if let Some(replay) = replay {
                replay.arm();
            }
        }
    }
}

struct State {
    hwnd: Cell<HWND>,
    dispatch: Rc<NativeDispatch>,
    callback: RefCell<Option<Box<dyn FnOnce()>>>,
    token: Cell<usize>,
    armed: Cell<bool>,
    #[cfg(test)]
    fail_post: Cell<bool>,
}

impl State {
    fn send(&self, message: u32, token: usize) -> bool {
        #[cfg(test)]
        if self.fail_post.get() {
            return false;
        }
        unsafe { PostMessageW(self.hwnd.get(), message, WPARAM(token), LPARAM(0)) }.is_ok()
    }

    fn arm(&self) {
        if self.dispatch.depth.get() != 0
            || self.callback.borrow().is_none()
            || self.armed.get()
            || self.hwnd.get().0.is_null()
        {
            return;
        }
        if self.send(PREPARE, self.token.get()) {
            self.armed.set(true);
        }
        // A full host queue leaves the callback pending; the next outer input
        // callback retries arming, without releasing accepted keys or spinning.
    }

    fn message(&self, message: u32, token: usize) {
        if token != self.token.get() || self.callback.borrow().is_none() {
            return;
        }
        self.armed.set(false);
        if self.dispatch.depth.get() != 0 {
            return;
        }
        if message == PREPARE {
            // TSF can enqueue the result only after its key callback returns.
            // This second posted turn goes behind those result notifications.
            if self.send(RUN, token) {
                self.armed.set(true);
            }
        } else {
            let callback = self.callback.borrow_mut().take();
            if let Some(callback) = callback {
                callback();
            }
        }
    }

    fn cancel(&self) {
        self.token.set(SEQUENCE.fetch_add(1, Ordering::Relaxed));
        self.armed.set(false);
        let callback = self.callback.borrow_mut().take();
        drop(callback);
    }
}

pub(crate) struct NativeReplay {
    state: Rc<State>,
    class: Vec<u16>,
    instance: HINSTANCE,
}

impl fmt::Debug for NativeReplay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativeReplay")
            .field("pending", &self.is_pending())
            .finish()
    }
}

impl NativeReplay {
    pub(crate) fn new(dispatch: Rc<NativeDispatch>) -> anyhow::Result<Self> {
        #[cfg(not(test))]
        let instance = HINSTANCE(crate::globals::DllModule::module_handle()?.0);
        #[cfg(test)]
        let instance =
            HINSTANCE(unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None)? }.0);
        let class: Vec<u16> = format!(
            "AzookeyNativeReplay-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        )
        .encode_utf16()
        .chain([0])
        .collect();
        let definition = WNDCLASSW {
            style: CS_IME,
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            lpszClassName: PCWSTR(class.as_ptr()),
            ..Default::default()
        };
        if unsafe { RegisterClassW(&definition) } == 0 {
            return Err(Error::from_win32().into());
        }
        let hwnd = match unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(class.as_ptr()),
                w!(""),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                HMENU::default(),
                instance,
                None,
            )
        } {
            Ok(hwnd) => hwnd,
            Err(error) => {
                unsafe {
                    let _ = UnregisterClassW(PCWSTR(class.as_ptr()), instance);
                }
                return Err(error.into());
            }
        };
        let state = Rc::new(State {
            hwnd: Cell::new(hwnd),
            dispatch: dispatch.clone(),
            callback: RefCell::new(None),
            token: Cell::new(0),
            armed: Cell::new(false),
            #[cfg(test)]
            fail_post: Cell::new(false),
        });
        unsafe {
            SetWindowLongPtrW(
                hwnd,
                GWLP_USERDATA,
                Rc::into_raw(state.clone()) as isize as _,
            );
        }
        *dispatch.replay.borrow_mut() = Rc::downgrade(&state);
        Ok(Self {
            state,
            class,
            instance,
        })
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.state.callback.borrow().is_some()
    }
    pub(crate) fn post(&self, callback: impl FnOnce() + 'static) {
        self.state.cancel();
        self.state
            .token
            .set(SEQUENCE.fetch_add(1, Ordering::Relaxed));
        *self.state.callback.borrow_mut() = Some(Box::new(callback));
        self.state.arm();
    }
    pub(crate) fn cancel(&self) {
        self.state.cancel();
    }
}

impl Drop for NativeReplay {
    fn drop(&mut self) {
        self.cancel();
        unsafe {
            if !self.state.hwnd.get().0.is_null() {
                let _ = DestroyWindow(self.state.hwnd.get());
            }
            let _ = UnregisterClassW(PCWSTR(self.class.as_ptr()), self.instance);
        }
    }
}

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const State;
    if pointer.is_null() {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }
    Rc::increment_strong_count(pointer);
    let state = Rc::from_raw(pointer);
    if message == WM_NCDESTROY {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        state.hwnd.set(HWND::default());
        state.cancel();
        drop(Rc::from_raw(pointer));
    } else if message == PREPARE || message == RUN {
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            state.message(message, wparam.0)
        }))
        .is_err()
        {
            tracing::error!("Native recovery continuation callback panicked");
        }
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, message, wparam, lparam)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, PeekMessageW, MSG, PM_REMOVE};

    fn pump() {
        for _ in 0..16 {
            let mut message = MSG::default();
            if !unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                break;
            }
            unsafe {
                DispatchMessageW(&message);
            }
        }
    }

    #[test]
    fn nested_dispatch_cannot_run_replay_before_outer_native_batch_returns() {
        for queued_stage in 0..3 {
            let dispatch = Rc::new(NativeDispatch::default());
            let replay = NativeReplay::new(dispatch.clone()).unwrap();
            let count = Rc::new(Cell::new(0));
            let before_post = (queued_stage == 0).then(|| dispatch.enter());
            let observed = count.clone();
            replay.post(move || observed.set(observed.get() + 1));
            if queued_stage == 2 {
                let mut message = MSG::default();
                assert!(unsafe {
                    PeekMessageW(
                        &mut message,
                        replay.state.hwnd.get(),
                        PREPARE,
                        PREPARE,
                        PM_REMOVE,
                    )
                }
                .as_bool());
                unsafe {
                    DispatchMessageW(&message);
                }
                assert_eq!(count.get(), 0);
            }
            let after_post = (queued_stage != 0).then(|| dispatch.enter());
            pump();
            assert_eq!(count.get(), 0, "queued stage {queued_stage}");
            assert!(replay.is_pending());
            drop(before_post);
            drop(after_post);
            pump();
            assert_eq!(count.get(), 1);
            assert!(!replay.is_pending());
        }
    }

    #[test]
    fn cancelled_duplicate_and_reused_window_messages_cannot_run_new_ticket() {
        let dispatch = Rc::new(NativeDispatch::default());
        let replay = NativeReplay::new(dispatch).unwrap();
        replay.post(|| panic!("cancelled callback"));
        let stale = replay.state.token.get();
        replay.cancel();
        let count = Rc::new(Cell::new(0));
        let observed = count.clone();
        replay.post(move || observed.set(observed.get() + 1));
        unsafe {
            PostMessageW(replay.state.hwnd.get(), RUN, WPARAM(stale), LPARAM(0)).unwrap();
        }
        pump();
        assert_eq!(count.get(), 1);
        let finished = replay.state.token.get();
        unsafe {
            PostMessageW(replay.state.hwnd.get(), RUN, WPARAM(finished), LPARAM(0)).unwrap();
        }
        pump();
        assert_eq!(count.get(), 1);
        drop(replay);
        let next = NativeReplay::new(Rc::new(NativeDispatch::default())).unwrap();
        let next_count = Rc::new(Cell::new(0));
        let observed = next_count.clone();
        next.post(move || observed.set(observed.get() + 1));
        unsafe {
            PostMessageW(next.state.hwnd.get(), RUN, WPARAM(stale), LPARAM(0)).unwrap();
        }
        let mut message = MSG::default();
        assert!(
            unsafe { PeekMessageW(&mut message, next.state.hwnd.get(), RUN, RUN, PM_REMOVE) }
                .as_bool()
        );
        unsafe {
            DispatchMessageW(&message);
        }
        assert_eq!(next_count.get(), 0);
        assert!(next.is_pending());
        pump();
        assert_eq!(next_count.get(), 1);
        assert!(!next.is_pending());
        next.post(|| panic!("destroyed pending callback"));
        drop(next);
        pump();
    }

    #[test]
    fn failed_post_keeps_ticket_and_rearms_after_a_later_outer_callback() {
        let dispatch = Rc::new(NativeDispatch::default());
        let replay = NativeReplay::new(dispatch.clone()).unwrap();
        let count = Rc::new(Cell::new(0));
        let observed = count.clone();
        replay.state.fail_post.set(true);
        replay.post(move || observed.set(observed.get() + 1));
        pump();
        assert_eq!(count.get(), 0);
        assert!(replay.is_pending());
        replay.state.fail_post.set(false);
        drop(dispatch.enter());
        pump();
        assert_eq!(count.get(), 1);
        assert!(!replay.is_pending());
    }
}
