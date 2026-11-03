//! Turn summary generation.
//!
//! [`generate`] requests single-line summaries for up to `batch_size` completed
//! turns and returns the validated results. [`resolve_model`] selects the model
//! and reasoning parameters for the request.
//!
//! Each turn is formatted as an XML [`TurnTranscript`] containing user
//! requests, tool call names and outcomes, and assistant responses (omitting
//! reasoning and raw tool payloads). [`TurnTranscript::fingerprint`] hashes the
//! rendered transcript and entry IDs so callers can verify that a turn was not
//! modified while the summary request was in flight.

use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    fmt::Write as _,
    hash::{DefaultHasher, Hash as _, Hasher as _},
    sync::Arc,
};

use jp_config::{
    AppConfig,
    assistant::{
        instructions::InstructionsConfig, request::CachePolicy, sections::SectionConfig,
        tool_choice::ToolChoice,
    },
    model::{
        ModelConfig,
        parameters::{CustomReasoningConfig, ParametersConfig, ReasoningEffort},
    },
};
pub use jp_conversation::event::MAX_SUMMARY_CHARS;
use jp_conversation::{
    ConversationEvent, ConversationStream, EventId, EventKind, Turn,
    event::{ChatRequest, ChatResponse, format_turn_summary},
    stream::EventInTurn,
    thread::ThreadBuilder,
};
use quick_xml::escape::escape;
use serde_json::{Map, Value, json};
use tracing::debug;

use crate::{
    Provider,
    error::Result,
    event::NoticeSink,
    event_builder,
    model::ModelDetails,
    query::{ChatQuery, Truncation},
    retry::{RetryConfig, collect_with_retry},
    window,
};

/// Maximum number of characters kept from a single request or response message
/// in a [`TurnTranscript`].
///
/// Messages exceeding this limit keep their first and last halves separated by
/// `\n[…]\n` so that turns containing large pasted logs or long documents can
/// still be summarized within a small model's context window.
pub const MAX_MESSAGE_CHARS: usize = 4_000;

const REQUEST_PREAMBLE: &str = "Summarize each of these conversation turns.";

/// Parameters for a turn summary generation request.
#[derive(Debug)]
pub struct TurnSummaryRequest {
    /// Conversation stream containing the candidate turns.
    pub events: ConversationStream,

    /// `TurnStart` event IDs of the candidate turns to summarize.
    pub turn_ids: Vec<EventId>,

    /// Resolved model and parameters for the summary request.
    pub model: ModelConfig,

    /// Maximum number of turns to summarize in a single request.
    pub batch_size: usize,

    /// Optional byte ceiling on the provider response.
    pub max_response_bytes: Option<u64>,

    /// Sink for provider notices (such as credential fallback).
    pub notices: NoticeSink,
}

/// Summary generated for a single turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedTurnSummary {
    pub turn_id: EventId,
    pub summary: String,
    pub fingerprint: TurnFingerprint,
}

/// Stripped XML transcript of a single turn for the summary prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnTranscript {
    turn_id: EventId,
    text: String,
    fingerprint: TurnFingerprint,
}

