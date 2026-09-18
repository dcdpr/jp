//! Wire-level assertions for the bearer (OAuth) authentication mode.

use async_anthropic::{Client, bearer, types::ListModelsResponse};
use wiremock::{
    Mock, MockServer, Request, ResponseTemplate,
    matchers::{method, path},
};

fn empty_models_response() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(&ListModelsResponse {
        data: vec![],
        first_id: None,
        has_more: false,
        last_id: None,
    })
}

async fn capture_models_request(client: &Client, server: &MockServer) -> Request {
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(empty_models_response())
        .expect(1)
        .mount(server)
        .await;

    client.models().list().await.unwrap();

    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap()
}

#[tokio::test]
async fn test_bearer_mode_headers() {
    let server = MockServer::start().await;
    let client = Client::builder()
        .auth_token("oauth-access-token")
        .base_url(server.uri())
        .version("2023-06-01")
        .build()
        .unwrap();

    let request = capture_models_request(&client, &server).await;
    let header = |name: &str| {
        request
            .headers
            .get(name)
            .map(|v| v.to_str().unwrap().to_owned())
    };

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
    assert!(header("x-client-request-id").is_some_and(|v| !v.is_empty()));
}

#[tokio::test]
async fn test_bearer_mode_merges_user_betas_without_duplicates() {
    let server = MockServer::start().await;
    let client = Client::builder()
        .auth_token("oauth-access-token")
        // One duplicate of a fingerprint beta, one genuine extra.
        .beta("interleaved-thinking-2025-05-14,structured-outputs-2025-10-27")
        .base_url(server.uri())
        .version("2023-06-01")
        .build()
        .unwrap();

    let request = capture_models_request(&client, &server).await;
    let betas = request
        .headers
        .get("anthropic-beta")
        .unwrap()
        .to_str()
        .unwrap();

    assert_eq!(
        betas,
        "oauth-2025-04-20,claude-code-20250219,interleaved-thinking-2025-05-14,\
         context-management-2025-06-27,structured-outputs-2025-10-27"
    );
}

#[tokio::test]
async fn test_api_key_mode_sends_no_fingerprint() {
    let server = MockServer::start().await;
    let client = Client::builder()
        .api_key("test-api-key")
        .base_url(server.uri())
        .version("2023-06-01")
        .build()
        .unwrap();

    let request = capture_models_request(&client, &server).await;

    assert_eq!(
        request.headers.get("x-api-key").unwrap().to_str().unwrap(),
        "test-api-key"
    );
    assert!(request.headers.get("authorization").is_none());
    assert!(request.headers.get("x-app").is_none());
    assert!(request.headers.get("anthropic-client-platform").is_none());
    assert!(request.headers.get("x-client-request-id").is_none());
    assert!(request.headers.get("anthropic-beta").is_none());
}

#[test]
fn test_merge_betas_with_no_extras() {
    assert_eq!(
        bearer::merge_betas(None),
        "oauth-2025-04-20,claude-code-20250219,interleaved-thinking-2025-05-14,\
         context-management-2025-06-27"
    );
}
