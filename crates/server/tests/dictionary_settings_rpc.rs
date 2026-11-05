//! Owned child-server fixture, opt-in through scripts/vm.py's build profile.
#![cfg(windows)]

use hyper_util::rt::TokioIo;
use shared::proto::{azookey_service_client::AzookeyServiceClient, *};
use std::{
    fs,
    os::windows::io::{AsRawHandle, IntoRawHandle},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use tokio::net::windows::named_pipe::NamedPipeClient;
use tonic::transport::Channel;
use windows::Win32::{Foundation::HANDLE, System::Pipes::GetNamedPipeServerProcessId};

struct OwnedServer(Child);
impl Drop for OwnedServer {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

async fn client(
    expected_pid: u32,
) -> Result<AzookeyServiceClient<Channel>, tonic::transport::Error> {
    let pipe_path = shared::server_pipe_path().unwrap();
    let channel = tonic::transport::Endpoint::from_static("http://[::]:50051")
        .connect_with_connector(tower::service_fn(move |_| async move {
            let handle = shared::open_named_pipe_client_handle(pipe_path)?;
            let mut server_pid = 0;
            unsafe { GetNamedPipeServerProcessId(HANDLE(handle.as_raw_handle()), &mut server_pid) }
                .map_err(std::io::Error::other)?;
            if server_pid != expected_pid {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "test refuses to connect to an unowned server",
                ));
            }
            let pipe = unsafe { NamedPipeClient::from_raw_handle(handle.into_raw_handle()) }?;
            Ok::<_, std::io::Error>(TokioIo::new(pipe))
        }))
        .await?;
    Ok(AzookeyServiceClient::new(channel))
}

