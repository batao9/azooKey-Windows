use anyhow::Result;
use hyper_util::rt::TokioIo;
use shared::proto::{
    azookey_service_client::AzookeyServiceClient, window_service_client::WindowServiceClient,
};
use std::{sync::Arc, time::Duration};
use tokio::{net::windows::named_pipe::ClientOptions, time};
use tonic::transport::Endpoint;
use tower::service_fn;
use windows::Win32::Foundation::ERROR_PIPE_BUSY;

const INPUT_STYLE_ROMAN2KANA: i32 = 0;
const INPUT_STYLE_DIRECT: i32 = 1;

// connect to kkc server
#[derive(Debug, Clone)]
pub struct IPCService {
    // kkc server client
    azookey_client: AzookeyServiceClient<tonic::transport::channel::Channel>,
    // candidate window server client
    window_client: WindowServiceClient<tonic::transport::channel::Channel>,
    runtime: Arc<tokio::runtime::Runtime>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClauseHint {
    pub text: String,
    pub raw_hiragana: String,
    pub corresponding_count: i32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Candidates {
    pub texts: Vec<String>,
    pub sub_texts: Vec<String>,
    pub hiragana: String,
    pub corresponding_count: Vec<i32>,
    pub clauses: Vec<ClauseHint>,
}

impl IPCService {
    pub fn new() -> Result<Self> {
        let runtime = tokio::runtime::Runtime::new()?;

        let server_channel = runtime.block_on(
            Endpoint::try_from("http://[::]:50051")?.connect_with_connector(service_fn(
                |_| async {
                    let client = loop {
                        match ClientOptions::new().open(r"\\.\pipe\azookey_server") {
                            Ok(client) => break client,
                            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY.0 as i32) => (),
                            Err(e) => return Err(e),
                        }

                        time::sleep(Duration::from_millis(50)).await;
                    };

                    Ok::<_, std::io::Error>(TokioIo::new(client))
                },
            )),
        )?;

        let ui_channel = runtime.block_on(
            Endpoint::try_from("http://[::]:50052")?.connect_with_connector(service_fn(
                |_| async {
                    let client = loop {
                        match ClientOptions::new().open(r"\\.\pipe\azookey_ui") {
                            Ok(client) => break client,
                            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY.0 as i32) => (),
                            Err(e) => return Err(e),
                        }

                        time::sleep(Duration::from_millis(50)).await;
                    };

                    Ok::<_, std::io::Error>(TokioIo::new(client))
                },
            )),
        )?;

        let azookey_client = AzookeyServiceClient::new(server_channel);
        let window_client = WindowServiceClient::new(ui_channel);
        tracing::debug!("Connected to server: {:?}", azookey_client);

        Ok(Self {
            azookey_client,
            window_client,
            runtime: Arc::new(runtime),
        })
    }
}

// implement methods to interact with kkc server
impl IPCService {
    fn candidates_from_composing_text(
        composing_text: Option<shared::proto::ComposingText>,
    ) -> anyhow::Result<Candidates> {
        if let Some(composing_text) = composing_text {
            Ok(Candidates {
                texts: composing_text
                    .suggestions
                    .iter()
                    .map(|s| s.text.clone())
                    .collect(),
                sub_texts: composing_text
                    .suggestions
                    .iter()
                    .map(|s| s.subtext.clone())
                    .collect(),
                hiragana: composing_text.hiragana,
                corresponding_count: composing_text
                    .suggestions
                    .iter()
                    .map(|s| s.corresponding_count)
                    .collect(),
                clauses: composing_text
                    .clauses
                    .into_iter()
                    .map(|clause| ClauseHint {
                        text: clause.text,
                        raw_hiragana: clause.raw_hiragana,
                        corresponding_count: clause.corresponding_count,
                    })
                    .collect(),
            })
        } else {
            anyhow::bail!("composing_text is None");
        }
    }

    fn reconnect(&mut self) -> anyhow::Result<()> {
        let refreshed = Self::new()?;
        self.azookey_client = refreshed.azookey_client;
        self.window_client = refreshed.window_client;
        self.runtime = refreshed.runtime;
        Ok(())
    }

