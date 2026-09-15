use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use serde::Deserialize;
use serde_json::json;
use tokio::{
    io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader, DuplexStream},
    sync::mpsc,
    time::{Duration, timeout},
};

use super::*;

/// A request with a fixed method and an echoing response shape.
#[derive(Serialize)]
struct Echo {
    value: String,
}

#[derive(Debug, Deserialize, PartialEq)]
struct Echoed {
    echoed: String,
}

impl Request for Echo {
    const METHOD: &'static str = "test/echo";

    type Response = Echoed;
}

/// One end of an in-memory connection, read and written a line at a time.
///
/// Stands in for the adapter: the test writes what the agent would send and
/// reads what JP sends, so the framing is exercised rather than bypassed.
struct Agent {
    lines: tokio::io::Lines<BufReader<DuplexStream>>,
    writer: DuplexStream,
}

impl Agent {
    async fn read(&mut self) -> Value {
        let line = timeout(Duration::from_secs(5), self.lines.next_line())
            .await
            .expect("agent read timed out")
            .unwrap()
            .expect("JP closed its output");
        serde_json::from_str(&line).unwrap()
    }

    async fn write(&mut self, message: Value) {
        let mut line = serde_json::to_vec(&message).unwrap();
        line.push(b'\n');
        self.writer.write_all(&line).await.unwrap();
        self.writer.flush().await.unwrap();
    }
}

/// Wire an agent to `drive`, returning the agent end and the driver's handle.
fn connect(
    handler: Handler,
    foreground: impl FnOnce(Peer) -> BoxFuture<'static, Result<(), RpcError>> + Send + 'static,
) -> (Agent, tokio::task::JoinHandle<Result<(), RpcError>>) {
    // JP's stdin is what the agent writes to; JP's stdout is what it reads.
    let (jp_input, agent_writer) = tokio::io::duplex(8192);
    let (jp_output, agent_reader) = tokio::io::duplex(8192);
    let driver = tokio::spawn(drive(
        agent_reader,
        jp_input,
        Tap::none(),
        handler,
        foreground,
    ));
    (
        Agent {
            lines: BufReader::new(jp_output).lines(),
            writer: agent_writer,
        },
        driver,
    )
}

/// A handler that answers nothing, for tests that only send requests.
fn silent() -> Handler {
    Box::new(|_| Box::pin(async { Ok(Value::Null) }))
}

fn boxed<F, Fut>(foreground: F) -> impl FnOnce(Peer) -> BoxFuture<'static, Result<(), RpcError>>
where
    F: FnOnce(Peer) -> Fut + Send + 'static,
    Fut: Future<Output = Result<(), RpcError>> + Send + 'static,
{
    move |peer| Box::pin(foreground(peer))
}

#[tokio::test]
async fn a_request_carries_its_method_and_receives_its_typed_answer() {
    let (mut agent, driver) = connect(
        silent(),
        boxed(|peer: Peer| async move {
            let answer = peer
                .request(Echo {
                    value: "hello".into(),
                })
                .await?;
            assert_eq!(answer, Echoed {
                echoed: "hello".into()
            });
            Ok(())
        }),
    );
    let request = agent.read().await;
    assert_eq!(
        request,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "test/echo",
            "params": {"value": "hello"},
        })
    );
    agent
        .write(json!({"jsonrpc": "2.0", "id": 1, "result": {"echoed": "hello"}}))
        .await;
    driver.await.unwrap().unwrap();
}

