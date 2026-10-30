use std::sync::{Arc, Mutex};

use chrono::{DateTime, TimeZone as _, Utc};
use jp_config::{
    PartialAppConfig,
    model::{
        id::{ModelIdConfig, ModelIdOrAliasConfig, ProviderId},
        parameters::ReasoningConfig,
    },
};
use jp_conversation::{
    SummaryPolicy,
    event::{ToolCallRequest, ToolCallResponse, TurnStart},
    stream::ConversationEventWithConfig,
};

use super::*;
use crate::{
    event::{Event, FinishReason},
    provider::mock::MockProvider,
};

/// A context window of 10k tokens: its budget fits a 10k character turn, but
/// not two of them.
const WINDOW: u32 = 10_000;

fn ts() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap()
}

fn id(value: &str) -> EventId {
    EventId::fixed(value)
}

/// One stream entry, named `id`.
fn entry(id: &str, kind: impl Into<EventKind>) -> (String, ConversationEvent) {
    (id.to_owned(), ConversationEvent::new(kind, ts()))
}

/// A stream holding `entries`, each under the ID it is paired with.
fn stream(entries: Vec<(String, ConversationEvent)>) -> ConversationStream {
    let mut stream = ConversationStream::new_test();

    // Every entry carries the base config, so no config delta lands between
    // them.
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

/// Turn `n`: one question and one answer, its entries named `turn00n`,
/// `chat00n` and `reply0n`.
fn qa(n: usize, question: &str, answer: &str) -> Vec<(String, ConversationEvent)> {
    vec![
        entry(&format!("turn{n:03}"), TurnStart::default()),
        entry(&format!("chat{n:03}"), ChatRequest::from(question)),
        entry(&format!("reply{n:02}"), ChatResponse::message(answer)),
    ]
}

/// A stream of `turns`, in order.
fn conversation(turns: Vec<Vec<(String, ConversationEvent)>>) -> ConversationStream {
    stream(turns.into_iter().flatten().collect())
}

/// One turn that reasons, reads a file, and answers.
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

/// A provider that answers with `data`, plus the log of what it received.
fn provider(data: &Value) -> (MockProvider, Arc<Mutex<Vec<ChatQuery>>>) {
    MockProvider::new(vec![
        Event::structured(0, data.to_string()),
        Event::flush(0),
        Event::Finished(FinishReason::Completed),
    ])
    .capturing_requests()
}

/// Ask `provider` to summarize `turn_ids`, on a model with `context_window`
/// tokens, or an unreported window.
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

/// The one chat request a captured query carries.
fn request(query: &ChatQuery) -> ChatRequest {
    query
        .thread
        .events
        .iter()
        .find_map(|event| event.as_chat_request().cloned())
        .expect("the query carries a request")
}

/// The turns a captured query asked about, in the order its schema names them.
fn requested_turns(query: &ChatQuery) -> Vec<String> {
    let schema = request(query).schema.expect("the request carries a schema");

    schema["properties"]["summaries"]["items"]["properties"]["turn_id"]["enum"]
        .as_array()
        .expect("the schema lists the turn ids")
        .iter()
        .map(|turn_id| turn_id.as_str().expect("a turn id is a string").to_owned())
        .collect()
}

// ---------------------------------------------------------------------------
// Model resolution
// ---------------------------------------------------------------------------

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

/// The assistant model lends its ID, never its parameters: a summary is a short
/// request, and the conversation's budget does not suit it.
#[test]
fn resolve_model_falls_back_to_the_assistant_model_without_its_parameters() {
    let mut config = AppConfig::new_test();
    config.assistant.model.id = ModelIdOrAliasConfig::Id(model_id("big-model"));
    config.assistant.model.parameters.max_tokens = Some(64_000);
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

// ---------------------------------------------------------------------------
// Transcripts
// ---------------------------------------------------------------------------

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
            "<tool_result>\n",
            "[compacted] fs_read_file: success\n",
            "</tool_result>\n",
            "<assistant>\n",
            "Fixed the typo in notes.md.\n",
            "</assistant>\n",
            "</turn>",
        )
    );
}

/// A stored summary stands in for its turns under a new ID, so transcribing the
/// projection would ask the model to summarize a turn it never saw.
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

/// An ID is whatever non-empty string a hand-edited file holds.
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

    // `chat001` names an entry, but not the one that opens its turn.
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

// ---------------------------------------------------------------------------
// Fingerprints
// ---------------------------------------------------------------------------

