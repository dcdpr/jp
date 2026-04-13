use std::io::{BufReader, Cursor};

use jp_plugin::message::*;
use serde_json::json;

use super::*;

/// Helper to build a host response line.
fn host_line(msg: &HostToPlugin) -> String {
    serde_json::to_string(msg).unwrap()
}

fn shared_writer() -> SharedWriter {
    Arc::new(Mutex::new(Box::new(Vec::<u8>::new())))
}

#[tokio::test]
async fn list_conversations_roundtrip() {
    let response = HostToPlugin::Conversations(ConversationsResponse {
        id: Some("1".to_owned()),
        data: vec![ConversationSummary {
            id: "123".to_owned(),
            title: Some("Test".to_owned()),
            last_activated_at: chrono::Utc::now(),
            events_count: 5,
        }],
    });

    let stdin_data = format!("{}\n", host_line(&response));
    let stdin = BufReader::new(Cursor::new(stdin_data));
    let (client, _shutdown) = PluginClient::start(stdin, shared_writer());
    let result = client.list_conversations().await.unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].id, "123");
    assert_eq!(result[0].title.as_deref(), Some("Test"));
}

#[tokio::test]
async fn read_events_roundtrip() {
    let response = HostToPlugin::Events(EventsResponse {
        id: Some("1".to_owned()),
        conversation: "456".to_owned(),
        data: vec![json!({"type": "turn_start", "timestamp": "2025-01-01T00:00:00Z"})],
    });

    let stdin_data = format!("{}\n", host_line(&response));
    let stdin = BufReader::new(Cursor::new(stdin_data));
    let (client, _shutdown) = PluginClient::start(stdin, shared_writer());
    let result = client.read_events("456").await.unwrap();

    assert_eq!(result.conversation, "456");
    assert_eq!(result.data.len(), 1);
}

#[tokio::test]
async fn host_error_propagated() {
    let response = HostToPlugin::Error(ErrorResponse {
        id: Some("1".to_owned()),
        request: Some("list_conversations".to_owned()),
        message: "something went wrong".to_owned(),
    });

    let stdin_data = format!("{}\n", host_line(&response));
    let stdin = BufReader::new(Cursor::new(stdin_data));
    let (client, _shutdown) = PluginClient::start(stdin, shared_writer());
    let err = client.list_conversations().await.unwrap_err();

    assert!(matches!(err, ClientError::Host(msg) if msg.contains("something went wrong")));
}

#[test]
fn shutdown_signals_watch() {
    let shutdown_msg = HostToPlugin::Shutdown;
    let stdin_data = format!("{}\n", host_line(&shutdown_msg));
    let stdin = BufReader::new(Cursor::new(stdin_data));
    let (_client, shutdown_rx) = PluginClient::start(stdin, shared_writer());

    // Give the reader thread a moment to process.
    std::thread::sleep(std::time::Duration::from_millis(50));

    assert!(*shutdown_rx.borrow());
}
