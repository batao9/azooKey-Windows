use shared::proto::{
    window_service_server::WindowService as WindowServiceProto, EmptyResponse, SetCandidateRequest,
    SetInputModeRequest, SetPositionRequest, SetSelectionRequest, SyncCandidateWindowRequest,
    SyncCandidateWindowResponse, UpdateCandidateWindowRequest,
};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        OnceLock,
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc;
use tonic::{Request, Response, Status};

#[derive(Debug, Clone)]
pub struct WindowController {
    sender: mpsc::Sender<WindowAction>,
}

impl WindowController {
    pub fn new(sender: mpsc::Sender<WindowAction>) -> Self {
        Self { sender }
    }
}

// ウィンドウ操作コマンド
#[derive(Debug, serde::Serialize)]
pub enum WindowAction {
    Show,
    Hide,
    SetPosition {
        top: i32,
        left: i32,
        bottom: i32,
        right: i32,
    },
    SetSelection {
        index: i32,
    },
    SetCandidate {
        candidates: Vec<String>,
    },
    SetInputMode(String),
    UpdateCandidateWindow {
        visible: Option<bool>,
        position: Option<WindowPositionAction>,
        candidates: Option<Vec<String>>,
        selected_index: Option<i32>,
        input_mode: Option<String>,
        reading: Option<String>,
        candidate_list_visible: Option<bool>,
        reading_vertical_adjustment: Option<i32>,
    },
}

#[derive(Debug, serde::Serialize)]
pub struct WindowPositionAction {
    pub top: i32,
    pub left: i32,
    pub bottom: i32,
    pub right: i32,
}

#[derive(Debug)]
pub struct WindowService {
    pub controller: WindowController,
    session_id: u64,
    candidate_revision: tokio::sync::Mutex<u64>,
}

impl WindowService {
    pub fn new(controller: WindowController) -> Self {
        static SESSIONS: OnceLock<AtomicU64> = OnceLock::new();
        let sequence = SESSIONS.get_or_init(|| {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            AtomicU64::new(((u64::from(std::process::id()) << 32) ^ timestamp).max(1))
        });
        let mut session_id = sequence.fetch_add(1, Ordering::Relaxed);
        if session_id == 0 {
            session_id = sequence.fetch_add(1, Ordering::Relaxed);
        }
        Self {
            controller,
            session_id,
            candidate_revision: tokio::sync::Mutex::new(0),
        }
    }

    // Match the Tonic service methods that consume this helper's result.
    #[allow(clippy::result_large_err)]
    async fn send_action(&self, action: WindowAction) -> Result<Response<EmptyResponse>, Status> {
        // Revision changes and actions must have the same queue order. Legacy
        // callers cannot prove which generation they are changing.
        let mut revision = self.candidate_revision.lock().await;
        self.controller
            .sender
            .send(action)
            .await
            .map_err(|_| Status::internal("window event channel is closed"))?;
        *revision = revision.wrapping_add(1).max(1);

        Ok(Response::new(EmptyResponse {}))
    }
}

#[tonic::async_trait]
impl WindowServiceProto for WindowService {
    async fn show_window(
        &self,
        _request: Request<EmptyResponse>,
    ) -> Result<Response<EmptyResponse>, Status> {
        self.send_action(WindowAction::Show).await
    }

    async fn hide_window(
        &self,
        _request: Request<EmptyResponse>,
    ) -> Result<Response<EmptyResponse>, Status> {
        self.send_action(WindowAction::Hide).await
    }
    async fn set_window_position(
        &self,
        request: Request<SetPositionRequest>,
    ) -> Result<Response<EmptyResponse>, Status> {
        let position = request
            .into_inner()
            .position
            .ok_or_else(|| Status::invalid_argument("position is required"))?;
        let top = position.top;
        let left = position.left;
        let bottom = position.bottom;
        let right = position.right;
        self.send_action(WindowAction::SetPosition {
            top,
            left,
            bottom,
            right,
        })
        .await
    }

