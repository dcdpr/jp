use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use jp_config::{
    PartialAppConfig,
    model::{
        id::{ModelIdConfig, ModelIdOrAliasConfig, ProviderId},
        parameters::ReasoningConfig,
    },
};
use jp_conversation::{
    Compaction, SummaryPolicy,
    event::{ToolCallRequest, ToolCallResponse, TurnStart},
    stream::ConversationEventWithConfig,
};

use super::*;
use crate::{
    event::{Event, FinishReason},
    provider::mock::MockProvider,
};

const WINDOW: u32 = 2_000;
const BATCH_SIZE: usize = 5;
const SMALL_BATCH_SIZE: usize = 2;
const OVERLONG_SUMMARY_LEN: usize = 100;
const TRUNCATED_PREFIX_LEN: usize = MAX_SUMMARY_CHARS - 1;
const OVERLONG_MESSAGE_LEN: usize = MAX_MESSAGE_CHARS + 500;
const ASSISTANT_MAX_TOKENS: u32 = 64_000;

fn id(value: &str) -> EventId {
    EventId::fixed(value)
}

fn entry(id: &str, kind: impl Into<EventKind>) -> (String, ConversationEvent) {
    (
        id.to_owned(),
        ConversationEvent::new(kind, DateTime::<Utc>::UNIX_EPOCH),
    )
}

fn stream(entries: Vec<(String, ConversationEvent)>) -> ConversationStream {
    let mut stream = ConversationStream::new_test();
    let config = stream.base_config().to_partial();
    stream.extend(
        entries
            .into_iter()
            .map(|(id, event)| ConversationEventWithConfig {
                event_id: EventId::fixed(&id),
                event,
                config: config.clone(),
            }),
    );

    stream
}

fn qa(n: usize, question: &str, answer: &str) -> Vec<(String, ConversationEvent)> {
    vec![
        entry(&format!("turn{n:03}"), TurnStart::default()),
        entry(&format!("chat{n:03}"), ChatRequest::from(question)),
        entry(&format!("reply{n:02}"), ChatResponse::message(answer)),
    ]
}

fn conversation(turns: Vec<Vec<(String, ConversationEvent)>>) -> ConversationStream {
    stream(turns.into_iter().flatten().collect())
}

fn coding_turn() -> ConversationStream {
    let arguments = Map::from_iter([("path".to_owned(), json!("notes.md"))]);

    stream(vec![
        entry("turn001", TurnStart::default()),
        entry("chat001", ChatRequest::from("Fix the typo in my notes.")),
        entry("think01", ChatResponse::reasoning("The notes have a typo.")),
        entry(
            "tool001",
            ToolCallRequest::new("call_1".to_owned(), "fs_read_file".to_owned(), arguments),
        ),
        entry("tool002", ToolCallResponse {
            id: "call_1".to_owned(),
            result: Ok("- buy milk\n- call dentst".to_owned()),
        }),
        entry(
            "reply01",
            ChatResponse::message("Fixed the typo in notes.md."),
        ),
    ])
}

fn model_id(name: &str) -> ModelIdConfig {
    ModelIdConfig {
        provider: ProviderId::Test,
        name: name.parse().expect("valid model name"),
    }
}

fn model_config(name: &str) -> ModelConfig {
    ModelConfig {
        id: ModelIdOrAliasConfig::Id(model_id(name)),
        parameters: ParametersConfig::default(),
    }
}

fn provider(data: &Value) -> (MockProvider, Arc<Mutex<Vec<ChatQuery>>>) {
    MockProvider::new(vec![
        Event::structured(0, data.to_string()),
        Event::flush(0),
        Event::Finished(FinishReason::Completed),
    ])
    .capturing_requests()
}

async fn summarize(
    provider: &MockProvider,
    context_window: Option<u32>,
    events: ConversationStream,
    turn_ids: &[&str],
    batch_size: usize,
) -> Vec<GeneratedTurnSummary> {
    let details = ModelDetails {
        context_window,
        ..ModelDetails::empty(model_id("mock"))
    };

    generate(provider, &details, TurnSummaryRequest {
        events,
        turn_ids: turn_ids.iter().map(|turn_id| id(turn_id)).collect(),
        model: model_config("mock"),
        batch_size,
        max_response_bytes: None,
        notices: NoticeSink::new(|_| {}),
    })
    .await
    .expect("summary generation succeeds")
}

