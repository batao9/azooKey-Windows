use std::cmp::max;
use std::sync::Arc;

use azookey_server::TonicNamedPipeServer;
use ipc::{WindowAction, WindowController, WindowService};
use shared::{
    proto::window_service_server::WindowServiceServer,
    LIVE_CONVERSION_READING_VERTICAL_ADJUSTMENT_DEFAULT,
};
use tao::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use tao::platform::windows::{EventLoopBuilderExtWindows, WindowExtWindows};
use tao::{
    event::{Event, StartCause, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
};
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;
use tonic::transport::Server;
use uiaccess::prepare_uiaccess_token;
use utils::{
    get_candidate_window_position, get_candidate_window_position_with_ruby_clearance, CandidateRect,
};
use windows::Win32::UI::WindowsAndMessaging::{
    SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_HIDE,
};
use windows::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{ShowWindow, SW_SHOWNOACTIVATE},
};

pub mod candidate;
pub mod indicator;
pub mod ipc;
pub mod ruby;
pub mod uiaccess;
pub mod utils;

const INDICATOR_WINDOW_LEFT_OFFSET: i32 = 45;
const RUBY_WINDOW_MIN_WIDTH: u32 = 58;
const RUBY_WINDOW_MAX_WIDTH: u32 = 640;
const RUBY_WINDOW_HEIGHT: u32 = 42;
const RUBY_WINDOW_TEXT_WIDTH_PER_CHAR: u32 = 16;
const RUBY_WINDOW_HORIZONTAL_CHROME: u32 = 40;

fn place_candidate_windows(
    candidate_window: &tao::window::Window,
    indicator_window: &tao::window::Window,
    rect: CandidateRect,
    ruby_clearance: Option<(&tao::window::Window, i32)>,
) {
    let (x, y) = if let Some((ruby_window, vertical_adjustment)) = ruby_clearance {
        get_candidate_window_position_with_ruby_clearance(
            rect.top,
            rect.left,
            rect.bottom,
            rect.right,
            candidate_window,
            ruby_window,
            vertical_adjustment,
        )
    } else {
        get_candidate_window_position(
            rect.top,
            rect.left,
            rect.bottom,
            rect.right,
            candidate_window,
        )
    };
    candidate_window.set_outer_position(PhysicalPosition::new(x, y));
    indicator_window.set_outer_position(PhysicalPosition::new(
        (rect.left - INDICATOR_WINDOW_LEFT_OFFSET) as f64,
        rect.bottom as f64,
    ));
}

fn place_ruby_window(
    ruby_window: &tao::window::Window,
    rect: CandidateRect,
    vertical_adjustment: i32,
) {
    let (x, y) = utils::get_ruby_window_position(
        rect.top,
        rect.left,
        rect.bottom,
        rect.right,
        ruby_window,
        vertical_adjustment,
    );
    ruby_window.set_outer_position(PhysicalPosition::new(x, y));
}

fn ruby_clearance<'a>(
    ruby_window: &'a tao::window::Window,
    reading: &str,
    candidate_list_visible: bool,
    vertical_adjustment: i32,
) -> Option<(&'a tao::window::Window, i32)> {
    (candidate_list_visible && !reading.is_empty()).then_some((ruby_window, vertical_adjustment))
}

fn send_user_event(proxy: &EventLoopProxy<UserEvent>, event: UserEvent) {
    if let Err(error) = proxy.send_event(event) {
        eprintln!("Warning: Failed to send UI event: {error:?}");
    }
}

fn evaluate_script(webview: &wry::WebView, script: &str) {
    if let Err(error) = webview.evaluate_script(script) {
        eprintln!("Warning: Failed to evaluate WebView script: {error:?}");
    }
}

