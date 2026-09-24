//! Redeeming reset credits, against a local stand-in for the account endpoints
//! and the subscription host.
//!
//! The account confirms a redemption before the responses host honours it, so
//! the request that prompted it keeps being refused for a short while.
//! The tests at the end pin what the turn does in that gap.

use std::{
    collections::BTreeMap,
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use chrono::{DateTime, TimeDelta, Utc};
use futures::StreamExt as _;
use jp_config::{AppConfig, providers::llm::AuthEntry};
use jp_conversation::{ConversationStream, thread::Thread};
use jp_credentials::{
    CATEGORY_LLM, CredentialSecret, CredentialStore, DEFAULT_COOLDOWN, InMemoryCredentialBackend,
    PROVIDER_OPENAI, SCOPE_ACCOUNT, StoredCredential,
};
use jp_storage::resource_lock::InMemoryResourceLocker;
use serde_json::Value;
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpListener, TcpStream},
};

use super::{Openai, Provider as _, resolve};
use crate::{
    EventStream, StreamError, StreamErrorKind, credential::Credential, event::Event,
    model::ModelDetails, query::ChatQuery, stream::with_idle_timeout,
};

/// Serve the credit listing and the redemption endpoint until the test ends,
/// recording each redemption body.
///
/// With `answer_consume` unset, a redemption is recorded and its connection
/// closed without a response: the backend committed, the client never heard.
async fn serve(listener: TcpListener, redemptions: Arc<Mutex<Vec<Value>>>, answer_consume: bool) {
    loop {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };

        let (target, body) = read_request(&mut socket).await;

        let reply = if target.ends_with("/wham/rate-limit-reset-credits") {
            r#"{ "credits": [], "available_count": 2 }"#
        } else if target.ends_with("/wham/rate-limit-reset-credits/consume") {
            redemptions
                .lock()
                .unwrap()
                .push(serde_json::from_str(&body).unwrap());

            if !answer_consume {
                drop(socket);
                continue;
            }

            r#"{ "code": "reset", "windows_reset": 1 }"#
        } else {
            "{}"
        };

        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: \
             {}\r\nConnection: close\r\n\r\n{reply}",
            reply.len()
        );
        socket.write_all(response.as_bytes()).await.unwrap();
    }
}

/// Read one request's target and body.
async fn read_request(socket: &mut TcpStream) -> (String, String) {
    let mut raw = vec![];
    let mut chunk = [0u8; 1024];

    loop {
        let read = socket.read(&mut chunk).await.unwrap();
        raw.extend_from_slice(&chunk[..read]);

        let text = String::from_utf8_lossy(&raw).to_string();
        let Some((head, body)) = text.split_once("\r\n\r\n") else {
            continue;
        };

        let length = head
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())?
            })
            .unwrap_or(0);

        if body.len() >= length || read == 0 {
            let target = head
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .to_owned();
            return (target, body.to_owned());
        }
    }
}

/// A conversation that runs out twice spends two credits, so its two
/// redemptions must not share an idempotency key.
#[tokio::test]
async fn test_two_redemptions_in_one_conversation_use_distinct_keys() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let redemptions = Arc::new(Mutex::new(vec![]));
    let server = tokio::spawn(serve(listener, redemptions.clone(), true));

    let mut config = AppConfig::new_test().providers.llm.openai;
    config.codex_base_url = format!("http://{address}/backend-api/codex");
    let provider = Openai::with_credential(&config, Credential::Bearer("token".to_owned()));
    let attempt = resolve::Attempt::injected(
        Credential::Bearer("token".to_owned()),
        resolve::Attribution::default(),
    );

    let first = provider.redeem_reset_credit(&attempt, "session-1").await;
    let second = provider.redeem_reset_credit(&attempt, "session-1").await;
    server.abort();

    assert_eq!(
        first.as_deref(),
        Some("subscription limit reached, redeemed a usage reset (1 left)")
    );
    assert!(second.is_some());

    let redemptions = redemptions.lock().unwrap();
    assert_eq!(redemptions.len(), 2);
    assert_ne!(
        redemptions[0]["redeem_request_id"],
        redemptions[1]["redeem_request_id"]
    );
    for body in redemptions.iter() {
        assert_ne!(body["redeem_request_id"], "session-1");
        assert!(body.get("credit_id").is_none(), "{body}");
    }
}

