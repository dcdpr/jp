//! Per-provider wire-request snapshots for conversation compaction.
//!
//! Builds a conversation stream with compaction overlays, runs each provider's
//! real request builder (no network, no recorded responses), and snapshots the
//! serialized request that would go on the wire.
//! A scenario is a single provider-parameterized function;
//! `request_for_all_providers!` fans it out across every provider, mirroring
//! the `test_all_providers!` pattern in `provider_tests.rs`.
//!
//! Every scenario shares one base conversation (`base_stream`) and differs only
//! in the compaction overlay it appends, so the snapshots isolate each policy's
//! effect on the request.

use chrono::{DateTime, Duration, TimeZone as _, Utc};
use jp_config::{
    AppConfig,
    model::{
        id::{ModelIdConfig, ModelIdOrAliasConfig, ProviderId},
        parameters::ReasoningConfig,
    },
    providers::llm::LlmProviderConfig,
};
use jp_conversation::{
    Compaction, ConversationStream, ReasoningPolicy, SummaryPolicy, ToolCallPolicy,
    event::{
        ChatRequest, ChatResponse, ConversationEvent, ToolCallRequest, ToolCallResponse, TurnStart,
    },
    thread::ThreadBuilder,
};
use jp_test::Result;

use super::build_request_value;
use crate::{query::ChatQuery, test::test_model_details};

/// Fixed timestamp for every event and overlay, so snapshots are deterministic.
fn ts() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap()
}

/// Provider config whose API-key env vars point at a variable present in the
/// test environment, so each provider constructs offline without real
/// credentials.
/// Mirrors the dummy-key handling in the VCR harness.
fn provider_config() -> LlmProviderConfig {
    let env = if cfg!(windows) { "USERNAME" } else { "USER" }.to_owned();
    let mut config = LlmProviderConfig::default();
    config.anthropic.api_key_env = env.clone();
    config.cerebras.api_key_env = env.clone();
    config.google.api_key_env = env.clone();
    config.openai.api_key_env = env.clone();
    config.openrouter.api_key_env = env;
    config
}

/// Deterministic base config: reasoning off at the model level (the
/// conversation reasoning *events* are what compaction strips, independent of
/// this), model pinned to the provider under test.
fn base_config(provider: ProviderId) -> AppConfig {
    let mut config = AppConfig::new_test();
    config.assistant.model.parameters.reasoning = Some(ReasoningConfig::Off);
    config.assistant.model.id = ModelIdOrAliasConfig::Id(ModelIdConfig {
        provider,
        name: "test".parse().unwrap(),
    });
    config
}

/// A four-turn conversation exercising every content type a compaction policy
/// can touch:
///
/// - turn 0: a reasoning event,
/// - turn 1: a tool call request/response pair,
/// - turn 2: a plain message exchange,
/// - turn 3: a trailing pending request (outside every scenario's range).
fn base_stream(provider: ProviderId) -> ConversationStream {
    let ts = ts();
    let mut stream = ConversationStream::new(base_config(provider).into()).with_created_at(ts);

    let mut tool_args = serde_json::Map::new();
    tool_args.insert("path".into(), "notes.md".into());

    stream.extend([
        ConversationEvent::new(TurnStart, ts),
        ConversationEvent::new(ChatRequest::from("What is the capital of France?"), ts),
        ConversationEvent::new(
            ChatResponse::reasoning("The user wants a capital city."),
            ts,
        ),
        ConversationEvent::new(ChatResponse::message("Paris."), ts),
        ConversationEvent::new(TurnStart, ts),
        ConversationEvent::new(ChatRequest::from("Read my notes file."), ts),
        ConversationEvent::new(
            ToolCallRequest::new("call_1".to_owned(), "read_file".to_owned(), tool_args),
            ts,
        ),
        ConversationEvent::new(
            ToolCallResponse {
                id: "call_1".to_owned(),
                result: Ok("- buy milk\n- call dentist".to_owned()),
            },
            ts,
        ),
        ConversationEvent::new(
            ChatResponse::message("Your notes mention milk and the dentist."),
            ts,
        ),
        ConversationEvent::new(TurnStart, ts),
        ConversationEvent::new(ChatRequest::from("And the capital of Germany?"), ts),
        ConversationEvent::new(ChatResponse::message("Berlin."), ts),
        ConversationEvent::new(TurnStart, ts),
        ConversationEvent::new(ChatRequest::from("And of Italy?"), ts),
    ]);

    stream
}

/// Apply a compaction overlay to the base stream, build the provider request,
/// and snapshot it at `tests/fixtures/<provider>/compaction/<name>.snap`.
fn snapshot(
    provider: ProviderId,
    name: &str,
    compact: impl FnOnce(&mut ConversationStream),
) -> Result {
    let mut stream = base_stream(provider);
    compact(&mut stream);

    let thread = ThreadBuilder::new().with_events(stream).build().unwrap();
    let request = build_request_value(
        provider,
        &provider_config(),
        &test_model_details(provider),
        ChatQuery::from(thread),
    )?;

    let path = format!(
        "{}/tests/fixtures/{}/compaction",
        env!("CARGO_MANIFEST_DIR"),
        provider.as_str(),
    );

    insta::with_settings!({ snapshot_path => path, prepend_module_to_snapshot => false }, {
        insta::assert_json_snapshot!(name, request);
    });

    Ok(())
}

