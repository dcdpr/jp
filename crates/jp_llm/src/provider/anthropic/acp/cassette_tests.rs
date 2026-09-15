//! Tests for the cassette machinery itself.
//!
//! Every script here is written by hand, so none of it is evidence about the
//! adapter's wire format.
//! What it covers is the replay harness: that a recorded conversation reaches
//! JP in the right order, that an answer finds the request it belongs to, and
//! that a script which stops agreeing with JP says so.

use std::time::Duration;

use serde_json::json;
use tokio::time::timeout;

use super::*;
use crate::provider::anthropic::acp::{
    rpc::{Handler, Inbound, Request, RpcError, drive},
    transport::{Foreground, Transport as _},
};

/// A request with no parameters, standing in for whatever JP sends first.
#[derive(Serialize)]
struct Ping;

impl Request for Ping {
    const METHOD: &'static str = "ping";

    type Response = Value;
}

/// A second method, for proving a divergence is reported against the right one.
#[derive(Serialize)]
struct Pong;

impl Request for Pong {
    const METHOD: &'static str = "pong";

    type Response = Value;
}

fn jp(message: Value) -> Framed {
    Framed {
        connection: 0,
        from: Side::Jp,
        message,
    }
}

fn agent(message: Value) -> Framed {
    Framed {
        connection: 0,
        from: Side::Agent,
        message,
    }
}

/// A handler that answers every agent request with `answer` and accepts every
/// notification, recording what it saw.
fn handler(answer: Value, seen: Arc<Mutex<Vec<String>>>) -> Handler {
    Box::new(move |inbound| {
        let answer = answer.clone();
        let seen = seen.clone();
        Box::pin(async move {
            let method = match inbound {
                Inbound::Notification { method, .. } | Inbound::Request { method, .. } => method,
            };
            seen.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(method);
            Ok(answer)
        })
    })
}

/// A foreground that sends one `Ping` and keeps the answer.
fn ping_into(slot: Arc<Mutex<Option<Value>>>) -> Foreground {
    Box::new(move |peer| {
        Box::pin(async move {
            let answer = peer.request(Ping).await?;
            *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(answer);
            Ok(())
        })
    })
}

/// Run `script` against a foreground sequence, returning its outcome.
async fn against(
    script: Vec<Framed>,
    foreground: Foreground,
) -> (Result<(), RpcError>, Vec<String>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let transport = Box::new(Recorded(script));
    let outcome = timeout(
        Duration::from_secs(5),
        transport.connect(handler(Value::Null, seen.clone()), foreground),
    )
    .await
    .expect("the replayed connection did not settle");
    let seen = seen.lock().unwrap_or_else(PoisonError::into_inner).clone();
    (outcome, seen)
}

#[tokio::test]
async fn a_recorded_answer_reaches_the_request_it_belongs_to() {
    let script = vec![
        jp(json!({"jsonrpc": "2.0", "id": 1, "method": "ping", "params": null})),
        agent(json!({"jsonrpc": "2.0", "id": 1, "result": {"pong": true}})),
    ];
    let answered = Arc::new(Mutex::new(None));
    let captured = answered.clone();
    let foreground: Foreground = Box::new(move |peer| {
        Box::pin(async move {
            let response = peer.request(Ping).await?;
            *captured.lock().unwrap_or_else(PoisonError::into_inner) = Some(response);
            Ok(())
        })
    });

    let (outcome, _) = against(script, foreground).await;

    outcome.unwrap();
    assert_eq!(
        answered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take(),
        Some(json!({"pong": true}))
    );
}

/// A recording made in one run carries that run's ids, and a replay allocates
/// its own from one.
/// An answer keyed on the recorded id would never be delivered.
#[tokio::test]
async fn an_answer_recorded_under_another_id_is_still_delivered() {
    let script = vec![
        jp(json!({"jsonrpc": "2.0", "id": 74, "method": "ping", "params": null})),
        agent(json!({"jsonrpc": "2.0", "id": 74, "result": {"pong": true}})),
    ];
    let answered = Arc::new(Mutex::new(None));
    let captured = answered.clone();
    let foreground: Foreground = Box::new(move |peer| {
        Box::pin(async move {
            let response = peer.request(Ping).await?;
            *captured.lock().unwrap_or_else(PoisonError::into_inner) = Some(response);
            Ok(())
        })
    });

    let (outcome, _) = against(script, foreground).await;

    outcome.unwrap();
    assert_eq!(
        answered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take(),
        Some(json!({"pong": true}))
    );
}