#[tokio::test]
async fn answers_reach_the_request_that_asked_even_when_they_arrive_reversed() {
    let (mut agent, driver) = connect(
        silent(),
        boxed(|peer: Peer| async move {
            let second = peer.clone();
            // Both are outstanding before either is answered, so the ids are
            // the only thing that can pair them up.
            let first = tokio::spawn(async move {
                peer.request(Echo {
                    value: "first".into(),
                })
                .await
            });
            let second = tokio::spawn(async move {
                second
                    .request(Echo {
                        value: "second".into(),
                    })
                    .await
            });
            assert_eq!(first.await.unwrap().unwrap(), Echoed {
                echoed: "one".into()
            });
            assert_eq!(second.await.unwrap().unwrap(), Echoed {
                echoed: "two".into()
            });
            Ok(())
        }),
    );
    let mut ids = vec![];
    for _ in 0..2 {
        let request = agent.read().await;
        ids.push((
            request["id"].as_i64().unwrap(),
            request["params"]["value"].as_str().unwrap().to_owned(),
        ));
    }
    ids.sort_by(|a, b| a.1.cmp(&b.1));
    let [(first, _), (second, _)] = ids.as_slice() else {
        panic!("expected two requests")
    };
    // Answer the second one first.
    agent
        .write(json!({"jsonrpc": "2.0", "id": second, "result": {"echoed": "two"}}))
        .await;
    agent
        .write(json!({"jsonrpc": "2.0", "id": first, "result": {"echoed": "one"}}))
        .await;
    driver.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_rejected_request_returns_the_agents_error_rather_than_a_local_one() {
    let (mut agent, driver) = connect(
        silent(),
        boxed(|peer: Peer| async move {
            let error = peer
                .request(Echo {
                    value: "nope".into(),
                })
                .await
                .unwrap_err();
            assert_eq!(error.code, -32602);
            assert_eq!(error.message, "Unknown model on this account.");
            assert_eq!(error.data, Some(json!({"model": "future"})));
            Ok(())
        }),
    );
    let id = agent.read().await["id"].clone();
    agent
        .write(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32602,
                "message": "Unknown model on this account.",
                "data": {"model": "future"},
            },
        }))
        .await;
    driver.await.unwrap().unwrap();
}