/// A summary is written back after a fresh read of the conversation, which
/// fingerprints only the turns that came back.
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

    let mut edited = events.clone();
    for mut event in edited.iter_mut() {
        if let Some(request) = event.as_chat_request_mut() {
            request.content = "An edited question".to_owned();
        }
    }

    let after = transcripts(&edited, &[id("turn001")])[0].fingerprint();
    assert_ne!(before, after);
}

/// The summary was written from a transcript that never held the tool output,
/// so changing that output leaves the summary as accurate as it was.
#[test]
fn editing_stripped_tool_output_keeps_the_fingerprint() {
    let events = coding_turn();
    let before = transcripts(&events, &[id("turn001")])[0].fingerprint();

    let mut edited = events.clone();
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

// ---------------------------------------------------------------------------
// Generation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn generate_summarizes_each_turn_it_sends() {
    let events = conversation(vec![
        qa(1, "First question", "First answer"),
        qa(2, "Second question", "Second answer"),
    ]);
    let (provider, _) = provider(&json!({"summaries": [
        {"turn_id": "turn001", "summary": "Ask the first question"},
        {"turn_id": "turn002", "summary": "Ask the second question"},
    ]}));

    let summaries = summarize(&provider, None, events.clone(), &["turn001", "turn002"], 5).await;

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
    let (provider, requests) = provider(&json!({"summaries": []}));

    summarize(&provider, None, events, &["turn001"], 5).await;

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

/// Providers derive a session and prompt cache identity from a stream's
/// creation time.
/// The request shares no prefix with the conversation, so it gains nothing from
/// the conversation's identity, and under it would compete for the cache the
/// conversation reads from.
#[tokio::test]
async fn generate_does_not_share_the_conversation_cache_identity() {
    let events = conversation(vec![qa(1, "First question", "First answer")]);
    let (provider, requests) = provider(&json!({"summaries": []}));

    summarize(&provider, None, events.clone(), &["turn001"], 5).await;

    assert_ne!(
        requests.lock().unwrap()[0].thread.events.created_at,
        events.created_at
    );
}

/// A config delta cannot unset a parameter, so a conversation's `max_tokens`
/// would otherwise reach a summary model that leaves it unset.
#[tokio::test]
async fn generate_runs_on_the_summary_model_alone() {
    let mut events = conversation(vec![qa(1, "First question", "First answer")]);
    let mut delta = PartialAppConfig::empty();
    delta.assistant.model.parameters.max_tokens = Some(64_000);
    events.add_config_delta(delta);
    let (provider, requests) = provider(&json!({"summaries": []}));

    summarize(&provider, None, events, &["turn001"], 5).await;

    let config = requests.lock().unwrap()[0].thread.events.config().unwrap();
    assert_eq!(config.assistant.model, model_config("mock"));
}

/// Every summary request carries turns that are summarized once, so a cache
/// entry written for it is paid for and never read, whatever the conversation's
/// own cache policy.
#[tokio::test]
async fn generate_turns_caching_off() {
    let mut events = conversation(vec![qa(1, "First question", "First answer")]);
    let mut delta = PartialAppConfig::empty();
    delta.assistant.request.cache = Some(CachePolicy::Long);
    events.add_config_delta(delta);
    let (provider, requests) = provider(&json!({"summaries": []}));

    summarize(&provider, None, events, &["turn001"], 5).await;

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
    let (provider, requests) = provider(&json!({"summaries": [
        {"turn_id": "turn001", "summary": "Ask the first question"},
    ]}));

    let summaries = summarize(&provider, None, events, &["turn001"], 5).await;

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
    let events = conversation(vec![qa(1, &"x".repeat(40_000), "Done.")]);
    let (provider, requests) = provider(&json!({"summaries": []}));

    let summaries = summarize(&provider, Some(WINDOW), events, &["turn001"], 5).await;

    assert!(summaries.is_empty());
    assert!(requests.lock().unwrap().is_empty());
}

/// An oversized turn is left out on its own, not treated as a full request: the
/// older turns behind it still go.
#[tokio::test]
async fn generate_sends_the_turns_behind_an_oversized_one() {
    let events = conversation(vec![
        qa(1, "First question", "First answer"),
        qa(2, &"x".repeat(40_000), "Done."),
    ]);
    let (provider, requests) = provider(&json!({"summaries": []}));

    summarize(&provider, Some(WINDOW), events, &["turn001", "turn002"], 5).await;

    assert_eq!(requested_turns(&requests.lock().unwrap()[0]), ["turn001"]);
}

#[tokio::test]
async fn generate_stops_at_the_turn_that_would_overflow_the_window() {
    let events = conversation(vec![
        qa(1, &"a".repeat(10_000), "Done."),
        qa(2, &"b".repeat(10_000), "Done."),
    ]);
    let (provider, requests) = provider(&json!({"summaries": []}));

    summarize(&provider, Some(WINDOW), events, &["turn001", "turn002"], 5).await;

    assert_eq!(requested_turns(&requests.lock().unwrap()[0]), ["turn002"]);
}

/// Providers that don't report a window (local llama.cpp, Ollama) leave nothing
/// to measure against, so only the batch size bounds the request.
#[tokio::test]
async fn generate_sends_every_turn_when_the_window_is_unknown() {
    let events = conversation(vec![
        qa(1, &"a".repeat(40_000), "Done."),
        qa(2, &"b".repeat(40_000), "Done."),
    ]);
    let (provider, requests) = provider(&json!({"summaries": []}));

    summarize(&provider, None, events, &["turn001", "turn002"], 5).await;

    assert_eq!(requested_turns(&requests.lock().unwrap()[0]), [
        "turn001", "turn002"
    ]);
}

#[tokio::test]
async fn generate_sends_the_newest_turns_up_to_the_batch_size() {
    let events = conversation(
        (1..=4)
            .map(|n| qa(n, &format!("Question {n}"), &format!("Answer {n}")))
            .collect(),
    );
    let (provider, requests) = provider(&json!({"summaries": []}));

    summarize(
        &provider,
        None,
        events,
        &["turn001", "turn002", "turn003", "turn004"],
        2,
    )
    .await;

    assert_eq!(requested_turns(&requests.lock().unwrap()[0]), [
        "turn003", "turn004"
    ]);
}

#[tokio::test]
async fn generate_with_a_batch_size_of_zero_sends_nothing() {
    let events = conversation(vec![qa(1, "First question", "First answer")]);
    let (provider, requests) = provider(&json!({"summaries": []}));

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
    let (provider, _) = provider(&json!({"summaries": [
        {"turn_id": "turn001", "summary": "Ask the first question"},
        {"turn_id": "turn002", "summary": "Ask the second question"},
    ]}));

    let summaries = summarize(&provider, None, events, &["turn001"], 5).await;

    let turn_ids: Vec<_> = summaries.iter().map(|s| s.turn_id.to_string()).collect();
    assert_eq!(turn_ids, ["turn001"]);
}

// ---------------------------------------------------------------------------
// Request and response shape
// ---------------------------------------------------------------------------

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
                    "type": "array",
                    "minItems": 2,
                    "maxItems": 2,
                    "items": {
                        "type": "object",
                        "required": ["turn_id", "summary"],
                        "additionalProperties": false,
                        "properties": {
                            "turn_id": {
                                "type": "string",
                                "enum": ["turn001", "turn002"],
                                "description": "The id of the turn this summary describes"
                            },
                            "summary": {
                                "type": "string",
                                "description": "What the turn asked for and accomplished, in one line of no more than 60 characters"
                            }
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
            "- Write one summary for every turn, naming the turn by its id\n",
            "- Describe what the turn asked for and what it accomplished\n",
            "- Concise and factual, on a single line of no more than 60 characters\n",
            "- Deliver as a JSON object with a \"summaries\" array\n",
            "- DO NOT mention this request to summarize turns\n",
            "</instruction>",
        )
    );
}