    async fn set_candidate(
        &self,
        request: Request<SetCandidateRequest>,
    ) -> Result<Response<EmptyResponse>, Status> {
        let candidate = request.into_inner().candidates;

        self.send_action(WindowAction::SetCandidate {
            candidates: candidate,
        })
        .await
    }

    async fn set_selection(
        &self,
        request: Request<SetSelectionRequest>,
    ) -> Result<Response<EmptyResponse>, Status> {
        let index = request.into_inner().index;
        self.send_action(WindowAction::SetSelection { index }).await
    }

    async fn set_input_mode(
        &self,
        request: Request<SetInputModeRequest>,
    ) -> Result<Response<EmptyResponse>, Status> {
        let mode = request.into_inner().mode;
        self.send_action(WindowAction::SetInputMode(mode)).await
    }

    async fn update_candidate_window(
        &self,
        request: Request<UpdateCandidateWindowRequest>,
    ) -> Result<Response<EmptyResponse>, Status> {
        self.send_action(candidate_window_action(request.into_inner()))
            .await
    }

    async fn sync_candidate_window(
        &self,
        request: Request<SyncCandidateWindowRequest>,
    ) -> Result<Response<SyncCandidateWindowResponse>, Status> {
        let request = request.into_inner();
        let update = request
            .update
            .ok_or_else(|| Status::invalid_argument("update is required"))?;
        let expected = match (
            request.expected_server_session_id,
            request.expected_candidate_revision,
        ) {
            (None, None) => None,
            (Some(session), Some(revision)) if update.candidates.is_none() => {
                Some((session, revision))
            }
            _ => {
                return Err(Status::invalid_argument(
                    "a delta requires both revision fields and no candidate list",
                ))
            }
        };
        let mut revision = self.candidate_revision.lock().await;
        if expected.is_some_and(|(session, expected)| {
            session != self.session_id || expected == 0 || expected != *revision
        }) {
            return Ok(Response::new(SyncCandidateWindowResponse {
                server_session_id: self.session_id,
                candidate_revision: *revision,
                applied: false,
            }));
        }
        let invalidate = update.candidates.is_some()
            || update.visible == Some(false)
            || update.input_mode.is_some();
        self.controller
            .sender
            .send(candidate_window_action(update))
            .await
            .map_err(|_| Status::internal("window event channel is closed"))?;
        if invalidate {
            *revision = revision.wrapping_add(1).max(1);
        }
        Ok(Response::new(SyncCandidateWindowResponse {
            server_session_id: self.session_id,
            candidate_revision: *revision,
            applied: true,
        }))
    }
}

