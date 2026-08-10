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

/// Start a client whose reader consumes bytes from the returned sender.
///
/// Pair with `feed_after_register` to deliver a canned response only once a
/// request is pending.
/// Feeding through a pre-filled reader instead races the reader thread against
/// request registration: the reader can dispatch (and drop) the response before
/// the request has registered, leaving the request to wait out the full
/// timeout.
fn channel_client() -> (PluginClient, std::sync::mpsc::Sender<u8>) {
    let (tx, rx) = std::sync::mpsc::channel::<u8>();
    let stdin = BufReader::new(BlockingReader { rx });
    let (client, _shutdown) = PluginClient::start(stdin, shared_writer());
    (client, tx)
}

/// Deliver `line` to the reader once `client` has a pending request, so the
/// response is dispatched to a waiting receiver rather than dropped.
fn feed_after_register(client: &PluginClient, tx: std::sync::mpsc::Sender<u8>, line: String) {
    let client = client.clone();
    tokio::spawn(async move {
        while client.inner.pending.lock().unwrap().is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        for b in line.into_bytes() {
            let _ = tx.send(b);
        }
    });
}

/// Two replies to one request reach two waiters, in the order they registered.
///
/// Starting a conversation is answered twice: once with the id as soon as it
/// exists, and again when its first turn ends.
/// The second reply is what clears the turn from the busy map, so a dispatcher
/// that served only the first would leave the conversation marked running for
/// the life of the process.
#[tokio::test]
async fn two_replies_to_one_request_reach_both_waiters() {
    let (client, _tx) = channel_client();

    let first = client.register("7");
    let second = client.register("7");

    dispatch(
        &client.inner.pending,
        Some("7"),
        HostToPlugin::Created(CreatedResponse {
            id: Some("7".to_owned()),
            conversation: "jp-c1".to_owned(),
        }),
    );
    dispatch(
        &client.inner.pending,
        Some("7"),
        HostToPlugin::QueryComplete(QueryCompleteResponse {
            id: Some("7".to_owned()),
            conversation: "jp-c1".to_owned(),
        }),
    );

    assert!(
        matches!(first.await, Ok(HostToPlugin::Created(_))),
        "the first waiter gets the first reply"
    );
    assert!(
        matches!(second.await, Ok(HostToPlugin::QueryComplete(_))),
        "the second waiter gets the second, rather than the first being served twice or the \
         second being dropped"
    );

    assert!(
        client.inner.pending.lock().unwrap().is_empty(),
        "the request is forgotten once its last waiter is served"
    );
}

/// One waiter still behaves as it always did.
///
/// The queue is only there for requests answered more than once; every other
/// request registers one waiter and must be cleaned up by the reply that serves
/// it, not left behind for a second that never comes.
#[tokio::test]
async fn a_single_reply_still_clears_its_request() {
    let (client, _tx) = channel_client();

    let only = client.register("3");

    dispatch(
        &client.inner.pending,
        Some("3"),
        HostToPlugin::QueryComplete(QueryCompleteResponse {
            id: Some("3".to_owned()),
            conversation: "jp-c1".to_owned(),
        }),
    );

    assert!(matches!(only.await, Ok(HostToPlugin::QueryComplete(_))));
    assert!(
        client.inner.pending.lock().unwrap().is_empty(),
        "a request with one waiter is forgotten when that waiter is served"
    );
}

/// Abandoning a request drops every waiter it registered.
///
/// The error paths in `start_conversation` register two and may bail after the
/// first; leaving the second behind would keep a sender alive for a reply that
/// is never coming.
#[tokio::test]
async fn forgetting_a_request_drops_all_of_its_waiters() {
    let (client, _tx) = channel_client();

    let first = client.register("9");
    let second = client.register("9");

    client.forget("9");

    assert!(client.inner.pending.lock().unwrap().is_empty());
    assert!(first.await.is_err(), "a dropped sender closes its channel");
    assert!(second.await.is_err());
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

    let (client, tx) = channel_client();
    feed_after_register(&client, tx, format!("{}\n", host_line(&response)));
    let result = client.list_conversations().await.unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].id, "123");
    assert_eq!(result[0].title.as_deref(), Some("Test"));
}

#[tokio::test]
async fn read_events_roundtrip() {
    let response = HostToPlugin::Events(EventsResponse {
        lock: jp_plugin::message::LockState::Free,
        title: None,
        id: Some("1".to_owned()),
        conversation: "456".to_owned(),
        data: vec![json!({"type": "turn_start", "timestamp": "2025-01-01T00:00:00Z"})],
    });

    let (client, tx) = channel_client();
    feed_after_register(&client, tx, format!("{}\n", host_line(&response)));
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

    let (client, tx) = channel_client();
    feed_after_register(&client, tx, format!("{}\n", host_line(&response)));
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
