use std::{
    io::{BufReader, Cursor, Read, Write},
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

use jp_plugin::message::*;
use serde_json::json;

use super::*;

/// Helper to build a host response line.
fn host_line(msg: &HostToPlugin) -> String {
    serde_json::to_string(msg).unwrap()
}

/// A writer whose bytes stay readable, for asserting what reached the host.
#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<u8>>>);

impl Recorder {
    fn written(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

impl Write for Recorder {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A reader that blocks on `read` until a byte is sent, and reports EOF when
/// the sending end is dropped.
/// Lets a test hold the reader open, issue a request, and then close stdin at a
/// controlled moment.
struct BlockingReader {
    rx: mpsc::Receiver<u8>,
}

impl Read for BlockingReader {
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

/// A client reading the bytes sent on the returned sender, and recording what
/// it writes.
fn recording_client() -> (PluginClient, mpsc::Sender<u8>, Recorder) {
    let (tx, rx) = mpsc::channel::<u8>();
    let stdin = BufReader::new(BlockingReader { rx });
    let recorder = Recorder::default();
    let writer: Writer = Arc::new(Mutex::new(Box::new(recorder.clone())));
    let (client, _shutdown) = PluginClient::start(stdin, writer).unwrap();

    (client, tx, recorder)
}

/// Wait until the client has written `requests` whole lines.
///
/// A request waits for its answer before it is written, so an answer fed after
/// this reaches a waiting request rather than being dropped.
async fn wait_for_requests(sent: &Recorder, requests: usize) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while sent.written().matches('\n').count() < requests {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("the client didn't write its request within 5 seconds");
}

/// Feed `lines` to the reader once the client has written its first request.
fn answer_first_request(sent: &Recorder, tx: mpsc::Sender<u8>, lines: Vec<String>) {
    let sent = sent.clone();
    tokio::spawn(async move {
        wait_for_requests(&sent, 1).await;
        for line in lines {
            for b in format!("{line}\n").into_bytes() {
                let _ = tx.send(b);
            }
        }
    });
}

#[tokio::test]
async fn list_conversations_roundtrip() {
    let response = HostToPlugin::Conversations(ConversationsResponse {
        id: Some("1".to_owned()),
        data: vec![ConversationSummary {
            id: "123".to_owned(),
            title: Some("Test".to_owned()),
            last_activated_at: chrono::Utc::now(),
            pinned_at: None,
            events_count: 5,
        }],
    });

    let (client, tx, sent) = recording_client();
    answer_first_request(&sent, tx, vec![host_line(&response)]);
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

    let (client, tx, sent) = recording_client();
    answer_first_request(&sent, tx, vec![host_line(&response)]);
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

    let (client, tx, sent) = recording_client();
    answer_first_request(&sent, tx, vec![host_line(&response)]);
    let err = client.list_conversations().await.unwrap_err();

    assert!(matches!(err, ClientError::Host(msg) if msg.contains("something went wrong")));
}

/// Starting a conversation is answered twice: once with the id as soon as it
/// exists, and again when its first turn ends.
/// The second answer is what clears the turn from the busy map, so losing it
/// would leave the conversation marked running for the life of the process.
#[tokio::test]
async fn starting_a_conversation_returns_its_id_and_then_its_turns_end() {
    let (client, tx, sent) = recording_client();
    answer_first_request(&sent, tx, vec![
        host_line(&HostToPlugin::Created(CreatedResponse {
            id: Some("1".to_owned()),
            conversation: "jp-c1".to_owned(),
        })),
        host_line(&HostToPlugin::QueryComplete(QueryCompleteResponse {
            id: Some("1".to_owned()),
            conversation: "jp-c1".to_owned(),
            outcome: QueryOutcome::Finished,
            data: None,
            answer: None,
        })),
    ]);

    let (conversation, outcome) = client
        .start_conversation("Hi", Some("Chat".to_owned()), vec![])
        .await
        .unwrap();

    assert_eq!(conversation, "jp-c1");
    outcome.finished().await.unwrap();

    let expected =
        r#"{"type":"query","id":"1","conversation":"","content":"Hi","new":true,"title":"Chat"}"#;
    assert_eq!(sent.written(), format!("{expected}\n"));
}

#[tokio::test]
async fn a_refused_conversation_reports_the_hosts_reason() {
    let (client, tx, sent) = recording_client();
    answer_first_request(&sent, tx, vec![host_line(&HostToPlugin::Error(
        ErrorResponse {
            id: Some("1".to_owned()),
            request: Some("query".to_owned()),
            message: "unknown persona".to_owned(),
        },
    ))]);

    let Err(error) = client.start_conversation("Hi", None, vec![]).await else {
        panic!("the host refused the conversation");
    };

    assert!(matches!(error, ClientError::Host(message) if message == "unknown persona"));
}

/// Sending to a busy conversation asks the host to interrupt the turn and
/// answer it, rather than to start a second one.
///
/// Asserted as the exact line on the wire, because this is the contract with
/// the host: an `action` an older host does not read leaves the turn stopped
/// and the message discarded.
#[tokio::test]
async fn a_reply_is_sent_as_an_interrupt_carrying_the_message() {
    let (client, tx, sent) = recording_client();
    answer_first_request(&sent, tx, vec![host_line(&HostToPlugin::Done(
        DoneResponse {
            id: Some("1".to_owned()),
        },
    ))]);

    client
        .reply("jp-c17000000000", "use Rust instead")
        .await
        .unwrap();

    let expected = r#"{"type":"interrupt","id":"1","conversation":"jp-c17000000000","action":"reply","content":"use Rust instead"}"#;

    assert_eq!(sent.written(), format!("{expected}\n"));
}

/// A host with no turn left to interrupt says so, and the caller still owns the
/// message.
#[tokio::test]
async fn a_reply_to_a_finished_turn_reports_the_hosts_reason() {
    let (client, tx, sent) = recording_client();
    answer_first_request(&sent, tx, vec![host_line(&HostToPlugin::Error(
        ErrorResponse {
            id: Some("1".to_owned()),
            request: Some("interrupt".to_owned()),
            message: "no turn is running on conversation jp-c17000000000".to_owned(),
        },
    ))]);

    let error = client
        .reply("jp-c17000000000", "use Rust instead")
        .await
        .expect_err("there is nothing to interrupt");

    assert!(matches!(
        error,
        ClientError::Host(message)
            if message == "no turn is running on conversation jp-c17000000000"
    ));
}

#[tokio::test]
async fn shutdown_signals_watch() {
    let stdin_data = format!("{}\n", host_line(&HostToPlugin::Shutdown));
    let stdin = BufReader::new(Cursor::new(stdin_data));
    let writer: Writer = Arc::new(Mutex::new(Box::new(Vec::<u8>::new())));
    let (_client, mut shutdown_rx) = PluginClient::start(stdin, writer).unwrap();

    tokio::time::timeout(Duration::from_secs(1), shutdown_rx.wait_for(|v| *v))
        .await
        .expect("shutdown not signaled")
        .unwrap();
}

#[tokio::test]
async fn pending_request_resolves_when_stdin_closes() {
    let (client, tx, sent) = recording_client();

    let request = tokio::spawn({
        let client = client.clone();
        async move { client.list_conversations().await }
    });
    wait_for_requests(&sent, 1).await;

    // Closing stdin (dropping the sender) makes the reader exit and drop every
    // waiter, so the request resolves instead of hanging.
    drop(tx);

    let err = tokio::time::timeout(Duration::from_secs(1), request)
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
async fn a_failed_send_is_a_protocol_error() {
    // Hold the sender so the reader stays parked rather than closing the
    // connection, which would fail the request for another reason.
    let (_tx, rx) = mpsc::channel::<u8>();
    let stdin = BufReader::new(BlockingReader { rx });
    let writer: Writer = Arc::new(Mutex::new(Box::new(FailingWriter)));
    let (client, _shutdown) = PluginClient::start(stdin, writer).unwrap();

    let err = client.list_conversations().await.unwrap_err();

    assert!(matches!(err, ClientError::Protocol(msg) if msg == "boom"));
}
