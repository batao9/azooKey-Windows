const MODELS_ENDPOINT: &str = "https://api.typesafe.ai/v1/models";
const MAX_RESPONSE_BYTES: usize = 64 * 1024;
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

pub async fn check_api_key(api_key: &str) -> Result<(), String> {
    let client = api_client()?;
    check_api_key_with_client(&client, MODELS_ENDPOINT, api_key).await
}

fn api_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(TIMEOUT)
        .build()
        .map_err(|_| "APIキーの確認に失敗しました。".to_string())
}

async fn check_api_key_with_client(
    client: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
) -> Result<(), String> {
    let response = client
        .get(endpoint)
        .bearer_auth(api_key)
        .send()
        .await
        .map_err(|error| {
            if error.is_timeout() {
                "APIキーの確認がタイムアウトしました。"
            } else {
                "APIキーの確認中にネットワークエラーが発生しました。"
            }
            .to_string()
        })?;

    let status = response.status();
    if status.as_u16() == 401 {
        return Err("APIキーが無効です。".to_string());
    }
    if status.as_u16() == 403 {
        return Err("このAPIキーではアクセスできません。".to_string());
    }
    if status.as_u16() == 429 {
        return Err("アクセスが集中しています。しばらくしてから再試行してください。".to_string());
    }
    if status.is_server_error() {
        return Err("サービスが一時的に利用できません。".to_string());
    }
    if status.as_u16() != 200 {
        return Err(format!(
            "APIキーの確認に失敗しました（HTTP {}）。",
            status.as_u16()
        ));
    }

    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err("API応答の形式が正しくありません。".to_string());
    }

    let mut body = Vec::new();
    let mut response = response;
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        if error.is_timeout() {
            "APIキーの確認がタイムアウトしました。"
        } else {
            "APIキーの確認中にネットワークエラーが発生しました。"
        }
        .to_string()
    })? {
        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err("API応答の形式が正しくありません。".to_string());
        }
        body.extend_from_slice(&chunk);
    }

    let value: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|_| "API応答の形式が正しくありません。".to_string())?;
    let models = value
        .get("models")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "API応答の形式が正しくありません。".to_string())?;
    if models.iter().all(|model| {
        model
            .get("name")
            .and_then(serde_json::Value::as_str)
            .is_some()
    }) {
        Ok(())
    } else {
        Err("API応答の形式が正しくありません。".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn serve(response: &'static [u8]) -> (String, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/v1/models", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 4096];
            let size = stream.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..size]).into_owned();
            let _ = stream.write_all(response).await;
            request
        });
        (endpoint, task)
    }

    fn run(future: impl std::future::Future<Output = ()>) {
        tokio::runtime::Runtime::new().unwrap().block_on(future);
    }

    #[test]
    fn accepts_models_and_sends_only_get_with_bearer_auth() {
        run(async {
            let (endpoint, task) = serve(
                b"HTTP/1.1 200 OK\r\nContent-Length: 27\r\nConnection: close\r\n\r\n{\"models\":[{\"name\":\"jev\"}]}",
            )
            .await;
            let client = reqwest::Client::new();
            assert!(check_api_key_with_client(&client, &endpoint, "dummy-key")
                .await
                .is_ok());
            let request = task.await.unwrap();
            assert!(request.starts_with("GET /v1/models HTTP/1.1"));
            assert!(request
                .to_ascii_lowercase()
                .contains("authorization: bearer dummy-key"));
        });
    }

    #[test]
    fn maps_auth_and_service_statuses_without_returning_remote_body() {
        run(async {
            for (status, expected) in [
                ("401 Unauthorized", "APIキーが無効です。"),
                ("403 Forbidden", "このAPIキーではアクセスできません。"),
                (
                    "429 Too Many Requests",
                    "アクセスが集中しています。しばらくしてから再試行してください。",
                ),
                (
                    "503 Service Unavailable",
                    "サービスが一時的に利用できません。",
                ),
            ] {
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: 11\r\nConnection: close\r\n\r\nsecret-body"
                );
                let (endpoint, task) =
                    serve(Box::leak(response.into_bytes().into_boxed_slice())).await;
                let error = check_api_key_with_client(&reqwest::Client::new(), &endpoint, "dummy")
                    .await
                    .unwrap_err();
                assert_eq!(error, expected);
                assert!(!error.contains("secret-body"));
                task.await.unwrap();
            }
        });
    }

    #[test]
    fn rejects_malformed_and_oversized_responses() {
        run(async {
            for body in [
                b"not json".to_vec(),
                format!(
                    "{{\"models\":[{{\"name\":\"{}\"}}]}}",
                    "x".repeat(65 * 1024)
                )
                .into_bytes(),
            ] {
                let response = format!(
                    "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n",
                    body.len()
                );
                let response = [response.into_bytes(), body, b"\r\n0\r\n\r\n".to_vec()].concat();
                let (endpoint, task) = serve(Box::leak(response.into_boxed_slice())).await;
                assert!(
                    check_api_key_with_client(&reqwest::Client::new(), &endpoint, "dummy")
                        .await
                        .is_err()
                );
                task.await.unwrap();
            }
        });
    }

    #[test]
    fn does_not_follow_redirects() {
        run(async {
            let (endpoint, task) = serve(
                b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .await;
            let error = check_api_key_with_client(&api_client().unwrap(), &endpoint, "dummy")
                .await
                .unwrap_err();
            assert!(error.contains("HTTP 302"));
            let request = task.await.unwrap();
            assert!(request.starts_with("GET /v1/models HTTP/1.1"));
        });
    }

    #[test]
    fn reports_timeout_and_network_errors_safely() {
        run(async {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_millis(20))
                .build()
                .unwrap();
            // A delayed listener gives reqwest a deterministic timeout.
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!("http://{}/v1/models", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = [0; 512];
                let _ = stream.read(&mut request).await;
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            });
            let error = check_api_key_with_client(&client, &endpoint, "dummy")
                .await
                .unwrap_err();
            assert_eq!(error, "APIキーの確認がタイムアウトしました。");
            server.abort();

            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!("http://{}/v1/models", listener.local_addr().unwrap());
            drop(listener);
            let error = check_api_key_with_client(&reqwest::Client::new(), &endpoint, "dummy")
                .await
                .unwrap_err();
            assert_eq!(error, "APIキーの確認中にネットワークエラーが発生しました。");
        });
    }
}