/// The shape a permission prompt takes: the agent interrupts with a request of
/// its own while JP's is still open, and JP's answer comes before the result.
#[tokio::test]
async fn an_agent_request_is_served_while_jps_own_is_outstanding() {
    let script = vec![
        jp(json!({"jsonrpc": "2.0", "id": 1, "method": "ping", "params": null})),
        agent(json!({"jsonrpc": "2.0", "method": "notice", "params": {}})),
        agent(json!({"jsonrpc": "2.0", "id": 900, "method": "permission", "params": {}})),
        jp(json!({"jsonrpc": "2.0", "id": 900, "result": null})),
        agent(json!({"jsonrpc": "2.0", "id": 1, "result": {"pong": true}})),
    ];
    let foreground: Foreground = Box::new(move |peer| {
        Box::pin(async move {
            peer.request(Ping).await?;
            Ok(())
        })
    });

    let (outcome, seen) = against(script, foreground).await;

    outcome.unwrap();
    assert_eq!(seen, vec!["notice".to_owned(), "permission".to_owned()]);
}

#[tokio::test]
async fn a_script_that_expects_another_method_reports_both() {
    let script = vec![jp(json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}))];
    let foreground: Foreground = Box::new(move |peer| {
        Box::pin(async move {
            peer.request(Pong).await?;
            Ok(())
        })
    });

    let (outcome, _) = against(script, foreground).await;

    assert_eq!(
        outcome.unwrap_err().message,
        "the recording has `ping` where JP sent `pong`"
    );
}

#[tokio::test]
async fn a_script_that_runs_out_says_so() {
    let script = vec![
        jp(json!({"jsonrpc": "2.0", "id": 1, "method": "ping"})),
        agent(json!({"jsonrpc": "2.0", "id": 1, "result": null})),
    ];
    let foreground: Foreground = Box::new(move |peer| {
        Box::pin(async move {
            peer.request(Ping).await?;
            peer.request(Pong).await?;
            Ok(())
        })
    });

    let (outcome, _) = against(script, foreground).await;

    assert_eq!(
        outcome.unwrap_err().message,
        "JP sent `pong` after the recording ended"
    );
}

#[test]
fn a_missing_recording_names_the_path_it_wanted() {
    let error = read("no-such-conversation").unwrap_err();

    assert!(
        error.starts_with("Recording not found at ")
            && error.contains("acp/no-such-conversation.jsonl"),
        "{error}"
    );
}

#[test]
fn a_recorded_conversation_reads_back_as_it_was_observed() {
    let directory = camino_tempfile::tempdir().unwrap();
    let path = directory.path().join("nested/traffic.jsonl");
    let tap = recorder(path.as_std_path());

    tap.observe(Side::Jp, &json!({"id": 1, "method": "ping"}));
    tap.observe(Side::Agent, &json!({"id": 1, "result": null}));

    let recorded = parse(&std::fs::read_to_string(&path).unwrap()).unwrap();

    assert_eq!(recorded.len(), 2);
    assert_eq!(recorded[0].from, Side::Jp);
    assert_eq!(recorded[0].message, json!({"id": 1, "method": "ping"}));
    assert_eq!(recorded[1].from, Side::Agent);
    assert_eq!(recorded[1].message, json!({"id": 1, "result": null}));
}

/// The recorder and the replayer are two halves of one format, written apart.
/// Recording a live exchange and then replaying the file has to reach the
/// foreground with the answer the live agent gave, or one half is writing what
/// the other cannot read.
#[tokio::test]
async fn what_the_recorder_writes_is_what_the_replayer_reads() {
    let directory = camino_tempfile::tempdir().unwrap();
    let path = directory.path().join("traffic.jsonl");

    // An agent that answers whatever it is asked, over a real pipe, observed by
    // the same tap the production path installs under `RECORD`.
    let (jp_writes, agent_reads) = tokio::io::duplex(1 << 16);
    let (agent_writes, jp_reads) = tokio::io::duplex(1 << 16);
    tokio::spawn(async move {
        use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};

        let mut lines = tokio::io::BufReader::new(agent_reads).lines();
        let mut writes = agent_writes;
        while let Ok(Some(line)) = lines.next_line().await {
            let asked: Value = serde_json::from_str(&line).unwrap();
            let reply = json!({"jsonrpc": "2.0", "id": asked["id"], "result": {"pong": true}});
            let mut line = serde_json::to_vec(&reply).unwrap();
            line.push(b'\n');
            writes.write_all(&line).await.unwrap();
            writes.flush().await.unwrap();
        }
    });

    let live = Arc::new(Mutex::new(None));
    timeout(
        Duration::from_secs(5),
        drive(
            jp_writes,
            jp_reads,
            recorder(path.as_std_path()),
            handler(Value::Null, Arc::default()),
            ping_into(live.clone()),
        ),
    )
    .await
    .expect("the recorded connection did not settle")
    .unwrap();

    let replayed = Arc::new(Mutex::new(None));
    let script = parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let (outcome, _) = against(script, ping_into(replayed.clone())).await;
    outcome.unwrap();

    let live = live.lock().unwrap_or_else(PoisonError::into_inner).take();
    assert_eq!(live, Some(json!({"pong": true})));
    assert_eq!(
        replayed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take(),
        live
    );
}