impl TurnTranscript {
    #[must_use]
    pub const fn turn_id(&self) -> &EventId {
        &self.turn_id
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub const fn fingerprint(&self) -> TurnFingerprint {
        self.fingerprint
    }
}

/// In-memory hash of a [`TurnTranscript`] and the event IDs of the entries that
/// produced it.
///
/// Used to detect whether a turn's summarized content or event identity changed
/// between task spawn and persistence. Not stable across processes or releases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TurnFingerprint(u64);

/// Resolve the model configuration for turn summary generation.
///
/// Follows the fallback chain:
/// 1. `conversation.turn_summary.generate.model`
/// 2. `conversation.title.generate.model`
/// 3. `assistant.model.id` with default parameters
///
/// If `parameters.reasoning` is unset on the selected model, it defaults to
/// [`ReasoningEffort::Low`] with `exclude = true`.
#[must_use]
pub fn resolve_model(config: &AppConfig) -> ModelConfig {
    let conversation = &config.conversation;

    let mut model = conversation
        .turn_summary
        .generate
        .model
        .clone()
        .or_else(|| conversation.title.generate.model.clone())
        .unwrap_or_else(|| ModelConfig {
            id: config.assistant.model.id.clone(),
            parameters: ParametersConfig::default(),
        });

    if model.parameters.reasoning.is_none() {
        model.parameters.reasoning = Some(
            CustomReasoningConfig {
                effort: ReasoningEffort::Low,
                exclude: true,
            }
            .into(),
        );
    }

    model
}

/// Generate single-line summaries for candidate turns in `request`.
///
/// Candidate turns are rendered into [`TurnTranscript`]s and packed newest
/// first up to `request.batch_size` and the context window budget of `details`.
/// Prompt caching is disabled because each batch of turns is summarized once.
///
/// # Errors
///
/// Returns an error if the conversation configuration fails to resolve, the
/// query thread cannot be built, or the provider request fails after retries.
pub async fn generate(
    provider: &dyn Provider,
    details: &ModelDetails,
    request: TurnSummaryRequest,
) -> Result<Vec<GeneratedTurnSummary>> {
    let TurnSummaryRequest {
        events,
        turn_ids,
        model,
        batch_size,
        max_response_bytes,
        notices,
    } = request;

    let sections = summary_instructions();
    let budget = details.context_window.map(|context_window| {
        let overhead =
            window::estimate_overhead_chars(None, &sections, &[], &[]) + REQUEST_PREAMBLE.len();
        window::budget_chars(context_window, overhead)
    });

    let packed = pack(transcripts(&events, &turn_ids), batch_size, budget);
    if packed.is_empty() {
        debug!("No turn transcripts fit the summary request.");
        return Ok(vec![]);
    }

    let sent: Vec<EventId> = packed.iter().map(|t| t.turn_id.clone()).collect();
    debug!(turns = sent.len(), "Requesting turn summaries.");

    let mut config = events.config()?;
    config.assistant.model = model;
    config.assistant.request.cache = CachePolicy::Off;

    // Construct a standalone stream with a fresh `created_at` timestamp so the
    // summary request does not share prompt-cache identity with the main
    // conversation.
    let mut thread_events = ConversationStream::new(Arc::new(config));
    thread_events.start_turn(ChatRequest {
        content: request_text(&packed),
        schema: Some(summary_schema(&sent)),
        author: None,
    });

    let query = ChatQuery {
        thread: ThreadBuilder::default()
            .with_events(thread_events)
            .with_sections(sections)
            .build()?,
        tools: vec![],
        tool_choice: ToolChoice::default(),
        truncation: Truncation::Forbidden,
    };

    let retry = RetryConfig::default().with_max_response_bytes(max_response_bytes);
    let response = collect_with_retry(provider, details, query, &retry, &notices).await?;

    let Some(data) = event_builder::structured_data(response) else {
        debug!("Turn summary response contained no structured data.");
        return Ok(vec![]);
    };

    let fingerprints: HashMap<EventId, TurnFingerprint> = packed
        .into_iter()
        .map(|transcript| (transcript.turn_id, transcript.fingerprint))
        .collect();

    Ok(extract_summaries(&data, &sent)
        .into_iter()
        .filter_map(|(turn_id, summary)| {
            let fingerprint = *fingerprints.get(&turn_id)?;
            Some(GeneratedTurnSummary {
                turn_id,
                summary,
                fingerprint,
            })
        })
        .collect())
}

/// Build [`TurnTranscript`]s for the turns matching `turn_ids`, in stream order.
///
/// Reads raw turn events directly from `events`, ignoring compaction overlays so
/// turns covered by a compaction summary are still transcribed from their own
/// events. Reasoning events and tool arguments/payloads are omitted, and long
/// messages are truncated to [`MAX_MESSAGE_CHARS`].
///
/// Unknown `EventId`s and turns without provider-visible content are skipped.
#[must_use]
pub fn transcripts(events: &ConversationStream, turn_ids: &[EventId]) -> Vec<TurnTranscript> {
    let wanted: HashSet<&EventId> = turn_ids.iter().collect();
    if wanted.is_empty() {
        return vec![];
    }

    let mut turns: Vec<Vec<EventInTurn<'_>>> = vec![];
    let mut keep_current = false;

    for entry in events.iter_events_by_turn() {
        if entry.is_turn_start() {
            keep_current = wanted.contains(entry.event_id);
            if keep_current {
                turns.push(vec![entry]);
            }
        } else if keep_current && let Some(turn) = turns.last_mut() {
            turn.push(entry);
        }
    }

    turns
        .iter()
        .filter_map(|entries| transcribe(entries))
        .collect()
}

fn transcribe(entries: &[EventInTurn<'_>]) -> Option<TurnTranscript> {
    let (turn_start, rest) = entries.split_first()?;
    if !turn_start.is_turn_start() {
        return None;
    }

    let mut hasher = DefaultHasher::new();
    let mut body = String::new();
    for entry in rest {
        if render_event(&mut body, entry.event) {
            entry.event_id.hash(&mut hasher);
        }
    }

    if body.is_empty() {
        return None;
    }

    let turn_id = turn_start.event_id.clone();
    let text = format!(
        "<turn id=\"{}\">\n{body}</turn>",
        escape(turn_id.to_string())
    );
    text.hash(&mut hasher);

    Some(TurnTranscript {
        turn_id,
        text,
        fingerprint: TurnFingerprint(hasher.finish()),
    })
}

fn render_event(out: &mut String, event: &ConversationEvent) -> bool {
    match &event.kind {
        EventKind::ChatRequest(request) => {
            push_element(out, "user", "", &truncate_message(&request.content));
        }
        EventKind::ChatResponse(ChatResponse::Message { message }) => {
            push_element(out, "assistant", "", &truncate_message(message));
        }
        EventKind::ChatResponse(ChatResponse::Structured { data }) => {
            push_element(out, "assistant", "", &truncate_message(&data.to_string()));
        }
        EventKind::ToolCallRequest(request) => {
            let name = format!(" name=\"{}\"", escape(request.name.as_str()));
            push_element(out, "tool_call", &name, "");
        }
        EventKind::ToolCallResponse(response) => {
            let tag = if response.result.is_ok() {
                "tool_result"
            } else {
                "tool_error"
            };
            push_element(out, tag, "", "");
        }
        EventKind::ChatResponse(ChatResponse::Reasoning { .. })
        | EventKind::TurnStart(_)
        | EventKind::InquiryRequest(_)
        | EventKind::InquiryResponse(_) => return false,
    }

    true
}

fn truncate_message(text: &str) -> Cow<'_, str> {
    let count = text.chars().count();
    if count <= MAX_MESSAGE_CHARS {
        return Cow::Borrowed(text);
    }

    let half = MAX_MESSAGE_CHARS / 2;
    let head: String = text.chars().take(half).collect();
    let tail: String = text.chars().skip(count - half).collect();
    Cow::Owned(format!("{}\n[…]\n{}", head.trim_end(), tail.trim_start()))
}

fn push_element(out: &mut String, tag: &str, attributes: &str, content: &str) {
    let content = content.trim();
    if content.is_empty() {
        let _ = writeln!(out, "<{tag}{attributes}/>");
        return;
    }

    let _ = writeln!(
        out,
        "<{tag}{attributes}>\n{}\n</{tag}>",
        cdata_if_needed(content)
    );
}

fn cdata_if_needed(content: &str) -> Cow<'_, str> {
    if !content.contains(['<', '>', '&']) {
        return Cow::Borrowed(content);
    }