fn candidate_window_action(request: UpdateCandidateWindowRequest) -> WindowAction {
    let position = request.position.map(|position| WindowPositionAction {
        top: position.top,
        left: position.left,
        bottom: position.bottom,
        right: position.right,
    });
    let candidates = request
        .candidates
        .map(|candidate_list| candidate_list.candidates);

    WindowAction::UpdateCandidateWindow {
        visible: request.visible,
        position,
        candidates,
        selected_index: request.selected_index,
        input_mode: request.input_mode,
        reading: request.reading,
        candidate_list_visible: request.candidate_list_visible,
        reading_vertical_adjustment: request.reading_vertical_adjustment,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shared::proto::WindowPosition;
    use tonic::Code;

    fn service_with_receiver() -> (WindowService, mpsc::Receiver<WindowAction>) {
        let (sender, receiver) = mpsc::channel(1);
        (WindowService::new(WindowController::new(sender)), receiver)
    }

    fn full_update(text: &str) -> UpdateCandidateWindowRequest {
        UpdateCandidateWindowRequest {
            candidates: Some(shared::proto::CandidateList {
                candidates: vec![text.into()],
            }),
            selected_index: Some(0),
            visible: Some(true),
            ..Default::default()
        }
    }

    fn guarded_selection(
        response: &SyncCandidateWindowResponse,
        index: i32,
    ) -> SyncCandidateWindowRequest {
        SyncCandidateWindowRequest {
            update: Some(UpdateCandidateWindowRequest {
                selected_index: Some(index),
                ..Default::default()
            }),
            expected_server_session_id: Some(response.server_session_id),
            expected_candidate_revision: Some(response.candidate_revision),
        }
    }

    #[tokio::test]
    async fn sync_selection_requires_the_acknowledged_generation_and_preserves_queue_order() {
        let (service, mut receiver) = service_with_receiver();
        let first = service
            .sync_candidate_window(Request::new(SyncCandidateWindowRequest {
                update: Some(full_update("かな")),
                ..Default::default()
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(first.applied);
        assert_ne!(first.server_session_id, 0);
        assert_ne!(first.candidate_revision, 0);
        assert!(matches!(
            receiver.recv().await.unwrap(),
            WindowAction::UpdateCandidateWindow {
                candidates: Some(_),
                ..
            }
        ));
        let delta = service
            .sync_candidate_window(Request::new(guarded_selection(&first, 1)))
            .await
            .unwrap()
            .into_inner();
        assert!(delta.applied);
        assert_eq!(delta.candidate_revision, first.candidate_revision);
        assert!(matches!(
            receiver.recv().await.unwrap(),
            WindowAction::UpdateCandidateWindow {
                candidates: None,
                selected_index: Some(1),
                ..
            }
        ));

        service
            .update_candidate_window(Request::new(full_update("別の候補")))
            .await
            .unwrap();
        receiver.recv().await.unwrap();
        let stale = service
            .sync_candidate_window(Request::new(guarded_selection(&first, 2)))
            .await
            .unwrap()
            .into_inner();
        assert!(!stale.applied);
        assert!(receiver.try_recv().is_err());
        let repaired = service
            .sync_candidate_window(Request::new(SyncCandidateWindowRequest {
                update: Some(full_update("現在の候補")),
                ..Default::default()
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(repaired.applied);
        assert_ne!(repaired.candidate_revision, first.candidate_revision);
        receiver.recv().await.unwrap();

        // A legacy selection is enqueued in place, before the next full frame.
        service
            .set_selection(Request::new(SetSelectionRequest { index: 3 }))
            .await
            .unwrap();
        assert!(matches!(
            receiver.recv().await.unwrap(),
            WindowAction::SetSelection { index: 3 }
        ));
        assert!(
            !service
                .sync_candidate_window(Request::new(guarded_selection(&repaired, 4)))
                .await
                .unwrap()
                .into_inner()
                .applied
        );
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn sync_selection_rejects_a_restarted_ui_even_when_revision_numbers_match() {
        let (service, mut receiver) = service_with_receiver();
        let initial = service
            .sync_candidate_window(Request::new(SyncCandidateWindowRequest {
                update: Some(full_update("初回")),
                ..Default::default()
            }))
            .await
            .unwrap()
            .into_inner();
        receiver.recv().await.unwrap();
        let restarted = WindowService::new(service.controller.clone());
        let replacement = restarted
            .sync_candidate_window(Request::new(SyncCandidateWindowRequest {
                update: Some(full_update("再起動後")),
                ..Default::default()
            }))
            .await
            .unwrap()
            .into_inner();
        receiver.recv().await.unwrap();
        assert_eq!(initial.candidate_revision, replacement.candidate_revision);
        assert_ne!(initial.server_session_id, replacement.server_session_id);
        assert!(
            !restarted
                .sync_candidate_window(Request::new(guarded_selection(&initial, 1)))
                .await
                .unwrap()
                .into_inner()
                .applied
        );
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn sync_position_updates_do_not_invalidate_candidates_but_hidden_windows_do() {
        let (service, mut receiver) = service_with_receiver();
        let initial = service
            .sync_candidate_window(Request::new(SyncCandidateWindowRequest {
                update: Some(full_update("候補")),
                ..Default::default()
            }))
            .await
            .unwrap()
            .into_inner();
        receiver.recv().await.unwrap();
        let moved = service
            .sync_candidate_window(Request::new(SyncCandidateWindowRequest {
                update: Some(UpdateCandidateWindowRequest {
                    position: Some(WindowPosition {
                        top: 1,
                        left: 2,
                        bottom: 3,
                        right: 4,
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            }))
            .await
            .unwrap()
            .into_inner();
        receiver.recv().await.unwrap();
        assert_eq!(moved.candidate_revision, initial.candidate_revision);
        service
            .sync_candidate_window(Request::new(SyncCandidateWindowRequest {
                update: Some(UpdateCandidateWindowRequest {
                    visible: Some(false),
                    ..Default::default()
                }),
                ..Default::default()
            }))
            .await
            .unwrap();
        receiver.recv().await.unwrap();
        assert!(
            !service
                .sync_candidate_window(Request::new(guarded_selection(&initial, 1)))
                .await
                .unwrap()
                .into_inner()
                .applied
        );
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn sync_rejects_incomplete_guards_without_enqueuing_an_action() {
        let (service, mut receiver) = service_with_receiver();
        let error = service
            .sync_candidate_window(Request::new(SyncCandidateWindowRequest {
                update: Some(UpdateCandidateWindowRequest::default()),
                expected_server_session_id: Some(1),
                expected_candidate_revision: None,
            }))
            .await
            .unwrap_err();
        assert_eq!(error.code(), Code::InvalidArgument);
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn set_window_position_without_position_returns_invalid_argument() {
        let (service, _receiver) = service_with_receiver();

        let error = service
            .set_window_position(Request::new(SetPositionRequest { position: None }))
            .await
            .expect_err("missing position should be rejected");

        assert_eq!(error.code(), Code::InvalidArgument);
    }

    #[tokio::test]
    async fn set_window_position_sends_action() {
        let (service, mut receiver) = service_with_receiver();

        service
            .set_window_position(Request::new(SetPositionRequest {
                position: Some(WindowPosition {
                    top: 1,
                    left: 2,
                    bottom: 3,
                    right: 4,
                }),
            }))
            .await
            .expect("valid position should be sent");

        match receiver.recv().await.expect("action should be queued") {
            WindowAction::SetPosition {
                top,
                left,
                bottom,
                right,
            } => {
                assert_eq!((top, left, bottom, right), (1, 2, 3, 4));
            }
            action => panic!("unexpected action: {action:?}"),
        }
    }

    #[tokio::test]
    async fn update_candidate_window_sends_batched_action() {
        let (service, mut receiver) = service_with_receiver();

        service
            .update_candidate_window(Request::new(UpdateCandidateWindowRequest {
                visible: Some(true),
                position: Some(WindowPosition {
                    top: 1,
                    left: 2,
                    bottom: 3,
                    right: 4,
                }),
                candidates: Some(shared::proto::CandidateList {
                    candidates: vec!["候補".to_string()],
                }),
                selected_index: Some(0),
                input_mode: Some("あ".to_string()),
                reading: Some("こうほ".to_string()),
                candidate_list_visible: Some(true),
                reading_vertical_adjustment: Some(4),
            }))
            .await
            .expect("batched update should be sent");

        match receiver.recv().await.expect("action should be queued") {
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
                assert_eq!(visible, Some(true));
                let position = position.expect("position should be included");
                assert_eq!(
                    (position.top, position.left, position.bottom, position.right),
                    (1, 2, 3, 4)
                );
                assert_eq!(candidates, Some(vec!["候補".to_string()]));
                assert_eq!(selected_index, Some(0));
                assert_eq!(input_mode, Some("あ".to_string()));
                assert_eq!(reading, Some("こうほ".to_string()));
                assert_eq!(candidate_list_visible, Some(true));
                assert_eq!(reading_vertical_adjustment, Some(4));
            }
            action => panic!("unexpected action: {action:?}"),
        }
    }

    #[tokio::test]
    async fn closed_channel_returns_internal_status() {
        let (service, receiver) = service_with_receiver();
        drop(receiver);

        let error = service
            .show_window(Request::new(EmptyResponse {}))
            .await
            .expect_err("closed channel should be reported as a gRPC error");

        assert_eq!(error.code(), Code::Internal);
    }
}