    #[tracing::instrument]
    pub fn append_text(&mut self, text: String) -> anyhow::Result<Candidates> {
        self.append_text_with_style(text, INPUT_STYLE_ROMAN2KANA)
    }

    #[tracing::instrument]
    pub fn append_text_with_context(
        &mut self,
        text: String,
        previous_candidates: &Candidates,
    ) -> anyhow::Result<Candidates> {
        self.append_text_with_style_and_context(
            text,
            INPUT_STYLE_ROMAN2KANA,
            Some(previous_candidates),
        )
    }

    #[tracing::instrument]
    pub fn append_text_direct(&mut self, text: String) -> anyhow::Result<Candidates> {
        self.append_text_with_style(text, INPUT_STYLE_DIRECT)
    }

    #[tracing::instrument]
    pub fn append_text_direct_with_context(
        &mut self,
        text: String,
        previous_candidates: &Candidates,
    ) -> anyhow::Result<Candidates> {
        self.append_text_with_style_and_context(text, INPUT_STYLE_DIRECT, Some(previous_candidates))
    }

    #[tracing::instrument]
    fn append_text_with_style(
        &mut self,
        text: String,
        input_style: i32,
    ) -> anyhow::Result<Candidates> {
        self.append_text_with_style_and_context(text, input_style, None)
    }

    #[inline]
    fn should_retry_append_after_refresh(
        previous_candidates: Option<&Candidates>,
        refreshed_candidates: &Candidates,
    ) -> bool {
        previous_candidates.is_some_and(|previous| previous == refreshed_candidates)
    }

    #[tracing::instrument]
    fn append_text_with_style_and_context(
        &mut self,
        text: String,
        input_style: i32,
        previous_candidates: Option<&Candidates>,
    ) -> anyhow::Result<Candidates> {
        let send = |this: &mut Self| -> anyhow::Result<
            tonic::Response<shared::proto::AppendTextResponse>,
        > {
            let request = tonic::Request::new(shared::proto::AppendTextRequest {
                text_to_append: text.clone(),
                input_style,
            });

            let response = this
                .runtime
                .clone()
                .block_on(this.azookey_client.append_text(request))?;
            Ok(response)
        };

        let response = match send(self) {
            Ok(response) => response,
            Err(first_error) => {
                tracing::warn!(
                    "append_text first attempt failed (style={input_style}, text_len={}), reconnecting IPC: {first_error:?}",
                    text.chars().count()
                );

                match self.reconnect() {
                    Ok(()) => {
                        tracing::info!(
                            "append_text IPC reconnect succeeded (style={input_style}), refreshing current composition"
                        );
                    }
                    Err(reconnect_error) => {
                        tracing::error!(
                            "append_text IPC reconnect failed (style={input_style}): {reconnect_error:?}"
                        );
                        return Err(reconnect_error);
                    }
                }

                match self.move_cursor(0) {
                    Ok(candidates) => {
                        if Self::should_retry_append_after_refresh(previous_candidates, &candidates)
                        {
                            tracing::warn!(
                                "append_text recovered unchanged composition after reconnect (style={input_style}), retrying original input"
                            );
                            let retry_response = send(self)?;
                            return Self::candidates_from_composing_text(
                                retry_response.into_inner().composing_text,
                            );
                        }

                        tracing::info!(
                            "append_text recovered changed composition after reconnect (style={input_style}), reusing server state"
                        );
                        return Ok(candidates);
                    }
                    Err(refresh_error) => {
                        tracing::error!(
                            "append_text refresh failed after reconnect (style={input_style}): {refresh_error:?}"
                        );
                        return Err(refresh_error);
                    }
                }
            }
        };
        Self::candidates_from_composing_text(response.into_inner().composing_text)
    }

    #[tracing::instrument]
    pub fn remove_text(&mut self) -> anyhow::Result<Candidates> {
        let request = tonic::Request::new(shared::proto::RemoveTextRequest {});
        let response = self
            .runtime
            .clone()
            .block_on(self.azookey_client.remove_text(request))?;
        Self::candidates_from_composing_text(response.into_inner().composing_text)
    }

    #[tracing::instrument]
    pub fn clear_text(&mut self) -> anyhow::Result<()> {
        let request = tonic::Request::new(shared::proto::ClearTextRequest {});
        let _response = self
            .runtime
            .clone()
            .block_on(self.azookey_client.clear_text(request))?;

        Ok(())
    }

