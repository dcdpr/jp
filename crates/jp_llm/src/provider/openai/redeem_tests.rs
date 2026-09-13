//! A redeemed usage reset keeps the turn alive while it propagates.
//!
//! The account confirms the redemption before the responses host honours it, so
//! the request that prompted it keeps being refused for a short while.
//! These tests pin what the turn does in that gap.

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use futures::StreamExt as _;
use jp_config::AppConfig;
use jp_conversation::{ConversationStream, thread::Thread};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::TcpListener,
};

use super::{Openai, Provider as _};
use crate::{model::ModelDetails, query::ChatQuery};

/// How many times each endpoint was called.
#[derive(Default)]
struct Calls {
    responses: AtomicUsize,
    consume: AtomicUsize,
}

/// A `429` carrying the headers that mark one spent account window.
///
/// One window, so the turn is willing to spend a credit on it; a reset an hour
/// out, so recording that timing would be visible as an hour-long cooldown.
const REFUSED: &str = concat!(
    "HTTP/1.1 429 Too Many Requests\r\n",
    "content-type: application/json\r\n",
    "x-codex-primary-used-percent: 100\r\n",
    "x-codex-primary-window-minutes: 300\r\n",
    "x-codex-primary-reset-after-seconds: 3600\r\n",
    "x-codex-secondary-used-percent: 0\r\n",
    "x-codex-secondary-window-minutes: 0\r\n",
    "x-codex-secondary-reset-after-seconds: 0\r\n",
    "connection: close\r\n",
    "content-length: 41\r\n\r\n",
    r#"{"error":{"message":"limit reached"}}"#,
);

fn json_response(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: \
         close\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    )
}

/// Serve the subscription host until the test drops the listener.
///
/// Every responses request is refused, so the turn keeps waiting for the reset
/// it redeemed for as long as it is willing to.
fn spawn_host(listener: TcpListener) -> Arc<Calls> {
    let calls = Arc::new(Calls::default());
    let served = calls.clone();

    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };

            // Read to the end of the headers, then drain the declared body, so
            // the client sees a complete exchange rather than a reset peer.
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                match socket.read(&mut byte).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => request.push(byte[0]),
                }
            }

            let head = String::from_utf8_lossy(&request).to_ascii_lowercase();
            let length: usize = head
                .split("content-length:")
                .nth(1)
                .and_then(|rest| rest.split("\r\n").next())
                .and_then(|value| value.trim().parse().ok())
                .unwrap_or(0);
            let mut body = vec![0u8; length];
            drop(socket.read_exact(&mut body).await);

            let response = if head.contains("rate-limit-reset-credits/consume") {
                served.consume.fetch_add(1, Ordering::SeqCst);
                json_response(r#"{"success":true}"#)
            } else if head.contains("rate-limit-reset-credits") {
                json_response(r#"{"credits":[{"id":"credit-1"},{"id":"credit-2"}]}"#)
            } else {
                served.responses.fetch_add(1, Ordering::SeqCst);
                REFUSED.to_owned()
            };

            drop(socket.write_all(response.as_bytes()).await);
            drop(socket.shutdown().await);
        }
    });

    calls
}

fn query() -> ChatQuery {
    ChatQuery::from(Thread {
        system_prompt: None,
        sections: vec![],
        attachments: vec![],
        events: ConversationStream::new_test().with_turn("hello"),
    })
}

/// A turn that spends a credit and is refused again does not give the reset one
/// immediate retry and then abandon the turn: it keeps asking while the
/// redemption propagates.
///
/// The bound matters as much as the retrying.
/// A window the credit does not cover is refused identically and never
/// resolves, so the wait has to end.
#[tokio::test]
async fn test_a_redeemed_reset_is_waited_out_and_then_given_up_on() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let calls = spawn_host(listener);

    let mut config = AppConfig::new_test().providers.llm.openai;
    config.codex_base_url = format!("http://{address}/codex");
    // Production allows a minute at five-second intervals. The behaviour under
    // test is the shape of the wait, not its length.
    let provider = Openai::with_subscription_credential(
        &config,
        "bearer-token".to_owned(),
        "acct-1".to_owned(),
    )
    .with_reset_timing(Duration::from_millis(250), Duration::from_millis(25));

    let stream = provider
        .chat_completion_stream(
            &ModelDetails::empty("openai/gpt-5.6-sol".parse().unwrap()),
            query(),
        )
        .await
        .unwrap();

    // Far longer than the shortened budget, so it fails a wait that never ends
    // rather than hanging the suite.
    let events = tokio::time::timeout(Duration::from_secs(30), stream.collect::<Vec<_>>())
        .await
        .unwrap_or_else(|_| {
            panic!(
                "the wait for a redeemed reset never ended after {} attempts",
                calls.responses.load(Ordering::SeqCst)
            )
        });

    assert!(
        events.last().is_some_and(Result::is_err),
        "a host that never honours the reset has to end the turn: {events:?}"
    );

    // One credit, however many times the request is refused.
    assert_eq!(calls.consume.load(Ordering::SeqCst), 1);

    // Two attempts is the old behaviour: the one that hit the limit, and the
    // single immediate retry after redeeming.
    let attempts = calls.responses.load(Ordering::SeqCst);
    assert!(
        attempts > 2,
        "expected the turn to keep retrying while the reset propagated, got {attempts} attempts"
    );
}
