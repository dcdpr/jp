//! The subscription conversation, recorded once and replayed thereafter.
//!
//! Without `RECORD`, these replay `tests/fixtures/acp/live.jsonl`: no adapter
//! is spawned, no allowance is spent, and a missing recording fails rather than
//! passing quietly.
//! With `RECORD=1` they reach the installed runtime, measure what its caching
//! actually does, and write the recording back.
//! Same switch and same fixture root as the workspace's HTTP cassettes.
//!
//! The recording is the only non-circular evidence that [`super::schema`]
//! spells the protocol's field names correctly: those types are written by
//! hand, and a fixture written from the same reading of the spec would agree
//! with them whether or not the adapter does.
//!
//! What it does not cover is a tool host, so it carries no
//! `session/request_permission` and no tool-call session update — the two
//! messages whose fields are most hand-written.
//! Capturing those needs a run with tools available, which today means a real
//! `RECORD=1 jp query`.

use std::{
    env, fmt,
    sync::{Arc, Mutex},
    time::Duration,
};

use camino::Utf8PathBuf;
use datetime_literal::datetime;
use jp_config::{AppConfig, assistant::request::CachePolicy, model::parameters::ReasoningConfig};
use jp_conversation::{
    ConversationStream,
    event::{ChatRequest, ChatResponse, ConversationEvent},
    thread::ThreadBuilder,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tracing::{
    Event as TracingEvent, Subscriber,
    field::{Field, Visit},
    instrument::WithSubscriber as _,
};
use tracing_subscriber::{
    Layer,
    layer::{Context, SubscriberExt as _},
    registry,
};
use uuid::Uuid;

use super::{
    cassette::{self, Framed, Recorded},
    inspect, model_details, options,
    schema::agent_method,
    transcript::PreparedRequest,
    transport::{self, NativeArtifact, Transport},
};
use crate::{
    event::{Event, EventPart, FinishReason},
    query::{ChatQuery, QueryContext},
};

/// Test-only observer of the production diagnostic event, never conversation
/// metadata.
#[derive(Clone, Default)]
pub(super) struct UsageCapture(Arc<Mutex<Option<Value>>>);

impl UsageCapture {
    pub(super) fn snapshot(&self) -> Option<Value> {
        self.0.lock().unwrap().clone()
    }
}

struct UsageVisitor(Option<Value>);
impl Visit for UsageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "usage" {
            self.0 = Some(
                serde_json::from_str(&format!("{value:?}")).expect("usage diagnostic must be JSON"),
            );
        }
    }
}

impl<S: Subscriber> Layer<S> for UsageCapture {
    fn on_event(&self, event: &TracingEvent<'_>, _: Context<'_, S>) {
        if event.metadata().target() != "jp_llm::provider::anthropic::acp::transport" {
            return;
        }
        let mut visitor = UsageVisitor(None);
        event.record(&mut visitor);
        if let Some(value) = visitor.0 {
            *self.0.lock().unwrap() = Some(value);
        }
    }
}

fn query(policy: CachePolicy, tag: &str) -> ChatQuery {
    let mut config = AppConfig::new_test();
    config.assistant.model.id = "anthropic/claude-opus-5".parse().unwrap();
    config.assistant.model.parameters.max_tokens = Some(128);
    config.assistant.model.parameters.reasoning = Some(ReasoningConfig::Off);
    config.assistant.request.cache = policy;
    let timestamp = datetime!(2026-09-11 12:00:00 Z);
    let mut events = ConversationStream::new(config.into()).with_created_at(timestamp);
    events.extend([
        ConversationEvent::new(
            ChatRequest::from("The active invoice is INV-1042."),
            timestamp,
        ),
        ConversationEvent::new(ChatResponse::message("Acknowledged."), timestamp),
        ConversationEvent::new(
            ChatRequest::from("Return only the active invoice ID."),
            timestamp,
        ),
    ]);
    // A stable prefix long enough for the model to cache it, and no longer:
    // the recording holds one copy per connection, so every line here costs
    // three in a committed file. Recording with 2000 lines produced 40920
    // cacheable tokens, far past any minimum.
    //
    // A prefix that stopped qualifying would fail the next recording rather
    // than pass quietly, since the first conversation asserts the cache entry
    // was created.
    let reference = "Invoice INV-1042: amount 125, status paid.\n".repeat(400);
    ThreadBuilder::new()
        .with_system_prompt(format!(
            "Qualification {tag}. Use the supplied invoice history.\n{reference}"
        ))
        .with_events(events)
        .build()
        .unwrap()
        .into()
}