    #[tracing::instrument]
    pub fn shrink_text(&mut self, offset: i32) -> anyhow::Result<Candidates> {
        let request = tonic::Request::new(shared::proto::ShrinkTextRequest { offset });
        let response = self
            .runtime
            .clone()
            .block_on(self.azookey_client.shrink_text(request))?;
        Self::candidates_from_composing_text(response.into_inner().composing_text)
    }

    #[tracing::instrument]
    pub fn move_cursor(&mut self, offset: i32) -> anyhow::Result<Candidates> {
        let request = tonic::Request::new(shared::proto::MoveCursorRequest { offset });
        let response = self
            .runtime
            .clone()
            .block_on(self.azookey_client.move_cursor(request))?;
        Self::candidates_from_composing_text(response.into_inner().composing_text)
    }

    pub fn set_context(&mut self, context: String) -> anyhow::Result<()> {
        let request = tonic::Request::new(shared::proto::SetContextRequest { context });
        let _response = self
            .runtime
            .clone()
            .block_on(self.azookey_client.set_context(request))?;

        Ok(())
    }
}

// implement methods to interact with candidate window server
impl IPCService {
    #[tracing::instrument]
    pub fn show_window(&mut self) -> anyhow::Result<()> {
        let request = tonic::Request::new(shared::proto::EmptyResponse {});
        self.runtime
            .clone()
            .block_on(self.window_client.show_window(request))?;

        Ok(())
    }

    #[tracing::instrument]
    pub fn hide_window(&mut self) -> anyhow::Result<()> {
        let request = tonic::Request::new(shared::proto::EmptyResponse {});
        self.runtime
            .clone()
            .block_on(self.window_client.hide_window(request))?;

        Ok(())
    }

    #[tracing::instrument]
    pub fn set_window_position(
        &mut self,
        top: i32,
        left: i32,
        bottom: i32,
        right: i32,
    ) -> anyhow::Result<()> {
        let request = tonic::Request::new(shared::proto::SetPositionRequest {
            position: Some(shared::proto::WindowPosition {
                top,
                left,
                bottom,
                right,
            }),
        });
        self.runtime
            .clone()
            .block_on(self.window_client.set_window_position(request))?;

        Ok(())
    }

    #[tracing::instrument]
    pub fn set_candidates(&mut self, candidates: Vec<String>) -> anyhow::Result<()> {
        let request = tonic::Request::new(shared::proto::SetCandidateRequest { candidates });
        self.runtime
            .clone()
            .block_on(self.window_client.set_candidate(request))?;

        Ok(())
    }

    #[tracing::instrument]
    pub fn set_selection(&mut self, index: i32) -> anyhow::Result<()> {
        let request = tonic::Request::new(shared::proto::SetSelectionRequest { index });
        self.runtime
            .clone()
            .block_on(self.window_client.set_selection(request))?;

        Ok(())
    }

    #[tracing::instrument]
    pub fn set_input_mode(&mut self, mode: &str) -> anyhow::Result<()> {
        let request = tonic::Request::new(shared::proto::SetInputModeRequest {
            mode: mode.to_string(),
        });
        self.runtime
            .clone()
            .block_on(self.window_client.set_input_mode(request))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Candidates, IPCService};

    #[test]
    fn append_retry_is_enabled_when_server_state_is_unchanged() {
        let previous = Candidates {
            texts: vec!["か".to_string()],
            sub_texts: vec![String::new()],
            hiragana: "か".to_string(),
            corresponding_count: vec![1],
            clauses: Vec::new(),
        };

        assert!(IPCService::should_retry_append_after_refresh(
            Some(&previous),
            &previous
        ));
    }

    #[test]
    fn append_retry_is_disabled_when_server_state_has_changed() {
        let previous = Candidates::default();
        let refreshed = Candidates {
            texts: vec!["か".to_string()],
            sub_texts: vec![String::new()],
            hiragana: "か".to_string(),
            corresponding_count: vec![1],
            clauses: Vec::new(),
        };

        assert!(!IPCService::should_retry_append_after_refresh(
            Some(&previous),
            &refreshed
        ));
    }
}