fn set_candidate_window_width(candidate_window: &tao::window::Window, candidates: &[String]) {
    let max_len = candidates
        .iter()
        .map(|s| s.chars().count())
        .max()
        .unwrap_or(0) as u32;

    let height = candidate_window.inner_size().height as i32;
    candidate_window.set_inner_size(PhysicalSize::new(
        max(225, 120 + max_len * 18),
        height as u32,
    ));
}

fn set_ruby_window_width(ruby_window: &tao::window::Window, reading: &str) {
    let len = reading.chars().count() as u32;
    let width = max(
        RUBY_WINDOW_MIN_WIDTH,
        RUBY_WINDOW_HORIZONTAL_CHROME
            .saturating_add(len.saturating_mul(RUBY_WINDOW_TEXT_WIDTH_PER_CHAR)),
    )
    .min(RUBY_WINDOW_MAX_WIDTH);
    ruby_window.set_inner_size(PhysicalSize::new(width, RUBY_WINDOW_HEIGHT));
}

fn update_candidate_list(candidate_webview: &wry::WebView, candidates: &[String]) {
    match serde_json::to_string(candidates) {
        Ok(candidates) => {
            evaluate_script(
                candidate_webview,
                &format!("updateCandidates({})", candidates),
            );
        }
        Err(error) => {
            eprintln!("Warning: Failed to serialize candidates: {error:?}");
        }
    }
}

fn update_ruby_reading(ruby_webview: &wry::WebView, reading: &str) {
    match serde_json::to_string(reading) {
        Ok(reading) => {
            evaluate_script(ruby_webview, &format!("updateReading({})", reading));
        }
        Err(error) => {
            eprintln!("Warning: Failed to serialize reading: {error:?}");
        }
    }
}

fn set_candidate_list_visible(candidate_webview: &wry::WebView, visible: bool) {
    evaluate_script(
        candidate_webview,
        &format!("setCandidateListVisible({})", visible),
    );
}

fn update_indicator(indicator_webview: &wry::WebView, input_method: &str) {
    match serde_json::to_string(input_method) {
        Ok(input_method) => evaluate_script(
            indicator_webview,
            &format!("updateInputMethod({})", input_method),
        ),
        Err(error) => {
            eprintln!("Warning: Failed to serialize input method: {error:?}");
        }
    }
}

fn show_window_no_activate(window: &tao::window::Window) {
    let _ = unsafe {
        ShowWindow(
            HWND(window.hwnd() as *mut std::ffi::c_void),
            SW_SHOWNOACTIVATE,
        )
    };
}

fn hide_window(window: &tao::window::Window) {
    let _ = unsafe { ShowWindow(HWND(window.hwnd() as *mut std::ffi::c_void), SW_HIDE) };
}

