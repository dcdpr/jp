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

#[tokio::test]
async fn shutdown_signals_watch() {
    let shutdown_msg = HostToPlugin::Shutdown;
    let stdin_data = format!("{}\n", host_line(&shutdown_msg));
    let stdin = BufReader::new(Cursor::new(stdin_data));
    let (_client, mut shutdown_rx) = PluginClient::start(stdin, shared_writer());

    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        shutdown_rx.wait_for(|v| *v),
    )
    .await
    .expect("shutdown not signaled")
    .unwrap();
}

/// A reader that blocks on `read` until a byte is sent, and reports EOF when
/// the sending end is dropped.
/// Lets a test hold the reader loop open, register a request, and then close
/// stdin at a controlled moment.
struct BlockingReader {
    rx: std::sync::mpsc::Receiver<u8>,
}

impl std::io::Read for BlockingReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self.rx.recv() {
            Ok(byte) => {
                buf[0] = byte;
                Ok(1)
            }
            // Sender dropped: report EOF.
            Err(_) => Ok(0),
        }
    }
}

#[tokio::test]
async fn pending_request_resolves_when_stdin_closes() {
    let (tx, rx) = std::sync::mpsc::channel::<u8>();
    let stdin = BufReader::new(BlockingReader { rx });
    let (client, _shutdown) = PluginClient::start(stdin, shared_writer());

    // Issue a request and wait until it is registered as pending.
    let request = tokio::spawn({
        let client = client.clone();
        async move { client.list_conversations().await }
    });
    while client.inner.pending.lock().unwrap().is_empty() {
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }

    // Closing stdin (dropping the sender) makes the reader loop exit and drain
    // the pending map, so the request resolves instead of hanging.
    drop(tx);

    let err = tokio::time::timeout(std::time::Duration::from_secs(1), request)
        .await
        .expect("request hung after stdin closed")
        .unwrap()
        .unwrap_err();

    assert!(matches!(err, ClientError::ChannelClosed));
}

/// A writer whose every operation fails, to exercise the send-error path.
struct FailingWriter;

impl Write for FailingWriter {
    fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "boom"))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "boom"))
    }
}

#[tokio::test]
async fn failed_send_does_not_leak_pending() {
    // Hold the sender so the reader loop stays parked and can't drain the map
    // itself; the request must clean up its own entry when the send fails.
    let (_tx, rx) = std::sync::mpsc::channel::<u8>();
    let stdin = BufReader::new(BlockingReader { rx });
    let writer: SharedWriter = Arc::new(Mutex::new(Box::new(FailingWriter)));
    let (client, _shutdown) = PluginClient::start(stdin, writer);

    let err = client.list_conversations().await.unwrap_err();
    assert!(matches!(err, ClientError::Protocol(_)));
    assert!(client.inner.pending.lock().unwrap().is_empty());
}