fn request(query: &ChatQuery) -> ChatRequest {
    query
        .thread
        .events
        .iter()
        .find_map(|event| event.as_chat_request().cloned())
        .expect("query contains a chat request")
}

fn requested_turns(query: &ChatQuery) -> Vec<String> {
    let schema = request(query).schema.expect("request contains a schema");

    schema["properties"]["summaries"]["required"]
        .as_array()
        .expect("schema lists required turn IDs")
        .iter()
        .map(|turn_id| turn_id.as_str().expect("turn ID is a string").to_owned())
        .collect()
}

#[test]
fn resolve_model_prefers_the_turn_summary_model() {
    let mut config = AppConfig::new_test();
    config.conversation.title.generate.model = Some(model_config("title-model"));
    config.conversation.turn_summary.generate.model = Some(model_config("summary-model"));

    assert_eq!(
        resolve_model(&config).id.resolved(),
        &model_id("summary-model")
    );
}

#[test]
fn resolve_model_falls_back_to_the_title_model() {
    let mut config = AppConfig::new_test();
    config.conversation.title.generate.model = Some(model_config("title-model"));
    config.conversation.turn_summary.generate.model = None;

    assert_eq!(
        resolve_model(&config).id.resolved(),
        &model_id("title-model")
    );
}

#[test]
fn resolve_model_falls_back_to_the_assistant_model_without_its_parameters() {
    let mut config = AppConfig::new_test();
    config.assistant.model.id = ModelIdOrAliasConfig::Id(model_id("big-model"));
    config.assistant.model.parameters.max_tokens = Some(ASSISTANT_MAX_TOKENS);
    config.conversation.title.generate.model = None;
    config.conversation.turn_summary.generate.model = None;

    let model = resolve_model(&config);

    assert_eq!(model.id.resolved(), &model_id("big-model"));
    assert_eq!(model.parameters.max_tokens, None);
}

#[test]
fn resolve_model_defaults_reasoning_to_low_effort() {
    let mut config = AppConfig::new_test();
    config.assistant.model.parameters.reasoning =
        Some(ReasoningConfig::Custom(CustomReasoningConfig {
            effort: ReasoningEffort::Max,
            exclude: false,
        }));

    assert_eq!(
        resolve_model(&config).parameters.reasoning,
        Some(ReasoningConfig::Custom(CustomReasoningConfig {
            effort: ReasoningEffort::Low,
            exclude: true,
        }))
    );
}

#[test]
fn resolve_model_keeps_an_explicit_reasoning_setting() {
    let mut config = AppConfig::new_test();
    let mut summary_model = model_config("summary-model");
    summary_model.parameters.reasoning = Some(ReasoningConfig::Off);
    config.conversation.turn_summary.generate.model = Some(summary_model);

    assert_eq!(
        resolve_model(&config).parameters.reasoning,
        Some(ReasoningConfig::Off)
    );
}

#[test]
fn a_transcript_leaves_out_reasoning_and_tool_output() {
    let built = transcripts(&coding_turn(), &[id("turn001")]);

    assert_eq!(built.len(), 1);
    assert_eq!(built[0].turn_id(), &id("turn001"));
    assert_eq!(
        built[0].text(),
        concat!(
            "<turn id=\"turn001\">\n",
            "<user>\n",
            "Fix the typo in my notes.\n",
            "</user>\n",
            "<tool_call name=\"fs_read_file\"/>\n",
            "<tool_result/>\n",
            "<assistant>\n",
            "Fixed the typo in notes.md.\n",
            "</assistant>\n",
            "</turn>",
        )
    );
}

#[test]
fn a_turn_inside_a_summary_compaction_is_transcribed_from_its_raw_events() {
    let mut events = conversation(vec![
        qa(1, "First question", "First answer"),
        qa(2, "Second question", "Second answer"),
    ]);
    events.add_compaction(
        Compaction::new(0, 0)
            .with_summary(SummaryPolicy::generated("A summary of the first turn.")),
    );

    let built = transcripts(&events, &[id("turn001")]);

    assert_eq!(
        built[0].text(),
        concat!(
            "<turn id=\"turn001\">\n",
            "<user>\n",
            "First question\n",
            "</user>\n",
            "<assistant>\n",
            "First answer\n",
            "</assistant>\n",
            "</turn>",
        )
    );
}