#[tokio::test]
async fn an_agent_request_is_answered_with_the_handlers_value() {
    let (mut agent, driver) = connect(
        Box::new(|message| {
            Box::pin(async move {
                let Inbound::Request { method, params } = message else {
                    panic!("expected a request")
                };
                assert_eq!(method, "session/request_permission");
                assert_eq!(params, json!({"toolCall": "read"}));
                Ok(json!({"outcome": "allowed"}))
            })
        }),
        // Held open until the agent's request has been answered.
        boxed(|peer: Peer| async move {
            peer.request(Echo {
                value: "wait".into(),
            })
            .await?;
            Ok(())
        }),
    );
    let pending = agent.read().await["id"].clone();
    agent
        .write(json!({
            "jsonrpc": "2.0",
            "id": 77,
            "method": "session/request_permission",
            "params": {"toolCall": "read"},
        }))
        .await;
    assert_eq!(
        agent.read().await,
        json!({
            "jsonrpc": "2.0",
            "id": 77,
            "result": {"outcome": "allowed"},
        })
    );
    agent
        .write(json!({"jsonrpc": "2.0", "id": pending, "result": {"echoed": "wait"}}))
        .await;
    driver.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_handler_refusing_a_notification_ends_the_connection() {
    let (mut agent, driver) = connect(
        Box::new(|message| {
            Box::pin(async move {
                assert!(matches!(message, Inbound::Notification { .. }));
                Err(RpcError::into_internal_error("subscription required"))
            })
        }),
        // Never answered: the notification has to be what ends this.
        boxed(|peer: Peer| async move {
            peer.request(Echo {
                value: "streaming".into(),
            })
            .await?;
            Ok(())
        }),
    );
    agent.read().await;
    agent
        .write(json!({"jsonrpc": "2.0", "method": "_auth/status_update", "params": {}}))
        .await;
    let error = timeout(Duration::from_secs(5), driver)
        .await
        .expect("connection should end")
        .unwrap()
        .unwrap_err();
    assert_eq!(error.message, "subscription required");
}

#[tokio::test]
async fn a_request_outstanding_when_the_agent_goes_away_fails_rather_than_hangs() {
    let (agent, driver) = connect(
        silent(),
        boxed(|peer: Peer| async move {
            let error = peer
                .request(Echo {
                    value: "orphan".into(),
                })
                .await
                .unwrap_err();
            assert_eq!(error.data, Some(json!("ACP connection closed")));
            Ok(())
        }),
    );
    drop(agent);
    timeout(Duration::from_secs(5), driver)
        .await
        .expect("connection should end")
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn nothing_the_handler_captured_outlives_the_connection() {
    // The handler holds a sender; a caller watching that channel for the end of
    // the turn only sees it close once the connection has let the handler go.
    let (events, mut watching) = mpsc::channel::<()>(1);
    let (mut agent, driver) = connect(
        Box::new(move |_| {
            let _events = events.clone();
            Box::pin(async { Ok(Value::Null) })
        }),
        boxed(|peer: Peer| async move {
            peer.request(Echo {
                value: "done".into(),
            })
            .await?;
            Ok(())
        }),
    );
    let id = agent.read().await["id"].clone();
    agent
        .write(json!({"jsonrpc": "2.0", "id": id, "result": {"echoed": "done"}}))
        .await;
    driver.await.unwrap().unwrap();
    // The agent is still connected, so only releasing the handler can close it.
    assert!(
        timeout(Duration::from_secs(5), watching.recv())
            .await
            .expect("the event channel outlived the connection")
            .is_none()
    );
}

#[tokio::test]
async fn a_tap_observes_both_directions_verbatim() {
    // What the adapter sends is what a fixture must replay, so the tap sees
    // each message exactly as framed rather than as JP decoded it.
    let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
    let collected = observed.clone();
    let tap = Tap::new(move |side, message| {
        collected
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((side, message.clone()));
    });
    let (jp_input, agent_writer) = tokio::io::duplex(8192);
    let (jp_output, agent_reader) = tokio::io::duplex(8192);
    let driver = tokio::spawn(drive(
        agent_reader,
        jp_input,
        tap,
        silent(),
        boxed(|peer: Peer| async move {
            peer.request(Echo {
                value: "out".into(),
            })
            .await?;
            Ok(())
        }),
    ));
    let mut agent = Agent {
        lines: BufReader::new(jp_output).lines(),
        writer: agent_writer,
    };
    let id = agent.read().await["id"].clone();
    agent
        .write(json!({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": "s"}}))
        .await;
    agent
        .write(json!({"jsonrpc": "2.0", "id": id, "result": {"echoed": "out"}}))
        .await;
    driver.await.unwrap().unwrap();

    let observed = observed
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    assert_eq!(observed, [
        (
            Side::Jp,
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "test/echo",
                "params": {"value": "out"},
            })
        ),
        (
            Side::Agent,
            json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {"sessionId": "s"},
            })
        ),
        (
            Side::Agent,
            json!({"jsonrpc": "2.0", "id": 1, "result": {"echoed": "out"}})
        ),
    ]);
}

#[tokio::test]
async fn a_line_that_is_not_json_does_not_end_the_connection() {
    let handled = Arc::new(AtomicUsize::new(0));
    let counted = handled.clone();
    let (mut agent, driver) = connect(
        Box::new(move |_| {
            let counted = counted.clone();
            Box::pin(async move {
                counted.fetch_add(1, Ordering::SeqCst);
                Ok(Value::Null)
            })
        }),
        boxed(|peer: Peer| async move {
            peer.request(Echo {
                value: "after".into(),
            })
            .await?;
            Ok(())
        }),
    );
    let id = agent.read().await["id"].clone();
    agent
        .writer
        .write_all(b"npm warn: not json\n")
        .await
        .unwrap();
    agent
        .write(json!({"jsonrpc": "2.0", "method": "session/update", "params": {}}))
        .await;
    agent
        .write(json!({"jsonrpc": "2.0", "id": id, "result": {"echoed": "after"}}))
        .await;
    driver.await.unwrap().unwrap();
    assert_eq!(handled.load(Ordering::SeqCst), 1);
}