fn spawn_server(exe: &Path, appdata: &Path, log: &Path) -> OwnedServer {
    let output = fs::File::create(log).unwrap();
    OwnedServer(
        Command::new(exe)
            .env("APPDATA", appdata)
            .current_dir(appdata)
            .stdin(Stdio::null())
            .stdout(output.try_clone().unwrap())
            .stderr(output)
            .spawn()
            .unwrap(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires built Swift/Rust payload, four dictionaries and real zenz.gguf beside the executable"]
async fn dictionary_settings_rpc_keeps_input_responsive_and_rejects_missing_resources() {
    assert!(
        is_x86_feature_detected!("avx"),
        "real CPU model requires AVX"
    );
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_azookey-server"));
    let appdata = tempfile::tempdir().unwrap();
    let settings = appdata.path().join("Azookey/settings.json");
    fs::create_dir_all(settings.parent().unwrap()).unwrap();
    let mut config = serde_json::to_value(shared::AppConfig::default()).unwrap();
    config["zenzai"]["enable"] = true.into();
    config["general"]["experimental_mixed_input"] = true.into();
    config["debug"]["server_log_enabled"] = true.into();
    config["debug"]["server_log_level"] = "debug".into();
    fs::write(&settings, serde_json::to_vec(&config).unwrap()).unwrap();
    let log = appdata.path().join("child.log");
    let mut server = spawn_server(&exe, appdata.path(), &log);
    let deadline = Instant::now() + Duration::from_secs(90);
    let mut input = loop {
        assert!(
            server.0.try_wait().unwrap().is_none(),
            "owned server exited; log={}",
            fs::read_to_string(&log).unwrap_or_default()
        );
        let last_error = match client(server.0.id()).await {
            Ok(connection) => break connection,
            Err(error) => format!("{error:?}"),
        };
        assert!(
            Instant::now() < deadline,
            "owned server did not become ready; pipe={}; error={last_error}; stdout={}; serverlog={}",
            shared::server_pipe_path().unwrap(),
            fs::read_to_string(&log).unwrap_or_default(),
            fs::read_to_string(appdata.path().join("Azookey/logs/server.log")).unwrap_or_default()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let original = input
        .append_text(AppendTextRequest {
            text_to_append: "repository".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(original.composing_text.unwrap().hiragana, "repository");
    let mut management = client(server.0.id()).await.unwrap();
    config["general"]["english_dictionary"] = false.into();
    fs::write(&settings, serde_json::to_vec(&config).unwrap()).unwrap();
    let began = Instant::now();
    let update = tokio::spawn(async move {
        management
            .update_config(UpdateConfigRequest { request_id: 100 })
            .await
    });
    let server_log = appdata.path().join("Azookey/logs/server.log");
    loop {
        let messages = fs::read_to_string(&server_log).unwrap_or_default();
        if messages
            .lines()
            .any(|line| line.contains("DictionaryModelPreparation: queued"))
        {
            break;
        }
        assert!(
            !update.is_finished(),
            "configuration did not exercise real model preparation"
        );
        assert!(
            began.elapsed() < Duration::from_secs(15),
            "model job was not queued"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let input_start = Instant::now();
    let response = tokio::time::timeout(
        Duration::from_secs(2),
        input.append_text(AppendTextRequest {
            text_to_append: "x".into(),
            input_style: 1,
            ..Default::default()
        }),
    )
    .await
    .expect("typing was blocked by dictionary/model preparation")
    .unwrap()
    .into_inner();
    assert_eq!(response.raw_input.as_deref(), Some("repositoryx"));
    assert_eq!(response.composing_text.unwrap().hiragana, "repositoryx");
    let input_ms = input_start.elapsed().as_secs_f64() * 1000.0;
    let updated = tokio::time::timeout(Duration::from_secs(30), update)
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .into_inner();
    assert_eq!(updated.server_session_id, original.server_session_id);
    input.clear_text(ClearTextRequest::default()).await.unwrap();
    let next = input
        .append_text(AppendTextRequest {
            text_to_append: "google".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    assert_ne!(next.composing_text.unwrap().hiragana, "google");
    println!(
        "DICTIONARY_SETTINGS_RPC_CONCURRENCY succeeded input_ms={input_ms:.2} settings_ms={:.2}",
        began.elapsed().as_secs_f64() * 1000.0
    );
    let before_invalid = input
        .move_cursor(MoveCursorRequest::default())
        .await
        .unwrap()
        .into_inner();
    fs::write(&settings, b"{ invalid JSON").unwrap();
    let mut management = client(server.0.id()).await.unwrap();
    let failure = management
        .update_config(UpdateConfigRequest { request_id: 101 })
        .await
        .unwrap_err();
    assert_eq!(failure.code(), tonic::Code::FailedPrecondition);
    let after_invalid = input
        .move_cursor(MoveCursorRequest::default())
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        before_invalid
            .composing_text
            .as_ref()
            .map(|text| &text.hiragana),
        after_invalid
            .composing_text
            .as_ref()
            .map(|text| &text.hiragana)
    );
    input.clear_text(ClearTextRequest::default()).await.unwrap();
    let preserved_mode = input
        .append_text(AppendTextRequest {
            text_to_append: "google".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    assert_ne!(preserved_mode.composing_text.unwrap().hiragana, "google");
    println!("DICTIONARY_SETTINGS_INVALID_CONFIG_PRESERVED succeeded");
    drop(input);
    drop(server);
    config["general"]["english_dictionary"] = true.into();
    fs::write(&settings, serde_json::to_vec(&config).unwrap()).unwrap();

    // Repeat real Rust startup in a distribution folder without its resource
    // bundle. It must return an initialization error, not open a ready server.
    let bundle = exe
        .parent()
        .unwrap()
        .join("azookey-server_azookey-server.resources");
    let held = bundle.with_extension(format!("held-{}", std::process::id()));
    assert!(!held.exists());
    fs::rename(&bundle, &held).unwrap();
    struct RestoreBundle(PathBuf, PathBuf);
    impl Drop for RestoreBundle {
        fn drop(&mut self) {
            fs::rename(&self.1, &self.0).unwrap();
        }
    }
    let _restore = RestoreBundle(bundle, held);
    let missing_log = appdata.path().join("missing-resource.log");
    let mut missing = spawn_server(&exe, appdata.path(), &missing_log);
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = missing.0.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "missing-resource server did not exit"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert!(!status.success());
    assert!(fs::read_to_string(missing_log)
        .unwrap()
        .contains("initialization failed"));
    println!("DICTIONARY_SETTINGS_RUST_STARTUP_MISSING_RESOURCE succeeded");
}
