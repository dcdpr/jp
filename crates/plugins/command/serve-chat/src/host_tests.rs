//! Unit tests for [`HostClient`] over in-memory pipes.

use std::{
    io::{self, BufReader, PipeWriter},
    num::NonZeroI64,
};

use jp_plugin::message::{
    CompactedResponse, ConfigResponse, CreatedResponse, DoneResponse, ErrorResponse,
    EventsResponse, InterruptAction, QueryCompleteResponse, ReasoningCompaction,
};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

#[derive(Clone, Default)]
struct Written(Arc<Mutex<Vec<u8>>>);

impl Write for Written {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Written {
    fn messages(&self) -> Vec<PluginToHost> {
        String::from_utf8(self.0.lock().unwrap().clone())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

struct Fixture {
    client: HostClient,
    written: Written,
    host: Arc<Mutex<PipeWriter>>,
    shutdown: watch::Receiver<bool>,
}

fn fixture() -> Fixture {
    let written = Written::default();
    let (reader, writer) = io::pipe().unwrap();
    let client = HostClient::new(written.clone());
    let shutdown = client.listen(BufReader::new(reader)).unwrap();

    Fixture {
        client,
        written,
        host: Arc::new(Mutex::new(writer)),
        shutdown,
    }
}

fn answer(host: &Mutex<PipeWriter>, message: &HostToPlugin) {
    let mut host = host.lock().unwrap();
    writeln!(host, "{}", serde_json::to_string(message).unwrap()).unwrap();
}

async fn eventually(condition: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("the condition didn't hold within 5 seconds");
}

async fn within<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .expect("no answer within 5 seconds")
}

fn query_complete(id: &str) -> HostToPlugin {
    HostToPlugin::QueryComplete(QueryCompleteResponse {
        id: Some(id.to_owned()),
        conversation: "c1".to_owned(),
        data: None,
    })
}

fn error(id: &str, message: &str) -> HostToPlugin {
    HostToPlugin::Error(ErrorResponse {
        id: Some(id.to_owned()),
        request: None,
        message: message.to_owned(),
    })
}

fn done(id: &str) -> HostToPlugin {
    HostToPlugin::Done(DoneResponse {
        id: Some(id.to_owned()),
    })
}

#[tokio::test]
async fn a_query_is_written_before_its_turn_is_awaited() {
    let f = fixture();

    let turn = f
        .client
        .query("c1", "Hi", vec!["personas/planner".to_owned()]);

    assert_eq!(f.written.messages(), vec![PluginToHost::Query(
        QueryRequest {
            id: Some("1".to_owned()),
            conversation: "c1".to_owned(),
            content: "Hi".to_owned(),
            new: false,
            title: None,
            cfg: vec!["personas/planner".to_owned()],
            schema: None,
            expires_in: None,
        }
    )]);

    answer(&f.host, &query_complete("1"));
    assert_eq!(within(turn).await, Ok(()));
}

#[tokio::test]
async fn a_query_the_host_refuses_fails_with_its_message() {
    let f = fixture();

    let turn = f.client.query("c1", "Hi", vec![]);
    answer(&f.host, &error("1", "conversation is locked"));

    assert_eq!(within(turn).await, Err("conversation is locked".to_owned()));
}

#[tokio::test]
async fn start_takes_created_and_the_turns_end_sent_back_to_back() {
    let f = fixture();
    let (written, host) = (f.written.clone(), Arc::clone(&f.host));
    tokio::spawn(async move {
        eventually(|| written.messages().len() == 1).await;
        answer(
            &host,
            &HostToPlugin::Created(CreatedResponse {
                id: Some("1".to_owned()),
                conversation: "c7".to_owned(),
            }),
        );
        answer(&host, &query_complete("1"));
    });

    let (conversation, turn) = within(f.client.start(NewTurn {
        content: "Hi".to_owned(),
        title: "Chat".to_owned(),
        cfg: vec![],
        expires_in: "7d".to_owned(),
    }))
    .await
    .unwrap();

    assert_eq!(conversation, "c7");
    assert_eq!(within(turn).await, Ok(()));
    let PluginToHost::Query(request) = &f.written.messages()[0] else {
        panic!("not a query: {:?}", f.written.messages());
    };
    assert!(request.new);
    assert_eq!(request.title.as_deref(), Some("Chat"));
    assert_eq!(request.expires_in.as_deref(), Some("7d"));
}

#[tokio::test]
async fn start_reports_the_hosts_refusal() {
    let f = fixture();
    let (written, host) = (f.written.clone(), Arc::clone(&f.host));
    tokio::spawn(async move {
        eventually(|| written.messages().len() == 1).await;
        answer(&host, &error("1", "unknown persona"));
    });

    let result = within(f.client.start(NewTurn {
        content: "Hi".to_owned(),
        title: "Chat".to_owned(),
        cfg: vec![],
        expires_in: "7d".to_owned(),
    }))
    .await;

    assert_eq!(result.err(), Some("unknown persona".to_owned()));
}

#[tokio::test]
async fn a_reply_is_taken_or_refused() {
    let f = fixture();
    let (written, host) = (f.written.clone(), Arc::clone(&f.host));
    tokio::spawn(async move {
        eventually(|| written.messages().len() == 2).await;
        answer(&host, &done("1"));
        answer(&host, &error("2", "no turn is running"));
    });

    let taken = tokio::spawn(f.client.reply("c1", "Actually second"));
    let refused = tokio::spawn(f.client.reply("c1", "Actually third"));

    assert_eq!(within(taken).await.unwrap(), Ok(()));
    assert_eq!(
        within(refused).await.unwrap(),
        Err(ReplyError::Refused("no turn is running".to_owned()))
    );
    let PluginToHost::Interrupt(request) = &f.written.messages()[0] else {
        panic!("not an interrupt: {:?}", f.written.messages());
    };
    assert_eq!(request.action, InterruptAction::Reply);
    assert_eq!(request.content.as_deref(), Some("Actually second"));
}

fn stripped() -> StoredCompaction {
    StoredCompaction {
        from: 2,
        to: 5,
        reasoning: true,
        tool_calls: None,
        summary: None,
    }
}

#[tokio::test]
async fn compact_sends_the_compactions_and_returns_the_stored_ones() {
    let f = fixture();
    let compactions = vec![Compaction {
        from: NonZeroI64::new(2).unwrap(),
        to: NonZeroI64::new(-2).unwrap(),
        reasoning: Some(ReasoningCompaction::default()),
        tool_calls: None,
        summary: None,
    }];
    let (written, host) = (f.written.clone(), Arc::clone(&f.host));
    tokio::spawn(async move {
        eventually(|| written.messages().len() == 1).await;
        answer(
            &host,
            &HostToPlugin::Compacted(CompactedResponse {
                id: Some("1".to_owned()),
                conversation: "c1".to_owned(),
                compactions: vec![stripped()],
            }),
        );
    });

    let stored = within(f.client.compact("c1", compactions.clone())).await;

    assert_eq!(stored, Ok(vec![stripped()]));
    assert_eq!(f.written.messages(), vec![PluginToHost::Compact(
        CompactRequest {
            id: Some("1".to_owned()),
            conversation: "c1".to_owned(),
            compactions,
        }
    )]);
}

#[tokio::test]
async fn a_compaction_the_host_refuses_fails_with_its_message() {
    let f = fixture();
    let (written, host) = (f.written.clone(), Arc::clone(&f.host));
    tokio::spawn(async move {
        eventually(|| written.messages().len() == 1).await;
        answer(&host, &error("1", "No turns to compact"));
    });

    let stored = within(f.client.compact("c1", vec![])).await;

    assert_eq!(stored, Err("No turns to compact".to_owned()));
}

#[tokio::test]
async fn stop_asks_the_running_turn_to_stop() {
    let f = fixture();
    let (written, host) = (f.written.clone(), Arc::clone(&f.host));
    tokio::spawn(async move {
        eventually(|| written.messages().len() == 1).await;
        answer(&host, &done("1"));
    });

    assert_eq!(within(f.client.stop("c1")).await, Ok(()));
    assert_eq!(f.written.messages(), vec![PluginToHost::Interrupt(
        InterruptRequest::stop("c1".to_owned()).with_id("1".to_owned())
    )]);
}

#[tokio::test]
async fn tools_reads_the_configuration_again() {
    let f = fixture();
    let (written, host) = (f.written.clone(), Arc::clone(&f.host));
    tokio::spawn(async move {
        eventually(|| written.messages().len() == 1).await;
        answer(
            &host,
            &HostToPlugin::Config(ConfigResponse {
                id: Some("1".to_owned()),
                path: Some("conversation".to_owned()),
                data: json!({ "tools": { "*": { "run": "ask" }, "bash": {} } }),
            }),
        );
    });

    let tools = within(f.client.tools()).await;

    assert_eq!(tools, Ok(vec!["bash".to_owned()]));
    assert_eq!(f.written.messages(), vec![PluginToHost::ReadConfig(
        ReadConfigRequest {
            id: Some("1".to_owned()),
            path: Some("conversation".to_owned()),
            reload: true,
        }
    )]);
}

#[tokio::test]
async fn events_returns_the_transcript() {
    let f = fixture();
    let (written, host) = (f.written.clone(), Arc::clone(&f.host));
    tokio::spawn(async move {
        eventually(|| written.messages().len() == 1).await;
        answer(
            &host,
            &HostToPlugin::Events(EventsResponse {
                id: Some("1".to_owned()),
                conversation: "c1".to_owned(),
                lock: jp_plugin::message::LockState::Free,
                title: None,
                data: vec![json!({ "type": "turn_start" })],
            }),
        );
    });

    let events = within(f.client.events("c1")).await;

    assert_eq!(events, Ok(vec![json!({ "type": "turn_start" })]));
}

#[tokio::test]
async fn a_closed_host_ends_every_waiting_request_and_shuts_down() {
    let Fixture {
        client,
        host,
        mut shutdown,
        ..
    } = fixture();

    let turn = client.query("c1", "Hi", vec![]);
    drop(host);

    assert_eq!(
        within(turn).await,
        Err("the connection to the host closed".to_owned())
    );
    within(shutdown.wait_for(|stopped| *stopped)).await.unwrap();
}

#[tokio::test]
async fn a_shutdown_message_shuts_down() {
    let mut f = fixture();

    answer(&f.host, &HostToPlugin::Shutdown);

    within(f.shutdown.wait_for(|stopped| *stopped))
        .await
        .unwrap();
}

#[tokio::test]
async fn an_answer_nobody_waits_for_is_dropped() {
    let f = fixture();

    let turn = f.client.query("c1", "Hi", vec![]);
    answer(&f.host, &query_complete("99"));
    answer(&f.host, &query_complete("1"));

    assert_eq!(within(turn).await, Ok(()));
}

#[test]
fn log_print_and_exit_write_their_messages() {
    let written = Written::default();
    let client = HostClient::new(written.clone());

    client.log(Level::WARN, "Dropped a message.");
    client.log_with(
        Level::INFO,
        "Turn ended.",
        Map::from_iter([("run_ms".to_owned(), 5200.into())]),
    );
    client.print("Relaying Signal messages.");
    client.exit(2, Some("No arguments.".to_owned())).unwrap();

    assert_eq!(written.messages(), vec![
        PluginToHost::Log(LogMessage {
            level: "warn".to_owned(),
            message: "Dropped a message.".to_owned(),
            fields: Map::new(),
        }),
        PluginToHost::Log(LogMessage {
            level: "info".to_owned(),
            message: "Turn ended.".to_owned(),
            fields: Map::from_iter([("run_ms".to_owned(), 5200.into())]),
        }),
        PluginToHost::Print(PrintMessage {
            text: "Relaying Signal messages.\n".to_owned(),
            channel: "content".to_owned(),
            format: "plain".to_owned(),
            language: None,
        }),
        PluginToHost::Exit(ExitMessage {
            code: 2,
            reason: Some("No arguments.".to_owned()),
            expected: false,
        }),
    ]);
}