#[test]
fn extract_summaries_keeps_the_requested_turns_in_answer_order() {
    let data = json!({"summaries": [
        {"turn_id": "turn002", "summary": "Add a test"},
        {"turn_id": "turn001", "summary": "Fix a typo"},
    ]});

    assert_eq!(extract_summaries(&data, &[id("turn001"), id("turn002")]), [
        (id("turn002"), "Add a test".to_owned()),
        (id("turn001"), "Fix a typo".to_owned()),
    ]);
}

#[test]
fn extract_summaries_discards_a_turn_that_was_not_requested() {
    let data = json!({"summaries": [
        {"turn_id": "turn001", "summary": "Fix a typo"},
        {"turn_id": "turn009", "summary": "Not asked for"},
    ]});

    assert_eq!(extract_summaries(&data, &[id("turn001")]), [(
        id("turn001"),
        "Fix a typo".to_owned()
    )]);
}

#[test]
fn extract_summaries_keeps_the_first_summary_of_a_repeated_turn() {
    let data = json!({"summaries": [
        {"turn_id": "turn001", "summary": "Fix a typo"},
        {"turn_id": "turn001", "summary": "Fix another typo"},
    ]});

    assert_eq!(extract_summaries(&data, &[id("turn001")]), [(
        id("turn001"),
        "Fix a typo".to_owned()
    )]);
}

