use super::*;
use shared::proto::{
    SyncCandidateWindowRequest, SyncCandidateWindowResponse, UpdateCandidateWindowRequest,
};
use std::{cell::Cell, convert::Infallible, sync::atomic::AtomicUsize};
use tonic::codegen::{http, BoxFuture, Service};

struct State {
    session: u64,
    revision: u64,
    applied: Vec<UpdateCandidateWindowRequest>,
    requests: Vec<SyncCandidateWindowRequest>,
    legacy: Vec<UpdateCandidateWindowRequest>,
}

#[derive(Clone)]
struct Probe {
    state: Arc<Mutex<State>>,
    legacy_only: Arc<AtomicBool>,
    fail_next: Arc<AtomicBool>,
}

impl tonic::server::NamedService for Probe {
    const NAME: &'static str = "window.WindowService";
}

impl tonic::server::UnaryService<SyncCandidateWindowRequest> for Probe {
    type Response = SyncCandidateWindowResponse;
    type Future = BoxFuture<tonic::Response<Self::Response>, tonic::Status>;

    fn call(&mut self, request: tonic::Request<SyncCandidateWindowRequest>) -> Self::Future {
        let request = request.into_inner();
        let mut state = self.state.lock().unwrap();
        state.requests.push(request.clone());
        let response = if self.legacy_only.load(Ordering::Acquire) {
            Err(tonic::Status::unimplemented("guarded RPC unavailable"))
        } else if self.fail_next.swap(false, Ordering::AcqRel) {
            Err(tonic::Status::unavailable("UI connection lost"))
        } else {
            let expected = request
                .expected_server_session_id
                .zip(request.expected_candidate_revision);
            let applied = expected.is_none_or(|(session, revision)| {
                session == state.session && revision != 0 && revision == state.revision
            });
            if applied {
                let update = request.update.unwrap();
                if update.candidates.is_some() {
                    state.revision += 1;
                }
                state.applied.push(update);
            }
            Ok(tonic::Response::new(SyncCandidateWindowResponse {
                server_session_id: state.session,
                candidate_revision: state.revision,
                applied,
            }))
        };
        Box::pin(async move { response })
    }
}

impl tonic::server::UnaryService<UpdateCandidateWindowRequest> for Probe {
    type Response = shared::proto::EmptyResponse;
    type Future = BoxFuture<tonic::Response<Self::Response>, tonic::Status>;

    fn call(&mut self, request: tonic::Request<UpdateCandidateWindowRequest>) -> Self::Future {
        let request = request.into_inner();
        let mut state = self.state.lock().unwrap();
        state.revision += 1;
        state.legacy.push(request.clone());
        state.applied.push(request);
        Box::pin(async { Ok(tonic::Response::new(shared::proto::EmptyResponse {})) })
    }
}

impl Service<http::Request<tonic::body::BoxBody>> for Probe {
    type Response = http::Response<tonic::body::BoxBody>;
    type Error = Infallible;
    type Future = BoxFuture<Self::Response, Self::Error>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: http::Request<tonic::body::BoxBody>) -> Self::Future {
        let guarded = request.uri().path().ends_with("/SyncCandidateWindow");
        let probe = self.clone();
        Box::pin(async move {
            if guarded {
                Ok(tonic::server::Grpc::new(tonic::codec::ProstCodec::<
                    SyncCandidateWindowResponse,
                    SyncCandidateWindowRequest,
                >::default())
                .unary(probe, request)
                .await)
            } else {
                Ok(tonic::server::Grpc::new(tonic::codec::ProstCodec::<
                    shared::proto::EmptyResponse,
                    UpdateCandidateWindowRequest,
                >::default())
                .unary(probe, request)
                .await)
            }
        })
    }
}

struct Incoming(tokio::net::TcpListener);
impl tonic::codegen::tokio_stream::Stream for Incoming {
    type Item = std::io::Result<tokio::net::TcpStream>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.0
            .poll_accept(cx)
            .map(|result| Some(result.map(|(stream, _)| stream)))
    }
}

fn fixture() -> (
    IPCService,
    Probe,
    tokio::task::JoinHandle<Result<(), tonic::transport::Error>>,
) {
    let mut service = IPCService::recovery_for_test(false);
    let probe = Probe {
        state: Arc::new(Mutex::new(State {
            session: 1,
            revision: 0,
            applied: Vec::new(),
            requests: Vec::new(),
            legacy: Vec::new(),
        })),
        legacy_only: Arc::default(),
        fail_next: Arc::default(),
    };
    let listener = service
        .runtime
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = service.runtime.spawn(
        tonic::transport::Server::builder()
            .add_service(probe.clone())
            .serve_with_incoming(Incoming(listener)),
    );
    let channel = service
        .runtime
        .block_on(
            Endpoint::from_shared(format!("http://{address}"))
                .unwrap()
                .connect(),
        )
        .unwrap();
    service.window_client = Some(WindowServiceClient::new(channel.clone()));
    service.window_reconnect_channel_for_test = Some(channel);
    (service, probe, server)
}

fn full(candidates: &[String], index: i32) -> CandidateWindowUpdate<'_> {
    CandidateWindowUpdate {
        visible: Some(true),
        position: Some(shared::proto::WindowPosition {
            top: 1,
            left: 2,
            bottom: 3,
            right: 4,
        }),
        candidates: Some(candidates),
        selected_index: Some(index),
        reading: Some("かな"),
        candidate_list_visible: Some(false),
        reading_vertical_adjustment: Some(4),
        ..Default::default()
    }
}