#[test]
fn a_long_message_in_a_transcript_is_truncated_to_head_and_tail() {
    let half = MAX_MESSAGE_CHARS / 2;
    let content = format!(
        "{}{}{}",
        "a".repeat(half),
        "m".repeat(OVERLONG_MESSAGE_LEN - MAX_MESSAGE_CHARS),
        "z".repeat(half)
    );
    let events = conversation(vec![qa(1, &content, "Done.")]);

    let built = transcripts(&events, &[id("turn001")]);

    assert_eq!(
        built[0].text(),
        format!(
            "<turn id=\"turn001\">\n<user>\n{}\n[…]\n{}\n</user>\n<assistant>\nDone.\n</\
             assistant>\n</turn>",
            "a".repeat(half),
            "z".repeat(half)
        )
    );
}

#[test]
fn markup_in_a_turn_is_wrapped_in_cdata() {
    let events = stream(vec![
        entry("turn001", TurnStart::default()),
        entry("chat001", ChatRequest::from("Is <T> valid after ]]>?")),
    ]);

    let built = transcripts(&events, &[id("turn001")]);

    assert_eq!(
        built[0].text(),
        concat!(
            "<turn id=\"turn001\">\n",
            "<user>\n",
            "<![CDATA[\n",
            "Is <T> valid after ]]]]><![CDATA[>?\n",
            "]]>\n",
            "</user>\n",
            "</turn>",
        )
    );
}

#[test]
fn a_turn_id_is_escaped_in_its_tag() {
    let events = stream(vec![
        entry("a\"b", TurnStart::default()),
        entry("chat001", ChatRequest::from("Hello")),
    ]);

    let built = transcripts(&events, &[id("a\"b")]);

    assert_eq!(
        built[0].text(),
        "<turn id=\"a&quot;b\">\n<user>\nHello\n</user>\n</turn>"
    );
}

#[test]
fn an_id_that_names_no_turn_start_is_skipped() {
    let events = conversation(vec![qa(1, "First question", "First answer")]);

    assert!(transcripts(&events, &[id("missing"), id("chat001")]).is_empty());
}

#[test]
fn a_turn_with_nothing_to_show_has_no_transcript() {
    let events = stream(vec![
        entry("turn001", TurnStart::default()),
        entry("think01", ChatResponse::reasoning("Only thoughts.")),
    ]);

    assert!(transcripts(&events, &[id("turn001")]).is_empty());
}

#[test]
fn transcripts_follow_stream_order() {
    let events = conversation(vec![
        qa(1, "First question", "First answer"),
        qa(2, "Second question", "Second answer"),
    ]);

    let built = transcripts(&events, &[id("turn002"), id("turn001")]);

    let turn_ids: Vec<_> = built.iter().map(|t| t.turn_id().to_string()).collect();
    assert_eq!(turn_ids, ["turn001", "turn002"]);
}

#[test]
fn a_fingerprint_does_not_depend_on_the_other_turns_transcribed() {
    let events = conversation(vec![
        qa(1, "First question", "First answer"),
        qa(2, "Second question", "Second answer"),
    ]);

    let alone = transcripts(&events, &[id("turn002")]);
    let together = transcripts(&events, &[id("turn001"), id("turn002")]);

    assert_eq!(alone[0].fingerprint(), together[1].fingerprint());
}

#[test]
fn editing_a_turn_changes_its_fingerprint() {
    let events = conversation(vec![qa(1, "First question", "First answer")]);
    let before = transcripts(&events, &[id("turn001")])[0].fingerprint();

    let mut edited = events;
    for mut event in edited.iter_mut() {
        if let Some(request) = event.as_chat_request_mut() {
            request.content = "An edited question".to_owned();
        }
    }

    let after = transcripts(&edited, &[id("turn001")])[0].fingerprint();
    assert_ne!(before, after);
}

#[test]
fn editing_stripped_tool_output_keeps_the_fingerprint() {
    let events = coding_turn();
    let before = transcripts(&events, &[id("turn001")])[0].fingerprint();

    let mut edited = events;
    for mut event in edited.iter_mut() {
        if let Some(response) = event.as_tool_call_response_mut() {
            response.result = Ok("- buy milk\n- call dentist".to_owned());
        }
    }

    let after = transcripts(&edited, &[id("turn001")])[0].fingerprint();
    assert_eq!(before, after);
}

