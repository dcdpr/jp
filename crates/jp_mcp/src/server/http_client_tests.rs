use axum::{Router, body::Body, extract::Request, http::Response, routing::any};
use reqwest::StatusCode;
use serde_json::json;
use tokio::{net::TcpListener, task::JoinHandle};

use super::*;

async fn fixture(
    status: StatusCode,
    content_type: &'static str,
    body: &'static str,
) -> (Arc<str>, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url: Arc<str> = format!("http://{}/mcp", listener.local_addr().unwrap()).into();
    let router = Router::new().route(
        "/mcp",
        any(move |request: Request| async move {
            assert_eq!(request.headers()["mcp-session-id"], "session-1");
            assert_eq!(request.headers()["mcp-protocol-version"], "2025-11-25");
            Response::builder()
                .status(status)
                .header("content-type", content_type)
                .body(Body::from(body))
                .unwrap()
        }),
    );
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (url, task)
}

fn headers() -> HashMap<HeaderName, HeaderValue> {
    HashMap::from([(
        HeaderName::from_static("mcp-protocol-version"),
        HeaderValue::from_static("2025-11-25"),
    )])
}

#[tokio::test]
async fn post_decodes_json_response() {
    let (url, server) = fixture(
        StatusCode::OK,
        "application/json; charset=utf-8",
        r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}"#,
    )
    .await;
    let message =
        serde_json::from_value(json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}))
            .unwrap();
    let result = LoopbackClient::new()
        .unwrap()
        .post_message(url, message, Some("session-1".into()), None, headers())
        .await
        .unwrap();
    let StreamableHttpPostResponse::Json(message, session) = result else {
        panic!("expected JSON response")
    };
    assert_eq!(session, None);
    assert_eq!(
        serde_json::to_value(message).unwrap(),
        json!({"jsonrpc":"2.0","id":1,"result":{"tools":[]}})
    );
    server.abort();
}

#[tokio::test]
async fn expired_session_is_not_a_new_request() {
    let (url, server) = fixture(StatusCode::NOT_FOUND, "text/plain", "expired").await;
    let message =
        serde_json::from_value(json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}))
            .unwrap();
    let result = LoopbackClient::new()
        .unwrap()
        .post_message(url, message, Some("session-1".into()), None, headers())
        .await;
    assert!(matches!(result, Err(StreamableHttpError::SessionExpired)));
    server.abort();
}

#[tokio::test]
async fn unsupported_stream_is_explicit() {
    let (url, server) = fixture(StatusCode::METHOD_NOT_ALLOWED, "text/plain", "unsupported").await;
    let result = LoopbackClient::new()
        .unwrap()
        .get_stream(url, "session-1".into(), None, None, headers())
        .await;
    assert!(matches!(
        result,
        Err(StreamableHttpError::ServerDoesNotSupportSse)
    ));
    server.abort();
}

#[tokio::test]
async fn unsupported_deletion_is_explicit() {
    let (url, server) = fixture(StatusCode::METHOD_NOT_ALLOWED, "text/plain", "unsupported").await;
    let result = LoopbackClient::new()
        .unwrap()
        .delete_session(url, "session-1".into(), None, headers())
        .await;
    assert!(matches!(
        result,
        Err(StreamableHttpError::ServerDoesNotSupportDeleteSession)
    ));
    server.abort();
}

#[tokio::test]
async fn get_stream_preserves_event_ids_and_data() {
    let (url, server) = fixture(
        StatusCode::OK,
        "text/event-stream",
        "id: event-1\nevent: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n\n",
    )
    .await;
    let mut stream = LoopbackClient::new()
        .unwrap()
        .get_stream(url, "session-1".into(), None, None, headers())
        .await
        .unwrap();
    let event = stream.next().await.unwrap().unwrap();
    assert_eq!(event.id.as_deref(), Some("event-1"));
    assert_eq!(
        event.data.as_deref(),
        Some(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#)
    );
    assert!(stream.next().await.is_none());
    server.abort();
}

#[test]
fn caller_headers_cannot_override_the_session() {
    let request = Client::new().get("http://127.0.0.1/mcp");
    let headers = HashMap::from([(
        HeaderName::from_static("mcp-session-id"),
        HeaderValue::from_static("wrong-session"),
    )]);
    let result = request_headers(request, Some("session-1"), None, headers);
    assert!(
        matches!(result, Err(StreamableHttpError::ReservedHeaderConflict(name)) if name == "mcp-session-id")
    );
}

#[tokio::test]
async fn unexpected_content_type_is_rejected() {
    let (url, server) = fixture(StatusCode::OK, "text/html", "not MCP").await;
    let result = LoopbackClient::new()
        .unwrap()
        .get_stream(url, "session-1".into(), None, None, headers())
        .await;
    assert!(
        matches!(result, Err(StreamableHttpError::UnexpectedContentType(Some(value))) if value == "text/html")
    );
    server.abort();
}