fn keep_windows_topmost(
    candidate_window: &tao::window::Window,
    ruby_window: &tao::window::Window,
    indicator_hwnd: isize,
) {
    unsafe {
        let _ = SetWindowPos(
            HWND(candidate_window.hwnd() as *mut std::ffi::c_void),
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );

        let _ = SetWindowPos(
            HWND(ruby_window.hwnd() as *mut std::ffi::c_void),
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );

        let _ = SetWindowPos(
            HWND(indicator_hwnd as *mut std::ffi::c_void),
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

#[derive(Debug)]
pub enum UserEvent {
    UpdateHeight(i32),
    UpdateCandidates(String),
    UpdateSelection(i32),
    UpdateInputMethod(String),
    WindowAction(WindowAction),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // obtain uiaccess token
    prepare_uiaccess_token()?;

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event()
        .with_any_thread(true)
        .build();

    // initialize window controller
    let (tx, mut rx) = mpsc::channel(32);
    let window_controller = WindowController::new(tx.clone());
    let grpc_service = WindowService {
        controller: window_controller.clone(),
    };

    // start grpc server
    tokio::spawn(async move {
        println!("WindowServer listening");
        Server::builder()
            .add_service(WindowServiceServer::new(grpc_service))
            .serve_with_incoming(TonicNamedPipeServer::new("azookey_ui"))
            .await
            .expect("gRPC server failed");
    });

    let event_loop_proxy = event_loop.create_proxy();
    let task_guard: Arc<Mutex<Option<JoinHandle<()>>>> = Arc::new(Mutex::new(None));

    let proxy_clone = event_loop_proxy.clone();
    let candidate_window = candidate::create_candidate_window(&event_loop)?;
    let candidate_webview_builder = candidate::create_candidate_webview()?;
    let candidate_webview = candidate_webview_builder
        .with_devtools(cfg!(debug_assertions))
        .with_ipc_handler(move |message| {
            if let Ok(message) = serde_json::from_str::<serde_json::Value>(message.body()) {
                if let Some(type_value) = message.get("type") {
                    if type_value == "resize" {
                        if let Some(height) = message.get("height") {
                            let height = height.as_f64().unwrap_or(0.0);
                            send_user_event(&proxy_clone, UserEvent::UpdateHeight(height as i32));
                        }
                    }
                }
            }
        })
        .build(&candidate_window)?;

    let indicator_window = indicator::create_indicator_window(&event_loop)?;
    let indicator_webview = indicator::create_indicator_webview(&indicator_window)?;
    let ruby_window = ruby::create_ruby_window(&event_loop)?;
    let ruby_webview = ruby::create_ruby_webview(&ruby_window)?;

    // handle window actions
    let proxy_clone = event_loop_proxy.clone();
    tokio::spawn(async move {
        while let Some(action) = rx.recv().await {
            send_user_event(&proxy_clone, UserEvent::WindowAction(action));
        }
    });

    let mut last_candidate_rect: Option<CandidateRect> = None;
    let mut current_reading = String::new();
    let mut current_candidate_list_visible = true;
    let mut current_reading_vertical_adjustment =
        LIVE_CONVERSION_READING_VERTICAL_ADJUSTMENT_DEFAULT;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        let indicator_hwnd = indicator_window.hwnd();

        match event {
            Event::NewEvents(StartCause::Init) => {}
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => *control_flow = ControlFlow::Exit,
            Event::UserEvent(script) => match script {
                UserEvent::UpdateCandidates(candidates) => {
                    evaluate_script(
                        &candidate_webview,
                        &format!("updateCandidates({})", candidates),
                    );
                }
                UserEvent::UpdateSelection(index) => {
                    evaluate_script(&candidate_webview, &format!("updateSelection({})", index));
                }
                UserEvent::UpdateInputMethod(input_method) => {
                    update_indicator(&indicator_webview, &input_method);
                }
                UserEvent::UpdateHeight(height) => {
                    let width = candidate_window.inner_size().width as i32;
                    candidate_window.set_inner_size(LogicalSize::new(width, height));
                    if let Some(rect) = last_candidate_rect {
                        place_candidate_windows(
                            &candidate_window,
                            &indicator_window,
                            rect,
                            ruby_clearance(
                                &ruby_window,
                                &current_reading,
                                current_candidate_list_visible,
                                current_reading_vertical_adjustment,
                            ),
                        );
                        if !current_reading.is_empty() {
                            place_ruby_window(
                                &ruby_window,
                                rect,
                                current_reading_vertical_adjustment,
                            );
                        }
                    }
                }
                UserEvent::WindowAction(action) => {
                    match action {
                        WindowAction::Show => {
                            // if mode indicator is already shown, hide it
                            let mut task_guard = match task_guard.try_lock() {
                                Ok(guard) => guard,
                                Err(_) => {
                                    eprintln!(
                                        "Warning: Failed to lock task_guard, skipping cleanup"
                                    );
                                    return;
                                }
                            };
                            if let Some(task) = task_guard.take() {
                                task.abort();
                                let _ = unsafe {
                                    ShowWindow(
                                        HWND(indicator_hwnd as *mut std::ffi::c_void),
                                        SW_HIDE,
                                    )
                                };
                            }

                            show_window_no_activate(&candidate_window);
                            if !current_reading.is_empty() {
                                show_window_no_activate(&ruby_window);
                            }
                        }
                        WindowAction::Hide => {
                            current_reading.clear();
                            update_ruby_reading(&ruby_webview, "");
                            hide_window(&candidate_window);
                            hide_window(&ruby_window);
                        }
                        WindowAction::SetPosition {
                            top,
                            left,
                            bottom,
                            right,
                        } => {
                            let rect = CandidateRect::new(top, left, bottom, right);
                            last_candidate_rect = Some(rect);

                            keep_windows_topmost(&candidate_window, &ruby_window, indicator_hwnd);
                            place_candidate_windows(
                                &candidate_window,
                                &indicator_window,
                                rect,
                                ruby_clearance(
                                    &ruby_window,
                                    &current_reading,
                                    current_candidate_list_visible,
                                    current_reading_vertical_adjustment,
                                ),
                            );
                            if !current_reading.is_empty() {
                                place_ruby_window(
                                    &ruby_window,
                                    rect,
                                    current_reading_vertical_adjustment,
                                );
                            }
                        }
                        WindowAction::SetCandidate { candidates } => {
                            current_candidate_list_visible = true;
                            set_candidate_list_visible(&candidate_webview, true);
                            set_candidate_window_width(&candidate_window, &candidates);
                            update_candidate_list(&candidate_webview, &candidates);
                            if let Some(rect) = last_candidate_rect {
                                place_candidate_windows(
                                    &candidate_window,
                                    &indicator_window,
                                    rect,
                                    ruby_clearance(
                                        &ruby_window,
                                        &current_reading,
                                        current_candidate_list_visible,
                                        current_reading_vertical_adjustment,
                                    ),
                                );
                                if !current_reading.is_empty() {
                                    place_ruby_window(
                                        &ruby_window,
                                        rect,
                                        current_reading_vertical_adjustment,
                                    );
                                }
                            }
                        }
                        WindowAction::SetSelection { index } => {
                            send_user_event(&event_loop_proxy, UserEvent::UpdateSelection(index));
                        }
                        WindowAction::SetInputMode(input_method) => {
                            update_indicator(&indicator_webview, &input_method);

                            let task_guard = task_guard.try_lock();

                            if let Ok(mut task_guard) = task_guard {
                                if let Some(task) = task_guard.take() {
                                    task.abort();
                                }

                                *task_guard = Some(tokio::spawn(async move {
                                    let _ = unsafe {
                                        ShowWindow(
                                            HWND(indicator_hwnd as *mut std::ffi::c_void),
                                            SW_SHOWNOACTIVATE,
                                        )
                                    };
                                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                                    let _ = unsafe {
                                        ShowWindow(
                                            HWND(indicator_hwnd as *mut std::ffi::c_void),
                                            SW_HIDE,
                                        )
                                    };
                                }));
                            }
                        }
                        WindowAction::UpdateCandidateWindow {
                            visible,
                            position,
                            candidates,
                            selected_index,
                            input_mode,
                            reading,
                            candidate_list_visible,
                            reading_vertical_adjustment,
                        } => {
                            if let Some(reading_vertical_adjustment) = reading_vertical_adjustment {
                                current_reading_vertical_adjustment = reading_vertical_adjustment;
                            }

                            if let Some(candidate_list_visible) = candidate_list_visible {
                                current_candidate_list_visible = candidate_list_visible;
                                set_candidate_list_visible(
                                    &candidate_webview,
                                    candidate_list_visible,
                                );
                                if !candidate_list_visible {
                                    hide_window(&candidate_window);
                                }
                            }

                            if let Some(ref reading) = reading {
                                current_reading = reading.clone();
                                update_ruby_reading(&ruby_webview, reading);
                                if reading.is_empty() {
                                    hide_window(&ruby_window);
                                } else {
                                    set_ruby_window_width(&ruby_window, reading);
                                }
                            }

                            if let Some(ref candidates) = candidates {
                                set_candidate_window_width(&candidate_window, candidates);
                                update_candidate_list(&candidate_webview, candidates);
                            }

                            if let Some(index) = selected_index {
                                evaluate_script(
                                    &candidate_webview,
                                    &format!("updateSelection({})", index),
                                );
                            }

                            if let Some(position) = position {
                                let rect = CandidateRect::new(
                                    position.top,
                                    position.left,
                                    position.bottom,
                                    position.right,
                                );
                                last_candidate_rect = Some(rect);
                                keep_windows_topmost(&candidate_window, &ruby_window, indicator_hwnd);
                                place_candidate_windows(
                                    &candidate_window,
                                    &indicator_window,
                                    rect,
                                    ruby_clearance(
                                        &ruby_window,
                                        &current_reading,
                                        current_candidate_list_visible,
                                        current_reading_vertical_adjustment,
                                    ),
                                );
                                if !current_reading.is_empty() {
                                    place_ruby_window(
                                        &ruby_window,
                                        rect,
                                        current_reading_vertical_adjustment,
                                    );
                                }
                            } else if candidates.is_some() || reading.is_some() {
                                if let Some(rect) = last_candidate_rect {
                                    place_candidate_windows(
                                        &candidate_window,
                                        &indicator_window,
                                        rect,
                                        ruby_clearance(
                                            &ruby_window,
                                            &current_reading,
                                            current_candidate_list_visible,
                                            current_reading_vertical_adjustment,
                                        ),
                                    );
                                    if !current_reading.is_empty() {
                                        place_ruby_window(
                                            &ruby_window,
                                            rect,
                                            current_reading_vertical_adjustment,
                                        );
                                    }
                                }
                            }

                            if let Some(input_method) = input_mode {
                                update_indicator(&indicator_webview, &input_method);

                                let task_guard = task_guard.try_lock();

                                if let Ok(mut task_guard) = task_guard {
                                    if let Some(task) = task_guard.take() {
                                        task.abort();
                                    }

                                    *task_guard = Some(tokio::spawn(async move {
                                        let _ = unsafe {
                                            ShowWindow(
                                                HWND(indicator_hwnd as *mut std::ffi::c_void),
                                                SW_SHOWNOACTIVATE,
                                            )
                                        };
                                        tokio::time::sleep(std::time::Duration::from_millis(500))
                                            .await;
                                        let _ = unsafe {
                                            ShowWindow(
                                                HWND(indicator_hwnd as *mut std::ffi::c_void),
                                                SW_HIDE,
                                            )
                                        };
                                    }));
                                }
                            }

                            if let Some(visible) = visible {
                                if visible {
                                    let mut task_guard = match task_guard.try_lock() {
                                        Ok(guard) => guard,
                                        Err(_) => {
                                            eprintln!(
                                                "Warning: Failed to lock task_guard, skipping cleanup"
                                            );
                                            return;
                                        }
                                    };
                                    if let Some(task) = task_guard.take() {
                                        task.abort();
                                        let _ = unsafe {
                                            ShowWindow(
                                                HWND(indicator_hwnd as *mut std::ffi::c_void),
                                                SW_HIDE,
                                            )
                                        };
                                    }

                                    if current_candidate_list_visible {
                                        show_window_no_activate(&candidate_window);
                                    } else {
                                        hide_window(&candidate_window);
                                    }

                                    if !current_reading.is_empty() {
                                        show_window_no_activate(&ruby_window);
                                    }
                                } else {
                                    current_reading.clear();
                                    update_ruby_reading(&ruby_webview, "");
                                    hide_window(&candidate_window);
                                    hide_window(&ruby_window);
                                }
                            } else if reading.is_some() && !current_reading.is_empty() {
                                show_window_no_activate(&ruby_window);
                            }
                        }
                    }
                }
            },
            _ => (),
        }
    });
}