    // Split any embedded `]]>` across two adjacent CDATA sections so it cannot
    // terminate the outer CDATA block early.
    Cow::Owned(format!(
        "<![CDATA[\n{}\n]]>",
        content.replace("]]>", "]]]]><![CDATA[>")
    ))
}

/// Pack transcripts newest-first up to `batch_size` and `budget`, returning the
/// selected turns in stream order.
///
/// Any single transcript exceeding `budget` on its own is skipped so older
/// turns behind it can still be packed.
fn pack(
    transcripts: Vec<TurnTranscript>,
    batch_size: usize,
    budget: Option<usize>,
) -> Vec<TurnTranscript> {
    let fits = |chars: usize| budget.is_none_or(|budget| chars <= budget);

    let mut packed = vec![];
    let mut total = 0;
    for transcript in transcripts.into_iter().rev() {
        if packed.len() == batch_size {
            break;
        }

        let chars = transcript.text.len();
        if !fits(chars) {
            debug!(
                turn_id = %transcript.turn_id,
                chars,
                "Turn transcript exceeds context budget; skipping."
            );
            continue;
        }

        if !fits(total + chars) {
            break;
        }

        total += chars;
        packed.push(transcript);
    }

    packed.reverse();
    packed
}

fn request_text(transcripts: &[TurnTranscript]) -> String {
    let mut text = REQUEST_PREAMBLE.to_owned();
    for transcript in transcripts {
        text.push_str("\n\n");
        text.push_str(&transcript.text);
    }

    text
}