fn initial(service: &mut IPCService, candidates: &[String]) -> CandidateWindowRevision {
    service
        .update_candidate_window_scoped(
            full(candidates, 0),
            CandidateWindowUpdateKind::Unconditional,
            || true,
            || None,
        )
        .unwrap()
        .candidate_revision()
        .unwrap()
}

fn selection(
    service: &mut IPCService,
    candidates: &[String],
    revision: Option<CandidateWindowRevision>,
    index: i32,
) -> WindowRpcDelivery {
    service
        .update_candidate_window_scoped(
            CandidateWindowUpdate {
                selected_index: Some(index),
                reading: Some("かな"),
                ..Default::default()
            },
            CandidateWindowUpdateKind::Selection(revision),
            || true,
            || Some(full(candidates, index).to_request()),
        )
        .unwrap()
}

#[test]
fn candidate_window_selection_omits_rows_until_restart_or_another_owner_requires_full_state() {
    let (mut service, probe, server) = fixture();
    let candidates = vec!["かな".into(), "仮名".into(), "カナ".into()];
    let revision = initial(&mut service, &candidates);
    for index in [1, 2, 0] {
        let repairs = Cell::new(0);
        let sent = service
            .update_candidate_window_scoped(
                CandidateWindowUpdate {
                    selected_index: Some(index),
                    ..Default::default()
                },
                CandidateWindowUpdateKind::Selection(Some(revision)),
                || true,
                || {
                    repairs.set(repairs.get() + 1);
                    Some(full(&candidates, index).to_request())
                },
            )
            .unwrap();
        assert_eq!(sent.candidate_revision(), Some(revision));
        assert_eq!(
            repairs.get(),
            0,
            "normal selection must not materialize a full repair payload"
        );
    }
    {
        let mut state = probe.state.lock().unwrap();
        assert!(state.applied[0].candidates.is_some());
        assert!(state.applied[1..]
            .iter()
            .all(|request| request.candidates.is_none()));
        state.session += 1;
        state.revision = 0;
        state.applied.clear();
    }
    let repaired = selection(&mut service, &candidates, Some(revision), 1)
        .candidate_revision()
        .unwrap();
    assert_ne!(repaired.server_session_id, revision.server_session_id);
    {
        let mut state = probe.state.lock().unwrap();
        assert_eq!(
            state.applied.len(),
            1,
            "restarted UI must not apply the stale delta"
        );
        assert_eq!(
            state.applied[0].candidates.as_ref().unwrap().candidates,
            candidates
        );
        assert_eq!(state.applied[0].candidate_list_visible, Some(false));
        assert_eq!(state.applied[0].visible, Some(true));
        assert_eq!(state.applied[0].reading.as_deref(), Some("かな"));
        state.revision += 1; // A different text service replaced the current rows.
        state.applied.clear();
    }
    assert!(selection(&mut service, &candidates, Some(repaired), 2).was_sent());
    assert_eq!(probe.state.lock().unwrap().applied.len(), 1);
    server.abort();
}

#[test]
fn candidate_window_downgrade_never_applies_a_delta_to_the_legacy_rpc() {
    let (mut service, probe, server) = fixture();
    let candidates = vec!["かな".into(), "仮名".into()];
    let revision = initial(&mut service, &candidates);
    probe.legacy_only.store(true, Ordering::Release);
    let sent = selection(&mut service, &candidates, Some(revision), 1);
    assert!(sent.was_sent());
    assert_eq!(sent.candidate_revision(), None);
    let requests = probe.state.lock().unwrap().requests.len();
    assert!(selection(&mut service, &candidates, None, 0).was_sent());
    let state = probe.state.lock().unwrap();
    assert_eq!(
        state.requests.len(),
        requests,
        "known legacy UI uses one full legacy call"
    );
    assert_eq!(state.legacy.len(), 2);
    assert!(state
        .legacy
        .iter()
        .all(|request| request.candidates.as_ref().unwrap().candidates == candidates));
    server.abort();
}

#[test]
fn candidate_window_failed_delta_has_one_absolute_repair_after_reconnect() {
    let (mut service, probe, server) = fixture();
    let candidates = vec!["かな".into(), "仮名".into()];
    let revision = initial(&mut service, &candidates);
    probe.fail_next.store(true, Ordering::Release);
    assert!(selection(&mut service, &candidates, Some(revision), 1).was_sent());
    let state = probe.state.lock().unwrap();
    assert_eq!(state.requests.len(), 3);
    assert!(state.requests[1]
        .update
        .as_ref()
        .unwrap()
        .candidates
        .is_none());
    assert!(state.requests[2]
        .update
        .as_ref()
        .unwrap()
        .candidates
        .is_some());
    assert_eq!(state.applied.len(), 2);
    server.abort();
}

#[test]
fn candidate_window_scope_change_during_repair_does_not_resurrect_old_composition() {
    let (mut service, probe, server) = fixture();
    let candidates = vec!["かな".into()];
    let revision = initial(&mut service, &candidates);
    probe.state.lock().unwrap().session += 1;
    probe.state.lock().unwrap().applied.clear();
    let current = Cell::new(true);
    let repairs = AtomicUsize::new(0);
    let sent = service
        .update_candidate_window_scoped(
            CandidateWindowUpdate {
                selected_index: Some(0),
                ..Default::default()
            },
            CandidateWindowUpdateKind::Selection(Some(revision)),
            || current.get(),
            || {
                repairs.fetch_add(1, Ordering::Relaxed);
                current.set(false);
                Some(full(&candidates, 0).to_request())
            },
        )
        .unwrap();
    assert_eq!(repairs.load(Ordering::Relaxed), 1);
    assert!(!sent.was_sent());
    let state = probe.state.lock().unwrap();
    assert_eq!(state.requests.len(), 2);
    assert!(state.applied.is_empty());
    server.abort();
}