fn context() -> QueryContext {
    QueryContext {
        root: Utf8PathBuf::from_path_buf(env::current_dir().unwrap()).unwrap(),
        mcp_endpoint: None,
        invocation: None,
    }
}

/// Run one conversation and return the usage the runtime reported.
///
/// Everything above the transport is shared by both modes: the same request,
/// the same driver, the same assertions on what came back.
async fn conversation(
    policy: CachePolicy,
    tag: &str,
    prepare: impl FnOnce(&PreparedRequest) -> (NativeArtifact, Box<dyn Transport>),
) -> Value {
    let model = model_details(&"claude-opus-5".parse().unwrap());
    let prepared = PreparedRequest::new(&model, query(policy, tag)).unwrap();
    let (artifact, transport) = prepare(&prepared);
    let environment = options::environment(&prepared, policy);

    let capture = UsageCapture::default();
    let subscriber = registry().with(capture.clone());
    let (sender, mut receiver) = mpsc::channel(64);
    let driver = transport::drive(
        prepared,
        context(),
        vec![],
        environment,
        artifact,
        transport,
        sender,
    )
    .with_subscriber(subscriber);

    let collect = async {
        let mut finish = None;
        let mut response = String::new();
        while let Some(event) = receiver.recv().await {
            match event.unwrap() {
                Event::Part {
                    part: EventPart::Message(text),
                    ..
                } => response.push_str(&text),
                Event::Finished(reason) => finish = Some(reason),
                _ => {}
            }
        }
        (finish, response)
    };

    let (result, (finish, response)) = tokio::time::timeout(Duration::from_mins(2), async {
        tokio::join!(driver, collect)
    })
    .await
    .expect("the ACP conversation exceeded two minutes");

    result.unwrap();
    assert_eq!(finish, Some(FinishReason::Completed));
    assert_eq!(response.trim(), "INV-1042");
    capture.snapshot().expect("runtime did not report usage")
}

/// Reach the installed adapter, writing the transcript it resumes from and
/// recording every message it exchanges.
fn live(
    policy: CachePolicy,
) -> impl FnOnce(&PreparedRequest) -> (NativeArtifact, Box<dyn Transport>) {
    move |prepared| {
        let launch =
            transport::launch(prepared, &context(), policy, None).expect("a prepared launch");
        (
            launch.artifact,
            Box::new(transport::Spawned {
                command: launch.command,
                tap: cassette::tap("live"),
            }),
        )
    }
}

/// The session the recording resumed, when it resumed one.
///
/// Read back out of the `session/load` JP sent, which is the only place the id
/// appears before the agent starts answering.
fn recorded_session(script: &[Framed]) -> Option<Uuid> {
    script
        .iter()
        .find(|entry| entry.message["method"] == json!(agent_method::SESSION_LOAD))
        .and_then(|entry| entry.message["params"]["sessionId"].as_str())
        .and_then(|id| id.parse().ok())
}

/// Answer from one recorded connection, touching neither the network nor the
/// Claude directory.
///
/// The transcript is not written, because nothing here reads one.
/// The session id is the recording's own: JP drops every SDK notification whose
/// session is not the one it opened, so a replay that minted a fresh id would
/// decode an empty conversation and report the result as missing.
///
/// A recording with no `session/load` had no history to resume, and leaving the
/// id unset is what makes the replay open with `session/new` as it did.
fn replayed(
    script: Vec<Framed>,
) -> impl FnOnce(&PreparedRequest) -> (NativeArtifact, Box<dyn Transport>) {
    move |_| {
        (
            NativeArtifact {
                session: recorded_session(&script),
                path: None,
            },
            Box::new(Recorded(script)),
        )
    }
}

