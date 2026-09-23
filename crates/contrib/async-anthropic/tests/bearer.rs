//! Wire-level assertions for the bearer (OAuth) authentication mode.

use std::sync::{Arc, Mutex};

use async_anthropic::{Client, bearer, types::ListModelsResponse};
use httpmock::{HttpMockRequest, HttpMockResponse, MockServer, prelude::GET};
use reqwest::header::HeaderMap;

/// Send one models-list request and return the headers the server received.
async fn capture_models_request(client: &Client, server: &MockServer) -> HeaderMap {
    let captured = Arc::new(Mutex::new(None));
    let body = serde_json::to_string(&ListModelsResponse {
        data: vec![],
        first_id: None,
        has_more: false,
        last_id: None,
    })
    .unwrap();

    let mock = server
        .mock_async({
            let captured = Arc::clone(&captured);
            move |when, then| {
                when.method(GET).path("/v1/models");
                then.respond_with(move |request: &HttpMockRequest| {
                    *captured.lock().unwrap() = Some(request.headers());
                    HttpMockResponse::builder()
                        .status(200)
                        .body(body.clone())
                        .build()
                });
            }
        })
        .await;

    client.models().list().await.unwrap();
    mock.assert_calls_async(1).await;
    // Leave the server free for the next capture on the same path.
    mock.delete_async().await;

    captured.lock().unwrap().take().unwrap()
}

/// Whether `id` has the hyphenated shape of a version 4 UUID.
fn is_uuid_v4(id: &str) -> bool {
    let bytes = id.as_bytes();

    id.len() == 36
        && [8, 13, 18, 23].iter().all(|&i| bytes[i] == b'-')
        && bytes[14] == b'4'
        && matches!(bytes[19], b'8' | b'9' | b'a' | b'b')
        && id
            .chars()
            .all(|c| c == '-' || c.is_ascii_digit() || ('a'..='f').contains(&c))
}

#[tokio::test]
async fn test_bearer_mode_headers() {
    let server = MockServer::start_async().await;
    let client = Client::builder()
        .auth_token("oauth-access-token")
        .base_url(server.base_url())
        .version("2023-06-01")
        .build()
        .unwrap();

    let headers = capture_models_request(&client, &server).await;
    let header = |name: &str| headers.get(name).map(|v| v.to_str().unwrap().to_owned());

    assert_eq!(
        header("authorization").as_deref(),
        Some("Bearer oauth-access-token")
    );
    assert_eq!(header("x-api-key"), None);
    assert_eq!(
        header("anthropic-beta").as_deref(),
        Some(
            "oauth-2025-04-20,claude-code-20250219,interleaved-thinking-2025-05-14,\
             context-management-2025-06-27"
        )
    );
    assert_eq!(header("anthropic-version").as_deref(), Some("2023-06-01"));
    assert_eq!(header("x-app").as_deref(), Some("cli"));
    assert_eq!(
        header("anthropic-client-platform").as_deref(),
        Some("desktop_app")
    );
    assert_eq!(
        header("anthropic-client-version").as_deref(),
        Some("1.11187.4")
    );
    assert_eq!(
        header("user-agent").as_deref(),
        Some("claude-cli/2.1.165 (external, local-agent, agent-sdk/0.3.165)")
    );
    assert_eq!(header("x-stainless-lang").as_deref(), Some("js"));
    assert!(
        header("x-client-request-id").is_some_and(|v| is_uuid_v4(&v)),
        "{:?}",
        header("x-client-request-id")
    );
}

/// Each request carries its own id, as Claude Code's does.
#[tokio::test]
async fn test_bearer_mode_request_ids_differ() {
    let server = MockServer::start_async().await;
    let client = Client::builder()
        .auth_token("oauth-access-token")
        .base_url(server.base_url())
        .build()
        .unwrap();

    let first = capture_models_request(&client, &server).await;
    let second = capture_models_request(&client, &server).await;

    assert_ne!(
        first.get("x-client-request-id"),
        second.get("x-client-request-id")
    );
}

#[tokio::test]
async fn test_bearer_mode_merges_user_betas_without_duplicates() {
    let server = MockServer::start_async().await;
    let client = Client::builder()
        .auth_token("oauth-access-token")
        // One duplicate of a fingerprint beta, one genuine extra.
        .beta("interleaved-thinking-2025-05-14,structured-outputs-2025-10-27")
        .base_url(server.base_url())
        .version("2023-06-01")
        .build()
        .unwrap();

    let headers = capture_models_request(&client, &server).await;
    let betas = headers.get("anthropic-beta").unwrap().to_str().unwrap();

    assert_eq!(
        betas,
        "oauth-2025-04-20,claude-code-20250219,interleaved-thinking-2025-05-14,\
         context-management-2025-06-27,structured-outputs-2025-10-27"
    );
}

#[tokio::test]
async fn test_api_key_mode_sends_no_fingerprint() {
    let server = MockServer::start_async().await;
    let client = Client::builder()
        .api_key("test-api-key")
        .base_url(server.base_url())
        .version("2023-06-01")
        .build()
        .unwrap();

    let headers = capture_models_request(&client, &server).await;

    assert_eq!(
        headers.get("x-api-key").unwrap().to_str().unwrap(),
        "test-api-key"
    );
    assert!(headers.get("authorization").is_none());
    assert!(headers.get("x-app").is_none());
    assert!(headers.get("anthropic-client-platform").is_none());
    assert!(headers.get("x-client-request-id").is_none());
    assert!(headers.get("anthropic-beta").is_none());
}

#[test]
fn test_merge_betas_with_no_extras() {
    assert_eq!(
        bearer::merge_betas(None),
        "oauth-2025-04-20,claude-code-20250219,interleaved-thinking-2025-05-14,\
         context-management-2025-06-27"
    );
}
