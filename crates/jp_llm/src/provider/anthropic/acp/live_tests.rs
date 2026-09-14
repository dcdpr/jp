//! Opt-in qualification against the installed subscription-backed runtime.

use std::{
    env, fmt,
    sync::{Arc, Mutex},
    time::Duration,
};

use camino::Utf8PathBuf;
use datetime_literal::datetime;
use futures::StreamExt as _;
use jp_config::{
    AppConfig, assistant::request::CachePolicy, model::parameters::ReasoningConfig,
    providers::llm::AuthEntry,
};
use jp_conversation::{
    ConversationStream,
    event::{ChatRequest, ChatResponse, ConversationEvent},
    thread::ThreadBuilder,
};
use serde_json::{Value, json};
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

use super::{super::Anthropic, model_details};
use crate::{
    Provider,
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
    // A long stable prefix clears the model's cache minimum.
    let reference = "Invoice INV-1042: amount 125, status paid.\n".repeat(2000);
    ThreadBuilder::new()
        .with_system_prompt(format!(
            "Qualification {tag}. Use the supplied invoice history.\n{reference}"
        ))
        .with_events(events)
        .build()
        .unwrap()
        .into()
}

async fn request(provider: &Anthropic, query: ChatQuery, context: QueryContext) -> Value {
    let capture = UsageCapture::default();
    let subscriber = registry().with(capture.clone());
    tokio::time::timeout(
        Duration::from_mins(2),
        async {
            let model = model_details(&"claude-opus-5".parse().unwrap());
            let mut stream = provider
                .start_query(&model, query, context)
                .await
                .unwrap()
                .events;
            let mut finish = None;
            let mut response = String::new();
            while let Some(event) = stream.next().await {
                match event.unwrap() {
                    Event::Part {
                        part: EventPart::Message(text),
                        ..
                    } => response.push_str(&text),
                    Event::Finished(reason) => finish = Some(reason),
                    _ => {}
                }
            }
            assert_eq!(finish, Some(FinishReason::Completed));
            assert_eq!(response.trim(), "INV-1042");
            capture.snapshot().expect("runtime did not report usage")
        }
        .with_subscriber(subscriber),
    )
    .await
    .expect("live ACP query exceeded two minutes")
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

#[tokio::test]
#[ignore = "Consumes subscription allowance; requires runtime setup and no-overage confirmation"]
async fn live_cache_reconstruction() {
    assert_eq!(
        env::var("JP_ACP_LIVE_NO_OVERAGE").as_deref(),
        Ok("1"),
        "Confirm paid Usage credits are disabled with JP_ACP_LIVE_NO_OVERAGE=1"
    );
    // A caller-provided run tag prevents a prior qualification run from warming
    // this run's initial prefix. It remains constant within each comparison.
    let tag = env::var("JP_ACP_CACHE_RUN").expect("Set a fresh JP_ACP_CACHE_RUN tag");
    assert!(!tag.is_empty());
    let mut config = AppConfig::new_test().providers.llm.anthropic;
    config.auth = vec![AuthEntry::Subscription(None)];
    let provider = Anthropic::new(&config).unwrap();
    let context = QueryContext {
        root: Utf8PathBuf::from_path_buf(env::current_dir().unwrap()).unwrap(),
        mcp_endpoint: None,
        invocation: None,
    };
    let short = query(CachePolicy::Short, &tag);
    let first = request(&provider, short.clone(), context.clone()).await;
    println!(
        "{}",
        json!({"case":"initial_runtime_managed","usage":first})
    );
    assert!(
        tokens(&first, "cache_creation_input_tokens") > 0,
        "runtime-managed caching did not create an entry"
    );
    let repeat = request(&provider, short, context.clone()).await;
    println!(
        "{}",
        json!({"case":"reconstructed_runtime_managed","usage":repeat})
    );
    assert_ne!(first["native_session_id"], repeat["native_session_id"]);
    assert!(
        tokens(&repeat, "cache_read_input_tokens") > 0,
        "reconstruction did not reuse any cache"
    );

    let off = request(&provider, query(CachePolicy::Off, &tag), context).await;
    println!("{}", json!({"case":"off","usage":off}));
    assert_eq!(tokens(&off, "cache_read_input_tokens"), 0);
    assert_eq!(tokens(&off, "cache_creation_input_tokens"), 0);
}
