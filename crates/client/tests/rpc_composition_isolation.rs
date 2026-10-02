//! Run in an isolated interactive Windows logon with this branch's server:
//! cargo test -p azookey-windows --test rpc_composition_isolation -- --ignored --nocapture
#![cfg(windows)]

use hyper_util::rt::TokioIo;
use shared::proto::{azookey_service_client::AzookeyServiceClient, *};
use std::{os::windows::io::IntoRawHandle, time::Duration};
use tokio::net::windows::named_pipe::NamedPipeClient;
use tonic::{transport::Channel, Code};

async fn client() -> AzookeyServiceClient<Channel> {
    let channel = tonic::transport::Endpoint::from_static("http://[::]:50051")
        .connect_with_connector(tower::service_fn(|_| async {
            let handle = shared::open_named_pipe_client_handle(shared::server_pipe_path()?)?;
            let pipe = unsafe { NamedPipeClient::from_raw_handle(handle.into_raw_handle()) }?;
            Ok::<_, std::io::Error>(TokioIo::new(pipe))
        }))
        .await
        .unwrap();
    AzookeyServiceClient::new(channel)
}

fn append(text: &str) -> AppendTextRequest {
    AppendTextRequest {
        text_to_append: text.into(),
        input_style: 1,
        ..Default::default()
    }
}

#[tokio::test]
#[ignore = "requires an isolated interactive Windows logon and a running azookey-server"]
async fn two_rpc_connections_cannot_read_modify_or_learn_each_others_composition() {
    let mut first = client().await;
    let mut second = client().await;
    first.clear_text(ClearTextRequest::default()).await.unwrap();
    let original = first
        .append_text(append("ひみつ"))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(original.composing_text.as_ref().unwrap().hiragana, "ひみつ");

    let handshake = second.append_text(append("")).await.unwrap().into_inner();
    assert_eq!(handshake.server_session_id, original.server_session_id);
    assert_eq!(handshake.composing_text.unwrap(), ComposingText::default());

    macro_rules! denied {
        ($method:ident, $request:expr) => {
            assert_eq!(
                second.$method($request).await.unwrap_err().code(),
                Code::PermissionDenied,
                stringify!($method)
            );
        };
    }
    denied!(move_cursor, MoveCursorRequest::default());
    denied!(remove_text, RemoveTextRequest::default());
    denied!(clear_text, ClearTextRequest::default());
    denied!(append_text, append("ぬすむ"));
    denied!(replace_composition, ReplaceCompositionRequest::default());
    denied!(
        start_reconversion,
        StartReconversionRequest {
            surface: "秘密".into(),
            ..Default::default()
        }
    );
    denied!(shrink_text, ShrinkTextRequest::default());
    denied!(advance_clause, AdvanceClauseRequest::default());
    denied!(
        prepare_future_clauses,
        PrepareFutureClausesRequest::default()
    );
    denied!(
        adjust_clause_boundary,
        AdjustClauseBoundaryRequest::default()
    );
    denied!(
        update_composition_snapshot,
        UpdateCompositionSnapshotRequest {
            operation: CompositionSnapshotOperation::Clear as i32,
            ..Default::default()
        }
    );
    denied!(set_context, SetContextRequest::default());
    denied!(
        commit_learning_candidate,
        CommitLearningCandidateRequest::default()
    );
    denied!(
        commit_learning_candidates,
        CommitLearningCandidatesRequest::default()
    );

    let preserved = first
        .move_cursor(MoveCursorRequest::default())
        .await
        .unwrap()
        .into_inner();
    assert_eq!(preserved.composing_text.unwrap().hiragana, "ひみつ");
    // Explicit end/focus release hands ownership to another still-live client.
    first.clear_text(ClearTextRequest::default()).await.unwrap();
    let claimed = second
        .append_text(append("べつのひみつ"))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(claimed.composing_text.unwrap().hiragana, "べつのひみつ");
    assert_eq!(
        first
            .clear_text(ClearTextRequest::default())
            .await
            .unwrap_err()
            .code(),
        Code::PermissionDenied
    );

    // Disconnect must erase abandoned state before another connection can read.
    drop(second);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        match first.move_cursor(MoveCursorRequest::default()).await {
            Ok(response) => {
                let text = response.into_inner().composing_text.unwrap();
                assert!(text.hiragana.is_empty());
                break;
            }
            Err(error)
                if error.code() == Code::PermissionDenied
                    && tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(error) => panic!("abandoned owner was not released: {error}"),
        }
    }
    // Absolute-state recovery may claim a new connection, but not read the old one.
    let recovered = first
        .replace_composition(ReplaceCompositionRequest {
            operations: vec![CompositionOperation {
                kind: CompositionOperationKind::Append as i32,
                input_style: 1,
                text: "かいふく".into(),
                ..Default::default()
            }],
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(recovered.composing_text.unwrap().hiragana, "かいふく");
    first.clear_text(ClearTextRequest::default()).await.unwrap();
}