#[test]
fn a_fingerprint_covers_the_ids_of_the_entries_it_read() {
    let original = stream(vec![
        entry("turn001", TurnStart::default()),
        entry("chat001", ChatRequest::from("Hello")),
    ]);
    let replaced = stream(vec![
        entry("turn001", TurnStart::default()),
        entry("chat002", ChatRequest::from("Hello")),
    ]);

    assert_ne!(
        transcripts(&original, &[id("turn001")])[0].fingerprint(),
        transcripts(&replaced, &[id("turn001")])[0].fingerprint()
    );
}

#[tokio::test]
async fn generate_summarizes_each_turn_it_sends() {
    let events = conversation(vec![
        qa(1, "First question", "First answer"),
        qa(2, "Second question", "Second answer"),
    ]);
    let (provider, _) = provider(&json!({"summaries": {
        "turn001": "Ask the first question",
        "turn002": "Ask the second question",
    }}));

    let summaries = summarize(
        &provider,
        None,
        events.clone(),
        &["turn001", "turn002"],
        BATCH_SIZE,
    )
    .await;

    let built = transcripts(&events, &[id("turn001"), id("turn002")]);
    assert_eq!(summaries, [
        GeneratedTurnSummary {
            turn_id: id("turn001"),
            summary: "Ask the first question".to_owned(),
            fingerprint: built[0].fingerprint(),
        },
        GeneratedTurnSummary {
            turn_id: id("turn002"),
            summary: "Ask the second question".to_owned(),
            fingerprint: built[1].fingerprint(),
        },
    ]);
}

#[tokio::test]
async fn generate_sends_the_transcripts_in_one_standalone_request() {
    let events = conversation(vec![qa(1, "First question", "First answer")]);
    let (provider, requests) = provider(&json!({"summaries": {}}));

    summarize(&provider, None, events, &["turn001"], BATCH_SIZE).await;

    let sent = requests.lock().unwrap();
    assert_eq!(sent.len(), 1);

    let query = &sent[0];
    let request = request(query);
    assert_eq!(
        request.content,
        concat!(
            "Summarize each of these conversation turns.\n",
            "\n",
            "<turn id=\"turn001\">\n",
            "<user>\n",
            "First question\n",
            "</user>\n",
            "<assistant>\n",
            "First answer\n",
            "</assistant>\n",
            "</turn>",
        )
    );
    assert_eq!(request.schema, Some(summary_schema(&[id("turn001")])));
    assert_eq!(query.thread.events.turn_count(), 1);
    assert!(query.tools.is_empty());
    assert_eq!(query.truncation, Truncation::Forbidden);
}

#[tokio::test]
async fn generate_does_not_share_the_conversation_cache_identity() {
    let events = conversation(vec![qa(1, "First question", "First answer")]);
    let (provider, requests) = provider(&json!({"summaries": {}}));

    summarize(&provider, None, events.clone(), &["turn001"], BATCH_SIZE).await;

    assert_ne!(
        requests.lock().unwrap()[0].thread.events.created_at,
        events.created_at
    );
}

#[tokio::test]
async fn generate_runs_on_the_summary_model_alone() {
    let mut events = conversation(vec![qa(1, "First question", "First answer")]);
    let mut delta = PartialAppConfig::empty();
    delta.assistant.model.parameters.max_tokens = Some(ASSISTANT_MAX_TOKENS);
    events.add_config_delta(delta);
    let (provider, requests) = provider(&json!({"summaries": {}}));

    summarize(&provider, None, events, &["turn001"], BATCH_SIZE).await;

    let config = requests.lock().unwrap()[0].thread.events.config().unwrap();
    assert_eq!(config.assistant.model, model_config("mock"));
}

#[tokio::test]
async fn generate_turns_caching_off() {
    let mut events = conversation(vec![qa(1, "First question", "First answer")]);
    let mut delta = PartialAppConfig::empty();
    delta.assistant.request.cache = Some(CachePolicy::Long);
    events.add_config_delta(delta);
    let (provider, requests) = provider(&json!({"summaries": {}}));

    summarize(&provider, None, events, &["turn001"], BATCH_SIZE).await;

    let config = requests.lock().unwrap()[0].thread.events.config().unwrap();
    assert_eq!(config.assistant.request.cache, CachePolicy::Off);
}

