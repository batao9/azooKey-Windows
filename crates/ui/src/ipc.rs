use shared::proto::{
    window_service_server::WindowService as WindowServiceProto, EmptyResponse, SetCandidateRequest,
    SetInputModeRequest, SetPositionRequest, SetSelectionRequest, UpdateCandidateWindowRequest,
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
}

impl WindowService {
    async fn send_action(&self, action: WindowAction) -> Result<Response<EmptyResponse>, Status> {
        self.controller
            .sender
            .send(action)
            .await
            .map_err(|_| Status::internal("window event channel is closed"))?;

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
        let request = request.into_inner();
        let position = request.position.map(|position| WindowPositionAction {
            top: position.top,
            left: position.left,
            bottom: position.bottom,
            right: position.right,
        });
        let candidates = request
            .candidates
            .map(|candidate_list| candidate_list.candidates);

        self.send_action(WindowAction::UpdateCandidateWindow {
            visible: request.visible,
            position,
            candidates,
            selected_index: request.selected_index,
            input_mode: request.input_mode,
            reading: request.reading,
            candidate_list_visible: request.candidate_list_visible,
            reading_vertical_adjustment: request.reading_vertical_adjustment,
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shared::proto::WindowPosition;
    use tonic::Code;

    fn service_with_receiver() -> (WindowService, mpsc::Receiver<WindowAction>) {
        let (sender, receiver) = mpsc::channel(1);
        (
            WindowService {
                controller: WindowController::new(sender),
            },
            receiver,
        )
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