/// A redemption the backend received but never answered may have reopened the
/// window, so it is not reported as a refusal: that would send the request down
/// the path that records the subscription as exhausted.
#[tokio::test]
async fn test_an_unanswered_redemption_retries_the_subscription() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let redemptions = Arc::new(Mutex::new(vec![]));
    let server = tokio::spawn(serve(listener, redemptions.clone(), false));

    let mut config = AppConfig::new_test().providers.llm.openai;
    config.codex_base_url = format!("http://{address}/backend-api/codex");
    let provider = Openai::with_credential(&config, Credential::Bearer("token".to_owned()));
    let attempt = resolve::Attempt::injected(
        Credential::Bearer("token".to_owned()),
        resolve::Attribution::default(),
    );

    let notice = provider.redeem_reset_credit(&attempt, "session-1").await;
    server.abort();

    assert_eq!(
        redemptions.lock().unwrap().len(),
        1,
        "the redemption was sent"
    );
    assert_eq!(
        notice.as_deref(),
        Some(
            "subscription limit reached, redeeming a usage reset went unconfirmed; retrying the \
             subscription"
        )
    );
}

/// How many times each endpoint was called.
#[derive(Default)]
struct Calls {
    responses: AtomicUsize,
    consume: AtomicUsize,
}

/// A `429` carrying the headers that mark one spent account window, which
/// reopens `reset_after` seconds from now.
///
/// One window, so the turn is willing to spend a credit on it.
fn refused(reset_after: u32) -> String {
    let body = r#"{"error":{"message":"limit reached"}}"#;
    format!(
        "HTTP/1.1 429 Too Many Requests\r\ncontent-type: \
         application/json\r\nx-codex-primary-used-percent: 100\r\nx-codex-primary-window-minutes: \
         300\r\nx-codex-primary-reset-after-seconds: \
         {reset_after}\r\nx-codex-secondary-used-percent: 0\r\nx-codex-secondary-window-minutes: \
         0\r\nx-codex-secondary-reset-after-seconds: 0\r\nconnection: close\r\ncontent-length: \
         {}\r\n\r\n{body}",
        body.len()
    )
}

/// A `503`, which fails the request without saying anything about the
/// credential.
fn unavailable() -> String {
    let body = r#"{"error":{"message":"upstream unavailable"}}"#;
    format!(
        "HTTP/1.1 503 Service Unavailable\r\ncontent-type: application/json\r\nconnection: \
         close\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    )
}

fn json_response(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: \
         close\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    )
}