#[tokio::test]
async fn generate_summarizes_a_turn_inside_a_summary_compaction_from_its_raw_events() {
    let mut events = conversation(vec![
        qa(1, "First question", "First answer"),
        qa(2, "Second question", "Second answer"),
    ]);
    events.add_compaction(
        Compaction::new(0, 0)
            .with_summary(SummaryPolicy::generated("A summary of the first turn.")),
    );
    let (provider, requests) = provider(&json!({"summaries": {
        "turn001": "Ask the first question",
    }}));

    let summaries = summarize(&provider, None, events, &["turn001"], BATCH_SIZE).await;

    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].turn_id, id("turn001"));
    assert_eq!(
        request(&requests.lock().unwrap()[0]).content,
        concat!(
            "Summarize each of these conversation turns.\n",
            "\n",
            "<turn id=\"turn001\">\n",
            "<user>\n",
            "First question\n",
            "</user>\n",
            "<assistant>\n",
            "First answer\n",
            "</assistant>\n",
            "</turn>",
        )
    );
}

#[tokio::test]
async fn generate_leaves_out_a_turn_too_large_for_the_window_without_a_request() {
    let events = conversation(vec![qa(
        1,
        &"x".repeat(MAX_MESSAGE_CHARS),
        &"y".repeat(MAX_MESSAGE_CHARS),
    )]);
    let (provider, requests) = provider(&json!({"summaries": {}}));

    let summaries = summarize(&provider, Some(WINDOW), events, &["turn001"], BATCH_SIZE).await;

    assert!(summaries.is_empty());
    assert!(requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn generate_sends_the_turns_behind_an_oversized_one() {
    let events = conversation(vec![
        qa(1, "First question", "First answer"),
        qa(
            2,
            &"x".repeat(MAX_MESSAGE_CHARS),
            &"y".repeat(MAX_MESSAGE_CHARS),
        ),
    ]);
    let (provider, requests) = provider(&json!({"summaries": {}}));

    summarize(
        &provider,
        Some(WINDOW),
        events,
        &["turn001", "turn002"],
        BATCH_SIZE,
    )
    .await;

    assert_eq!(requested_turns(&requests.lock().unwrap()[0]), ["turn001"]);
}

#[tokio::test]
async fn generate_stops_at_the_turn_that_would_overflow_the_window() {
    let events = conversation(vec![
        qa(1, &"a".repeat(MAX_MESSAGE_CHARS / 2), "Done."),
        qa(2, &"b".repeat(MAX_MESSAGE_CHARS / 2), "Done."),
    ]);
    let (provider, requests) = provider(&json!({"summaries": {}}));

    summarize(
        &provider,
        Some(WINDOW),
        events,
        &["turn001", "turn002"],
        BATCH_SIZE,
    )
    .await;

    assert_eq!(requested_turns(&requests.lock().unwrap()[0]), ["turn002"]);
}

#[tokio::test]
async fn generate_sends_the_newest_turns_up_to_the_batch_size() {
    let events = conversation(
        (1..=4)
            .map(|n| qa(n, &format!("Question {n}"), &format!("Answer {n}")))
            .collect(),
    );
    let (provider, requests) = provider(&json!({"summaries": {}}));

    summarize(
        &provider,
        None,
        events,
        &["turn001", "turn002", "turn003", "turn004"],
        SMALL_BATCH_SIZE,
    )
    .await;

    assert_eq!(requested_turns(&requests.lock().unwrap()[0]), [
        "turn003", "turn004"
    ]);
}

#[tokio::test]
async fn generate_with_a_batch_size_of_zero_sends_nothing() {
    let events = conversation(vec![qa(1, "First question", "First answer")]);
    let (provider, requests) = provider(&json!({"summaries": {}}));

    let summaries = summarize(&provider, None, events, &["turn001"], 0).await;

    assert!(summaries.is_empty());
    assert!(requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn generate_discards_a_summary_for_a_turn_it_did_not_request() {
    let events = conversation(vec![
        qa(1, "First question", "First answer"),
        qa(2, "Second question", "Second answer"),
    ]);
    let (provider, _) = provider(&json!({"summaries": {
        "turn001": "Ask the first question",
        "turn002": "Ask the second question",
    }}));

    let summaries = summarize(&provider, None, events, &["turn001"], BATCH_SIZE).await;

    let turn_ids: Vec<_> = summaries.iter().map(|s| s.turn_id.to_string()).collect();
    assert_eq!(turn_ids, ["turn001"]);
}

#[test]
fn the_schema_asks_for_one_summary_per_requested_turn() {
    let schema = summary_schema(&[id("turn001"), id("turn002")]);

    assert_eq!(
        Value::Object(schema),
        json!({
            "type": "object",
            "required": ["summaries"],
            "additionalProperties": false,
            "properties": {
                "summaries": {
                    "type": "object",
                    "required": ["turn001", "turn002"],
                    "additionalProperties": false,
                    "properties": {
                        "turn001": {
                            "type": "string",
                            "description": "What this turn asked for and accomplished, in one line of no more than 60 characters"
                        },
                        "turn002": {
                            "type": "string",
                            "description": "What this turn asked for and accomplished, in one line of no more than 60 characters"
                        }
                    }
                }
            }
        })
    );
}

#[test]
fn the_instructions_ask_for_short_single_line_summaries() {
    let sections = summary_instructions();

    assert_eq!(sections.len(), 1);
    assert_eq!(
        sections[0].render(),
        concat!(
            "<instruction title=\"Turn Summaries\">\n",
            "Summarize each conversation turn you are given\n",
            "\n",
            "- Write one summary for every turn, keyed by its turn id in \"summaries\"\n",
            "- Describe what the turn asked for and what it accomplished\n",
            "- Concise and factual, on a single line of no more than 60 characters\n",
            "- Deliver as a JSON object with a \"summaries\" map from turn id to summary\n",
            "- DO NOT mention this request to summarize turns\n",
            "</instruction>",
        )
    );
}

#[test]
fn extract_summaries_returns_requested_turns_in_requested_order() {
    let data = json!({"summaries": {
        "turn002": "Add a test",
        "turn001": "Fix a typo",
        "turn009": "Unrequested",
    }});

    assert_eq!(extract_summaries(&data, &[id("turn001"), id("turn002")]), [
        (id("turn001"), "Fix a typo".to_owned()),
        (id("turn002"), "Add a test".to_owned()),
    ]);
}

#[test]
fn extract_summaries_normalizes_whitespace_and_truncates_overlong_values() {
    let data = json!({"summaries": {
        "turn001": "  Fix a typo\n  in the\tnotes ",
        "turn002": "x".repeat(OVERLONG_SUMMARY_LEN),
    }});

    assert_eq!(extract_summaries(&data, &[id("turn001"), id("turn002")]), [
        (id("turn001"), "Fix a typo in the notes".to_owned()),
        (
            id("turn002"),
            format!("{}…", "x".repeat(TRUNCATED_PREFIX_LEN))
        ),
    ]);
}

#[test]
fn extract_summaries_skips_blank_and_non_string_entries() {
    let data = json!({"summaries": {
        "turn001": "   ",
        "turn002": 3,
        "turn004": "Kept",
    }});

    let requested = [id("turn001"), id("turn002"), id("turn003"), id("turn004")];

    assert_eq!(extract_summaries(&data, &requested), [(
        id("turn004"),
        "Kept".to_owned()
    )]);
}

#[test]
fn extract_summaries_without_a_summaries_map_is_empty() {
    for data in [
        json!({"other": {}}),
        json!({"summaries": []}),
        json!({"summaries": "Fix a typo"}),
        json!("Fix a typo"),
    ] {
        assert!(
            extract_summaries(&data, &[id("turn001")]).is_empty(),
            "for {data}"
        );
    }
}

#[test]
fn mechanical_turn_summary_delegates_to_turn_fallback_summary() {
    let events = stream(vec![
        entry("turn001", TurnStart::default()),
        entry(
            "chat001",
            ChatRequest::from("Fix the retry test\nIt fails on CI."),
        ),
    ]);

    let turn = events.iter_turns().next().expect("a turn");
    assert_eq!(
        mechanical_turn_summary(&turn).as_deref(),
        Some("Fix the retry test")
    );
}
