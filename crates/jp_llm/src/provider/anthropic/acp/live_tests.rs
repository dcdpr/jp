//! Opt-in qualification against the installed subscription-backed runtime.

use std::{env, time::Duration};

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

use super::{super::Anthropic, model_details, usage::METADATA_KEY};
use crate::{
    Provider,
    event::{Event, EventPart, FinishReason},
    query::{ChatQuery, QueryContext},
};

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
    // A long stable prefix clears the model's cache minimum. Policy-specific
    // tags keep the long-cache control from reusing a short-cache entry.
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
    tokio::time::timeout(Duration::from_mins(2), async {
        let model = model_details(&"claude-opus-5".parse().unwrap()).unwrap();
        let mut stream = provider
            .start_query(&model, query, context)
            .await
            .unwrap()
            .events;
        let mut snapshot = None;
        let mut finish = None;
        let mut response = String::new();
        while let Some(event) = stream.next().await {
            match event.unwrap() {
                Event::Part {
                    part: EventPart::Message(text),
                    ..
                } => response.push_str(&text),
                Event::Flush { metadata, .. } => {
                    if let Some(value) = metadata.get(METADATA_KEY) {
                        snapshot = Some(value.clone());
                    }
                }
                Event::Finished(reason) => finish = Some(reason),
                _ => {}
            }
        }
        assert_eq!(finish, Some(FinishReason::Completed));
        assert_eq!(response.trim(), "INV-1042");
        snapshot.expect("runtime did not report usage")
    })
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

fn writes(snapshot: &Value, duration: &str) -> u64 {
    snapshot["requests"]
        .as_object()
        .expect("missing main request usage")
        .values()
        .map(|usage| {
            usage["cache_creation"][duration]
                .as_u64()
                .expect("runtime omitted cache TTL counters")
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
    };
    let short = query(CachePolicy::Short, &tag);
    let first = request(&provider, short.clone(), context.clone()).await;
    println!("{}", json!({"case":"initial_short","usage":first}));
    assert!(
        writes(&first, "ephemeral_5m_input_tokens") > 0,
        "initial short-cache request did not write a five-minute entry"
    );
    assert_eq!(writes(&first, "ephemeral_1h_input_tokens"), 0);
    let repeat = request(&provider, short, context.clone()).await;
    println!("{}", json!({"case":"reconstructed_short","usage":repeat}));
    assert_ne!(first["native_session_id"], repeat["native_session_id"]);
    assert!(
        tokens(&repeat, "cache_read_input_tokens") > 0,
        "reconstruction did not reuse any cache"
    );

    let long = request(
        &provider,
        query(CachePolicy::Long, &format!("{tag}-long")),
        context.clone(),
    )
    .await;
    println!("{}", json!({"case":"long","usage":long}));
    assert!(
        writes(&long, "ephemeral_1h_input_tokens") > 0,
        "long-cache control did not write a one-hour entry"
    );
    assert_eq!(writes(&long, "ephemeral_5m_input_tokens"), 0);
    let off = request(&provider, query(CachePolicy::Off, &tag), context).await;
    println!("{}", json!({"case":"off","usage":off}));
    assert_eq!(tokens(&off, "cache_read_input_tokens"), 0);
    assert_eq!(tokens(&off, "cache_creation_input_tokens"), 0);
}