/// No compaction: the full conversation, as a control for every other scenario.
fn baseline(provider: ProviderId, name: &str) -> Result {
    snapshot(provider, name, |_| {})
}

/// Reasoning events in the compacted range are dropped from the request.
fn reasoning_strip(provider: ProviderId, name: &str) -> Result {
    snapshot(provider, name, |stream| {
        stream.add_compaction(Compaction::new(0, 0).with_reasoning(ReasoningPolicy::Strip));
    })
}

/// The compacted range collapses to a single synthetic request/response pair
/// carrying a pre-computed summary; the trailing turn survives.
fn summary(provider: ProviderId, name: &str) -> Result {
    snapshot(provider, name, |stream| {
        stream.add_compaction(
            Compaction::new(0, 2).with_summary(SummaryPolicy {
                summary: "Earlier: the user asked about France's capital and had their notes read."
                    .to_owned(),
            }),
        );
    })
}

/// Tool call request arguments in the compacted range are blanked to `{}`; the
/// response is untouched.
fn tool_strip_request(provider: ProviderId, name: &str) -> Result {
    snapshot(provider, name, |stream| {
        stream.add_compaction(
            Compaction::new(1, 1).with_tool_calls(ToolCallPolicy::Strip {
                request: true,
                response: false,
            }),
        );
    })
}

/// Tool call responses in the compacted range are replaced with a status line;
/// the request is untouched.
fn tool_strip_response(provider: ProviderId, name: &str) -> Result {
    snapshot(provider, name, |stream| {
        stream.add_compaction(
            Compaction::new(1, 1).with_tool_calls(ToolCallPolicy::Strip {
                request: false,
                response: true,
            }),
        );
    })
}

/// Both the request arguments and the response content are stripped.
fn tool_strip_both(provider: ProviderId, name: &str) -> Result {
    snapshot(provider, name, |stream| {
        stream.add_compaction(
            Compaction::new(1, 1).with_tool_calls(ToolCallPolicy::Strip {
                request: true,
                response: true,
            }),
        );
    })
}

/// The tool call request/response pair is removed entirely from the request.
fn tool_omit(provider: ProviderId, name: &str) -> Result {
    snapshot(provider, name, |stream| {
        stream.add_compaction(Compaction::new(1, 1).with_tool_calls(ToolCallPolicy::Omit));
    })
}

/// Two summaries with overlapping ranges: the later-timestamped one wins the
/// shared turn, so turn 0 keeps summary A and turns 1-2 resolve to summary B.
fn summary_overlap(provider: ProviderId, name: &str) -> Result {
    snapshot(provider, name, |stream| {
        let mut a = Compaction::new(0, 1).with_summary(SummaryPolicy {
            summary: "Summary A: France's capital and the start of the notes lookup.".to_owned(),
        });
        a.timestamp = ts();

        let mut b = Compaction::new(1, 2).with_summary(SummaryPolicy {
            summary: "Summary B: the notes lookup and Germany's capital.".to_owned(),
        });
        b.timestamp = ts() + Duration::seconds(1);

        stream.add_compaction(a);
        stream.add_compaction(b);
    })
}

macro_rules! request_for_all_providers {
    ($($scenario:ident),* $(,)?) => {
        mod anthropic  { use super::*; $(request_for_all_providers!(@case $scenario, ProviderId::Anthropic);)* }
        mod cerebras   { use super::*; $(request_for_all_providers!(@case $scenario, ProviderId::Cerebras);)* }
        mod google     { use super::*; $(request_for_all_providers!(@case $scenario, ProviderId::Google);)* }
        mod llamacpp   { use super::*; $(request_for_all_providers!(@case $scenario, ProviderId::Llamacpp);)* }
        mod ollama     { use super::*; $(request_for_all_providers!(@case $scenario, ProviderId::Ollama);)* }
        mod openai     { use super::*; $(request_for_all_providers!(@case $scenario, ProviderId::Openai);)* }
        mod openrouter { use super::*; $(request_for_all_providers!(@case $scenario, ProviderId::Openrouter);)* }
    };
    (@case $scenario:ident, $provider:expr) => {
        paste::paste! {
            #[test]
            fn [< test_ $scenario >]() -> Result {
                $scenario($provider, stringify!($scenario))
            }
        }
    };
}

request_for_all_providers![
    baseline,
    reasoning_strip,
    summary,
    tool_strip_request,
    tool_strip_response,
    tool_strip_both,
    tool_omit,
    summary_overlap,
];