/// Serve the subscription host until the test drops the listener.
///
/// The account endpoints always hold a credit to spend.
/// Each responses request is answered by `respond`, given the lowercased
/// request head and how many responses requests came before it.
fn spawn_host(
    listener: TcpListener,
    respond: impl Fn(&str, usize) -> String + Send + 'static,
) -> Arc<Calls> {
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
                json_response(r#"{"code":"reset","windows_reset":1}"#)
            } else if head.contains("rate-limit-reset-credits") {
                json_response(r#"{"credits":[],"available_count":2}"#)
            } else {
                respond(&head, served.responses.fetch_add(1, Ordering::SeqCst))
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

fn model() -> ModelDetails {
    ModelDetails::empty("openai/gpt-5.6-sol".parse().unwrap())
}

/// A provider on the injected subscription credential, pointed at `address`.
///
/// Production allows a minute at five-second intervals.
/// The behaviour under test is the shape of the wait, not its length.
fn provider(address: SocketAddr) -> Openai {
    let mut config = AppConfig::new_test().providers.llm.openai;
    config.codex_base_url = format!("http://{address}/codex");
    Openai::with_subscription_credential(&config, "bearer-token".to_owned(), "acct-1".to_owned())
        .with_reset_timing(Duration::from_millis(250), Duration::from_millis(25))
}

/// Collect `stream`, failing rather than hanging the suite if it never ends.
async fn collect(stream: EventStream, calls: &Calls) -> Vec<Result<Event, StreamError>> {
    tokio::time::timeout(Duration::from_secs(30), stream.collect::<Vec<_>>())
        .await
        .unwrap_or_else(|_| {
            panic!(
                "the wait for a redeemed reset never ended after {} attempts",
                calls.responses.load(Ordering::SeqCst)
            )
        })
}

/// The kind of the error that ended `events`.
fn final_error(events: &[Result<Event, StreamError>]) -> StreamErrorKind {
    match events.last() {
        Some(Err(error)) => error.kind,
        other => panic!("expected the stream to end with an error, got {other:?}"),
    }
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
    let calls = spawn_host(listener, |_, _| refused(3600));

    let stream = provider(address)
        .chat_completion_stream(&model(), query())
        .await
        .unwrap();
    let events = collect(stream, &calls).await;

    // A host that never honours the reset has to end the turn.
    assert_eq!(final_error(&events), StreamErrorKind::SubscriptionExhausted);

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

/// The wait sends nothing the caller renders, but it cannot be silent: the
/// query path wraps every stream in an idle timeout, and a timed-out stream is
/// rebuilt from scratch.
///
/// The idle window here is shorter than the wait and longer than one interval,
/// the same relation as production's ten-second minimum against its minute and
/// five seconds.
#[tokio::test]
async fn test_the_wait_for_a_redeemed_reset_does_not_trip_the_idle_timeout() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let calls = spawn_host(listener, |_, _| refused(3600));

    let stream = provider(address)
        .chat_completion_stream(&model(), query())
        .await
        .unwrap();
    let events = collect(
        with_idle_timeout(stream, Duration::from_millis(100)),
        &calls,
    )
    .await;

    assert_eq!(final_error(&events), StreamErrorKind::SubscriptionExhausted);
    assert_eq!(calls.consume.load(Ordering::SeqCst), 1);
}

/// A request that fails for a reason unrelated to the credential while it waits
/// is rebuilt by the caller.
/// The rebuilt request waits on the credit already spent instead of spending a
/// second one on the same window.
#[tokio::test]
async fn test_a_rebuilt_request_waits_on_the_credit_already_spent() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    // The request after the redemption meets an outage; every other is refused.
    let calls = spawn_host(listener, |_, index| {
        if index == 1 {
            unavailable()
        } else {
            refused(3600)
        }
    });
    let provider = provider(address);

    let first = provider
        .chat_completion_stream(&model(), query())
        .await
        .unwrap();
    let first = collect(first, &calls).await;
    let Some(Err(outage)) = first.last() else {
        panic!("expected the outage to end the first request: {first:?}");
    };
    assert!(
        outage.is_retryable(),
        "the caller only rebuilds after a retryable error: {outage:?}"
    );
    assert_eq!(calls.consume.load(Ordering::SeqCst), 1);

    let second = provider
        .chat_completion_stream(&model(), query())
        .await
        .unwrap();
    let second = collect(second, &calls).await;

    assert_eq!(final_error(&second), StreamErrorKind::SubscriptionExhausted);
    assert_eq!(calls.consume.load(Ordering::SeqCst), 1);
}

/// A stored profile holding a token.
fn token_profile(token: &str) -> StoredCredential {
    StoredCredential {
        secret: CredentialSecret::Token {
            token: token.to_owned(),
        },
        account_id: Some("acct-1".to_owned()),
        email: None,
        cooldowns: BTreeMap::new(),
        needs_relogin: false,
        generation: 0,
    }
}

/// The profile's account-scoped cooldown, as stored.
fn cooldown(store: &CredentialStore, profile: &str) -> Option<DateTime<Utc>> {
    store
        .load()
        .unwrap()
        .profiles(CATEGORY_LLM, PROVIDER_OPENAI)?
        .get(profile)?
        .cooldowns
        .get(SCOPE_ACCOUNT)
        .copied()
}

/// The credit spent on one profile says nothing about the next one in the
/// chain.
/// That profile's reported reset is still the best evidence of when it reopens,
/// so it is recorded as reported rather than replaced by the short default a
/// redeemed profile gets.
#[tokio::test]
async fn test_a_redemption_on_one_profile_leaves_the_next_profiles_reset_alone() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let calls = spawn_host(listener, |head, _| {
        if head.contains("bearer bearer-2") {
            refused(300)
        } else {
            refused(3600)
        }
    });

    let store = CredentialStore::new(
        Arc::new(InMemoryCredentialBackend::new()),
        Arc::new(InMemoryResourceLocker::default()),
    );
    store
        .mutate(|document| {
            document.insert_profile(
                CATEGORY_LLM,
                PROVIDER_OPENAI,
                "first",
                token_profile("bearer-1"),
            );
            document.insert_profile(
                CATEGORY_LLM,
                PROVIDER_OPENAI,
                "second",
                token_profile("bearer-2"),
            );
            Ok(())
        })
        .unwrap();

    let mut config = AppConfig::new_test().providers.llm.openai;
    config.codex_base_url = format!("http://{address}/codex");
    config.auth = vec![
        AuthEntry::Subscription(Some("first".to_owned())),
        AuthEntry::Subscription(Some("second".to_owned())),
    ];
    let provider = Openai::with_store(&config, store.clone())
        .with_reset_timing(Duration::from_millis(100), Duration::from_millis(25));

    let before = Utc::now();
    let stream = provider
        .chat_completion_stream(&model(), query())
        .await
        .unwrap();
    let events = collect(stream, &calls).await;
    let after = Utc::now();

    assert_eq!(final_error(&events), StreamErrorKind::SubscriptionExhausted);

    // One credit for the whole request, spent on the first profile.
    assert_eq!(calls.consume.load(Ordering::SeqCst), 1);

    let first = cooldown(&store, "first").unwrap();
    assert!(
        first >= before + DEFAULT_COOLDOWN && first <= after + DEFAULT_COOLDOWN,
        "the redeemed profile gets the short default: {first}"
    );

    let second = cooldown(&store, "second").unwrap();
    let reported = TimeDelta::seconds(300);
    assert!(
        second >= before + reported - TimeDelta::seconds(1)
            && second <= after + reported + TimeDelta::seconds(1),
        "the next profile keeps its reported reset, {before} + 300s: {second}"
    );
}