/// A query opens a connection per request, and all of them belong in one
/// recording: a per-connection truncation would leave only the last.
#[test]
fn successive_connections_accumulate_in_one_recording() {
    let directory = camino_tempfile::tempdir().unwrap();
    let path = directory.path().join("traffic.jsonl");

    let first = recorder(path.as_std_path());
    first.observe(Side::Jp, &json!({"method": "one"}));
    let second = recorder(path.as_std_path());
    second.observe(Side::Jp, &json!({"method": "two"}));
    // Interleaved, since a connection stays open while the next one starts.
    first.observe(Side::Agent, &json!({"result": "one"}));

    let recorded = parse(&std::fs::read_to_string(&path).unwrap()).unwrap();

    assert_eq!(
        recorded
            .iter()
            .map(|entry| (entry.connection, entry.message.clone()))
            .collect::<Vec<_>>(),
        [
            (0, json!({"method": "one"})),
            (1, json!({"method": "two"})),
            (0, json!({"result": "one"})),
        ]
    );
}

/// A recording arrives interleaved, since one connection stays open while the
/// next begins.
/// Each script has to come back whole and in order regardless.
#[test]
fn a_recording_splits_into_one_script_per_connection() {
    let second = |message| Framed {
        connection: 1,
        from: Side::Jp,
        message,
    };
    let script = vec![
        jp(json!({"method": "first-a"})),
        second(json!({"method": "second-a"})),
        jp(json!({"method": "first-b"})),
        second(json!({"method": "second-b"})),
    ];

    let scripts = connections(script);

    assert_eq!(scripts.len(), 2);
    assert_eq!(
        scripts[0]
            .iter()
            .map(|entry| entry.message["method"].clone())
            .collect::<Vec<_>>(),
        [json!("first-a"), json!("first-b")]
    );
    assert_eq!(
        scripts[1]
            .iter()
            .map(|entry| entry.message["method"].clone())
            .collect::<Vec<_>>(),
        [json!("second-a"), json!("second-b")]
    );
}

#[test]
fn an_empty_recording_splits_into_no_scripts() {
    assert!(connections(Vec::new()).is_empty());
}

/// A recording is committed, so what identifies the account that made it and
/// the machine it ran on has to be gone before it reaches the file, not after
/// somebody remembers.
#[test]
fn what_identifies_the_recorder_never_reaches_the_file() {
    let directory = camino_tempfile::tempdir().unwrap();
    let path = directory.path().join("traffic.jsonl");
    let tap = recorder(path.as_std_path());

    tap.observe(
        Side::Agent,
        &json!({
            "method": "_auth/status_update",
            "params": {"authStatus": {"kind": "account", "account": {
                "email": "someone@example.com",
                "organization": "Example Inc",
                "plan": "Claude Max",
            }}},
        }),
    );
    tap.observe(
        Side::Jp,
        &json!({"method": "session/load", "params": {"cwd": "/home/someone/work"}}),
    );
    tap.observe(
        Side::Agent,
        &json!({"method": "_claude/sdkMessage", "params": {"message": {"cwd": "/home/someone/work"}}}),
    );

    let contents = std::fs::read_to_string(&path).unwrap();
    let recorded = parse(&contents).unwrap();

    assert_eq!(
        recorded[0].message["params"]["authStatus"]["account"],
        json!({
            "email": "[redacted]",
            "organization": "[redacted]",
            // The plan sits beside the two redacted fields and says nothing
            // about who holds it, so losing it would cost the recording detail
            // for nothing.
            "plan": "Claude Max",
        })
    );
    assert_eq!(recorded[1].message["params"]["cwd"], json!("[redacted]"));
    assert_eq!(
        recorded[2].message["params"]["message"]["cwd"],
        json!("[redacted]")
    );
    assert!(!contents.contains("someone"), "{contents}");
    assert!(!contents.contains("Example Inc"), "{contents}");
}

/// A message that simply lacks a redacted field is the common case, and has to
/// pass through rather than grow one.
#[test]
fn redaction_does_not_invent_fields_a_message_lacks() {
    let mut message = json!({"method": "initialize", "params": {"protocolVersion": 1}});
    let before = message.clone();

    for path in SENSITIVE {
        redact(&mut message, path);
    }

    assert_eq!(message, before);
}

#[test]
fn a_malformed_line_is_reported_by_number() {
    let contents = "{\"from\":\"jp\",\"message\":{}}\n\nnot json\n";

    let error = parse(contents).unwrap_err();

    assert!(
        error.starts_with("line 3: not a framed message: "),
        "{error}"
    );
}

#[test]
fn a_framed_message_round_trips_through_its_line() {
    let line =
        serde_json::to_string(&agent(json!({"jsonrpc": "2.0", "method": "notice"}))).unwrap();

    assert_eq!(
        line,
        r#"{"connection":0,"from":"agent","message":{"jsonrpc":"2.0","method":"notice"}}"#
    );

    let parsed: Framed = serde_json::from_str(&line).unwrap();
    assert_eq!(parsed.from, Side::Agent);
    assert_eq!(parsed.message["method"], "notice");
}