/// The usage without its session id, which a live run mints per connection and
/// a replay adopts from the recording, so it differs between them without
/// saying anything about either.
fn anonymous(usage: &Value) -> Value {
    let mut usage = usage.clone();
    if let Some(object) = usage.as_object_mut() {
        object.insert("native_session_id".into(), json!("[session]"));
    }
    usage
}

/// What keeps one run's cache measurements out of the next run's way.
///
/// A recording needs a tag nothing has seen, so its first conversation meets a
/// cold cache and the entry it creates is its own; all three share it, so the
/// second meets what the first left.
/// Generated rather than asked for, since "fresh every time" is a property a
/// machine keeps and a person forgets.
///
/// A replay measures no cache, so it takes a fixed tag and stays reproducible.
fn tag(recording: bool) -> String {
    if recording {
        Uuid::new_v4().to_string()
    } else {
        "replay".to_owned()
    }
}

/// One conversation, answered from `script` when there is one and by the
/// runtime when there is not.
async fn attempt(policy: CachePolicy, tag: &str, script: Option<Vec<Framed>>) -> Value {
    match script {
        Some(script) => conversation(policy, tag, replayed(script)).await,
        None => conversation(policy, tag, live(policy)).await,
    }
}

fn tokens(snapshot: &Value, key: &str) -> u64 {
    let requests = snapshot["requests"]
        .as_object()
        .expect("missing main request usage");
    assert!(!requests.is_empty(), "runtime omitted main request usage");
    requests
        .values()
        .map(|usage| {
            usage[key]
                .as_u64()
                .expect("runtime omitted a token counter")
        })
        .sum()
}

/// Three conversations: a cached one, the same one again, and one with caching
/// off.
///
/// Replayed by default, so this costs nothing and proves the recorded traffic
/// still decodes into the events and usage JP reports.
/// Under `RECORD=1` it reaches the runtime instead, and adds the measurements
/// only a live service can answer — that an entry was created, that a second
/// session reconstructed it, and that turning caching off stops both.
/// Those cannot be replayed: a fixture compared against itself passes whatever
/// it contains.
#[tokio::test]
async fn cache_reconstruction() {
    let recording = cassette::recording();
    if recording {
        // Reaching the runtime with a stale adapter or a lapsed login produces
        // a failure that reads as a protocol problem, so check first.
        inspect(None)
            .await
            .expect("a qualified adapter and an active login");
    }
    let tag = tag(recording);

    let mut scripts = if recording {
        Vec::new()
    } else {
        let read = cassette::read("live").unwrap_or_else(|error| panic!("{error}"));
        let scripts = cassette::connections(read);
        assert_eq!(
            scripts.len(),
            3,
            "the recording should hold one connection per request"
        );
        scripts
    }
    .into_iter();

    let first = attempt(CachePolicy::Short, &tag, scripts.next()).await;
    let repeat = attempt(CachePolicy::Short, &tag, scripts.next()).await;
    let off = attempt(CachePolicy::Off, &tag, scripts.next()).await;

    insta::assert_json_snapshot!(
        "cache_reconstruction_usage",
        json!({
            "initial": anonymous(&first),
            "reconstructed": anonymous(&repeat),
            "off": anonymous(&off),
        })
    );

    if !recording {
        return;
    }

    println!("{}", json!({"case": "initial", "usage": first}));
    println!("{}", json!({"case": "reconstructed", "usage": repeat}));
    println!("{}", json!({"case": "off", "usage": off}));
    assert!(
        tokens(&first, "cache_creation_input_tokens") > 0,
        "runtime-managed caching did not create an entry"
    );
    assert_ne!(first["native_session_id"], repeat["native_session_id"]);
    assert!(
        tokens(&repeat, "cache_read_input_tokens") > 0,
        "reconstruction did not reuse any cache"
    );
    assert_eq!(tokens(&off, "cache_read_input_tokens"), 0);
    assert_eq!(tokens(&off, "cache_creation_input_tokens"), 0);
}