#[test]
fn extract_summaries_folds_a_summary_onto_one_line() {
    let data = json!({"summaries": [
        {"turn_id": "turn001", "summary": "  Fix a typo\n  in the\tnotes "},
    ]});

    assert_eq!(extract_summaries(&data, &[id("turn001")]), [(
        id("turn001"),
        "Fix a typo in the notes".to_owned()
    )]);
}

#[test]
fn extract_summaries_cuts_an_overlong_summary() {
    let data = json!({"summaries": [
        {"turn_id": "turn001", "summary": "x".repeat(100)},
    ]});

    let summaries = extract_summaries(&data, &[id("turn001")]);

    assert_eq!(summaries[0].1, format!("{}…", "x".repeat(71)));
}

#[test]
fn extract_summaries_skips_blank_and_malformed_entries() {
    let data = json!({"summaries": [
        {"turn_id": "turn001", "summary": "   "},
        {"turn_id": "turn002"},
        {"turn_id": 3, "summary": "A number for an id"},
        "Not an object",
        {"turn_id": "turn004", "summary": "Kept"},
    ]});

    let requested = [id("turn001"), id("turn002"), id("turn004")];

    assert_eq!(extract_summaries(&data, &requested), [(
        id("turn004"),
        "Kept".to_owned()
    )]);
}

#[test]
fn extract_summaries_without_a_summaries_array_is_empty() {
    for data in [
        json!({"other": []}),
        json!({"summaries": "Fix a typo"}),
        json!("Fix a typo"),
    ] {
        assert!(
            extract_summaries(&data, &[id("turn001")]).is_empty(),
            "for {data}"
        );
    }
}

// ---------------------------------------------------------------------------
// Mechanical fallback
// ---------------------------------------------------------------------------

/// The mechanical summary of the first turn in `events`.
fn mechanical(events: &ConversationStream) -> Option<String> {
    let turn = events.iter_turns().next().expect("a turn");
    mechanical_turn_summary(&turn)
}

#[test]
fn the_mechanical_summary_is_the_first_line_of_the_first_request() {
    let events = stream(vec![
        entry("turn001", TurnStart::default()),
        entry(
            "chat001",
            ChatRequest::from("Fix the retry test\nIt fails on CI."),
        ),
        entry("reply01", ChatResponse::message("Looking.")),
        entry("chat002", ChatRequest::from("Also update the docs.")),
    ]);

    assert_eq!(mechanical(&events).as_deref(), Some("Fix the retry test"));
}

#[test]
fn the_mechanical_summary_skips_blank_lines_and_folds_whitespace() {
    let events = stream(vec![
        entry("turn001", TurnStart::default()),
        entry(
            "chat001",
            ChatRequest::from("\n   \n  Fix   the\tretry test  \nIt fails."),
        ),
    ]);

    assert_eq!(mechanical(&events).as_deref(), Some("Fix the retry test"));
}

#[test]
fn the_mechanical_summary_is_cut_to_72_characters() {
    let events = stream(vec![
        entry("turn001", TurnStart::default()),
        entry("chat001", ChatRequest::from("y".repeat(100))),
    ]);

    assert_eq!(mechanical(&events), Some(format!("{}…", "y".repeat(71))));
}

#[test]
fn a_72_character_request_is_kept_whole() {
    let events = stream(vec![
        entry("turn001", TurnStart::default()),
        entry("chat001", ChatRequest::from("y".repeat(72))),
    ]);

    assert_eq!(mechanical(&events), Some("y".repeat(72)));
}

#[test]
fn the_mechanical_summary_counts_characters_not_bytes() {
    let events = stream(vec![
        entry("turn001", TurnStart::default()),
        entry("chat001", ChatRequest::from("é".repeat(72))),
    ]);

    assert_eq!(mechanical(&events), Some("é".repeat(72)));
}

#[test]
fn a_turn_without_a_request_has_no_mechanical_summary() {
    let events = stream(vec![
        entry("turn001", TurnStart::default()),
        entry("reply01", ChatResponse::message("An answer to nothing.")),
    ]);

    assert_eq!(mechanical(&events), None);
}