/// JSON schema for turn summary structured output.
///
/// Requires a `summaries` object with one string property per ID in `turn_ids`.
#[must_use]
#[allow(clippy::missing_panics_doc)]
pub fn summary_schema(turn_ids: &[EventId]) -> Map<String, Value> {
    let required: Vec<String> = turn_ids.iter().map(ToString::to_string).collect();
    let properties: Map<String, Value> = required
        .iter()
        .map(|id| {
            (
                id.clone(),
                json!({
                    "type": "string",
                    "description": "What this turn asked for and accomplished, in one line \
                                    of no more than 60 characters"
                }),
            )
        })
        .collect();

    let schema = json!({
        "type": "object",
        "required": ["summaries"],
        "additionalProperties": false,
        "properties": {
            "summaries": {
                "type": "object",
                "required": required,
                "additionalProperties": false,
                "properties": properties,
            }
        }
    });

    schema
        .as_object()
        .expect("schema is always an object")
        .clone()
}

/// Build the instruction sections for turn summary generation.
#[must_use]
pub fn summary_instructions() -> Vec<SectionConfig> {
    vec![
        InstructionsConfig::default()
            .with_title("Turn Summaries")
            .with_description("Summarize each conversation turn you are given")
            .with_item("Write one summary for every turn, keyed by its turn id in \"summaries\"")
            .with_item("Describe what the turn asked for and what it accomplished")
            .with_item("Concise and factual, on a single line of no more than 60 characters")
            .with_item("Deliver as a JSON object with a \"summaries\" map from turn id to summary")
            .with_item("DO NOT mention this request to summarize turns")
            .to_section(),
    ]
}

/// Extract and normalize turn summaries from a structured JSON response.
///
/// Expects an object with a `summaries` map keyed by turn ID:
///
/// ```json
/// {"summaries": {"k3m9x2a": "Add retry backoff"}}
/// ```
///
/// Returns entries for IDs in `turn_ids` in the order of `turn_ids`. Blank or
/// non-string values and unrequested keys are ignored.
#[must_use]
pub fn extract_summaries(data: &Value, turn_ids: &[EventId]) -> Vec<(EventId, String)> {
    let Some(summaries) = data.get("summaries").and_then(Value::as_object) else {
        return vec![];
    };

    turn_ids
        .iter()
        .filter_map(|turn_id| {
            let raw = summaries.get(&turn_id.to_string())?.as_str()?;
            let summary = format_turn_summary(raw)?;
            Some((turn_id.clone(), summary))
        })
        .collect()
}

/// Fallback summary for a turn without a stored LLM summary.
///
/// Delegates to [`Turn::fallback_summary`].
#[must_use]
pub fn mechanical_turn_summary(turn: &Turn<'_>) -> Option<String> {
    turn.fallback_summary()
}

#[cfg(test)]
#[path = "turn_summary_tests.rs"]
mod tests;
