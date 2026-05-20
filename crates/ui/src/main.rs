use std::cmp::max;
use std::sync::Arc;

use azookey_server::TonicNamedPipeServer;
use ipc::{WindowAction, WindowController, WindowService};
use shared::proto::window_service_server::WindowServiceServer;
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
use utils::{get_candidate_window_position, CandidateRect};
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
pub mod uiaccess;
pub mod utils;

const INDICATOR_WINDOW_LEFT_OFFSET: i32 = 45;

fn place_candidate_windows(
    candidate_window: &tao::window::Window,
    indicator_window: &tao::window::Window,
    rect: CandidateRect,
) {
    let (x, y) = get_candidate_window_position(
        rect.top,
        rect.left,
        rect.bottom,
        rect.right,
        candidate_window,
    );
    candidate_window.set_outer_position(PhysicalPosition::new(x, y));
    indicator_window.set_outer_position(PhysicalPosition::new(
        (rect.left - INDICATOR_WINDOW_LEFT_OFFSET) as f64,
        rect.bottom as f64,
    ));
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

    // handle window actions
    let proxy_clone = event_loop_proxy.clone();
    tokio::spawn(async move {
        while let Some(action) = rx.recv().await {
            send_user_event(&proxy_clone, UserEvent::WindowAction(action));
        }
    });

    let mut last_candidate_rect: Option<CandidateRect> = None;

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
                    match serde_json::to_string(&input_method) {
                        Ok(input_method) => evaluate_script(
                            &indicator_webview,
                            &format!("updateInputMethod({})", input_method),
                        ),
                        Err(error) => {
                            eprintln!("Warning: Failed to serialize input method: {error:?}");
                        }
                    }
                }
                UserEvent::UpdateHeight(height) => {
                    let width = candidate_window.inner_size().width as i32;
                    candidate_window.set_inner_size(LogicalSize::new(width, height));
                    if let Some(rect) = last_candidate_rect {
                        place_candidate_windows(&candidate_window, &indicator_window, rect);
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

                            let _ = unsafe {
                                ShowWindow(
                                    HWND(candidate_window.hwnd() as *mut std::ffi::c_void),
                                    SW_SHOWNOACTIVATE,
                                )
                            };
                        }
                        WindowAction::Hide => {
                            let _ = unsafe {
                                ShowWindow(
                                    HWND(candidate_window.hwnd() as *mut std::ffi::c_void),
                                    SW_HIDE,
                                )
                            };
                        }
                        WindowAction::SetPosition {
                            top,
                            left,
                            bottom,
                            right,
                        } => {
                            let rect = CandidateRect::new(top, left, bottom, right);
                            last_candidate_rect = Some(rect);

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
                                    HWND(indicator_hwnd as *mut std::ffi::c_void),
                                    HWND_TOPMOST,
                                    0,
                                    0,
                                    0,
                                    0,
                                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                                );
                            }
                            place_candidate_windows(&candidate_window, &indicator_window, rect);
                        }
                        WindowAction::SetCandidate { candidates } => {
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

                            match serde_json::to_string(&candidates) {
                                Ok(candidates) => {
                                    send_user_event(
                                        &event_loop_proxy,
                                        UserEvent::UpdateCandidates(candidates),
                                    );
                                }
                                Err(error) => {
                                    eprintln!("Warning: Failed to serialize candidates: {error:?}");
                                }
                            }
                            if let Some(rect) = last_candidate_rect {
                                place_candidate_windows(&candidate_window, &indicator_window, rect);
                            }
                        }
                        WindowAction::SetSelection { index } => {
                            send_user_event(&event_loop_proxy, UserEvent::UpdateSelection(index));
                        }
                        WindowAction::SetInputMode(input_method) => {
                            send_user_event(
                                &event_loop_proxy,
                                UserEvent::UpdateInputMethod(input_method),
                            );

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
                    }
                }
            },
            _ => (),
        }
    });
}
