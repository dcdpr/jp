use jp_test::mock::{MockServer, POST};
use serde_json::json;

use super::*;

fn client() -> LoopbackClient {
    LoopbackClient(Client::builder().no_proxy().build().unwrap())
}

fn ping() -> ClientJsonRpcMessage {
    serde_json::from_value(json!({"jsonrpc": "2.0", "id": 1, "method": "ping"})).unwrap()
}

fn initialized() -> ClientJsonRpcMessage {
    serde_json::from_value(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        .unwrap()
}

async fn post(
    server: &MockServer,
    message: ClientJsonRpcMessage,
    session_id: Option<&str>,
) -> Result<StreamableHttpPostResponse, Error> {
    client()
        .post_message(
            server.url("/mcp").into(),
            message,
            session_id.map(Into::into),
            None,
            HashMap::new(),
        )
        .await
}

/// A `404` for a request that carried a session means the server dropped the
/// session, which rmcp's worker handles differently from a missing endpoint.
#[tokio::test]
async fn a_404_with_a_session_is_an_expired_session() {
    let server = MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(POST).path("/mcp");
            then.status(404);
        })
        .await;

    assert!(matches!(
        post(&server, ping(), Some("session-1")).await,
        Err(StreamableHttpError::SessionExpired)
    ));
    assert!(matches!(
        post(&server, ping(), None).await,
        Err(StreamableHttpError::UnexpectedServerResponse(_))
    ));
}

/// A JSON-RPC error on a failure status reaches the caller as an MCP error.
#[tokio::test]
async fn a_json_rpc_error_on_a_failure_status_is_returned_as_a_message() {
    let server = MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(POST).path("/mcp");
            then.status(400)
                .header("content-type", "application/json")
                .json_body(json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "error": {"code": -32600, "message": "Invalid Request"}
                }));
        })
        .await;

    let result = post(&server, ping(), Some("session-1")).await;
    assert!(
        matches!(
            result,
            Ok(StreamableHttpPostResponse::Json(
                JsonRpcMessage::Error(_),
                _
            ))
        ),
        "{result:?}"
    );
}

/// An empty `200` answers a notification the same way a `202` does.
#[tokio::test]
async fn an_empty_200_to_a_notification_is_accepted() {
    let server = MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/mcp")
                .header("mcp-session-id", "session-1")
                .header("accept", "text/event-stream, application/json");
            then.status(200);
        })
        .await;

    assert!(matches!(
        post(&server, initialized(), Some("session-1")).await,
        Ok(StreamableHttpPostResponse::Accepted)
    ));
}
