//! Turn summary generation.
//!
//! [`generate`] is the entry point: it asks a model for a one-line summary of
//! each of a conversation's turns, at most `batch_size` of them per request,
//! and returns the summaries that came back.
//! [`resolve_model`] picks the model that request runs on.
//!
//! A turn reaches the model as a [`TurnTranscript`]: its events with reasoning
//! left out and tool calls reduced to their name and outcome, tagged with the
//! ID of the turn's `TurnStart` so an answer can name the turn it describes.
//! [`TurnTranscript::fingerprint`] identifies what the model read, so a caller
//! storing a summary later can check that the turn has not changed since.
//!
//! [`mechanical_turn_summary`] is what to show for a turn with no stored
//! summary.

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
use jp_conversation::{
    Compaction, ConversationEvent, ConversationStream, EventId, EventKind, ReasoningPolicy,
    ToolCallPolicy, Turn,
    event::{ChatRequest, ChatResponse},
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

/// The most characters a turn summary holds.
///
/// The model is asked for at most 60; a longer answer is cut to this, as is the
/// request line [`mechanical_turn_summary`] shows in a summary's place.
pub const MAX_SUMMARY_CHARS: usize = 72;

/// Opens the request, ahead of the transcripts.
const REQUEST_PREAMBLE: &str = "Summarize each of these conversation turns.";

/// A request for LLM-generated turn summaries.
#[derive(Debug)]
pub struct TurnSummaryRequest {
    /// The conversation the turns belong to.
    pub events: ConversationStream,

    /// The turns to summarize, each named by the ID of its `TurnStart`.
    ///
    /// An ID that names no turn in `events` is skipped.
    pub turn_ids: Vec<EventId>,

    /// The model the request runs on, with the parameters it runs under.
    ///
    /// These parameters are the only ones the request carries: none are
    /// inherited from the conversation.
    /// See [`resolve_model`].
    pub model: ModelConfig,

    /// The most turns the request summarizes.
    pub batch_size: usize,

    /// Output byte ceiling for the request, from
    /// `assistant.request.max_response_bytes`.
    /// A runaway response is cut off past this and surfaced as a non-retryable
    /// error rather than regenerated.
    /// `None` leaves the response unbounded.
    pub max_response_bytes: Option<u64>,

    /// Where the provider's notices go, such as a switch to another credential.
    pub notices: NoticeSink,
}

/// A summary the model wrote for one turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedTurnSummary {
    /// The ID of the turn's `TurnStart`.
    pub turn_id: EventId,

    /// One line of at most [`MAX_SUMMARY_CHARS`] characters.
    pub summary: String,

    /// The fingerprint of the transcript the model read for this turn.
    pub fingerprint: TurnFingerprint,
}

/// One turn as the summary model reads it.
///
/// Built by [`transcripts`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnTranscript {
    /// The ID of the turn's `TurnStart`.
    turn_id: EventId,

    /// The markup the request carries for this turn.
    text: String,

    /// See [`TurnFingerprint`].
    fingerprint: TurnFingerprint,
}

impl TurnTranscript {
    /// The ID of the turn's `TurnStart`.
    #[must_use]
    pub const fn turn_id(&self) -> &EventId {
        &self.turn_id
    }

    /// The markup the request carries for this turn.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Identifies what the model reads for this turn.
    #[must_use]
    pub const fn fingerprint(&self) -> TurnFingerprint {
        self.fingerprint
    }
}

/// Identifies the transcript of one turn.
///
/// Two fingerprints of a turn are equal when its transcript reads the same and
/// was built from entries with the same IDs.
/// Editing what a transcript leaves out, such as reasoning or tool output, does
/// not change it.
///
/// Only comparable within one process: the hash is not stable across releases,
/// so a fingerprint must not be stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TurnFingerprint(u64);

/// Resolve the model that turn summaries are generated with.
///
/// `conversation.turn_summary.generate.model` is used when configured, then
/// `conversation.title.generate.model`, and otherwise the assistant model's ID
/// with default parameters, so a summary request never inherits the
/// conversation's own parameters.
///
/// Reasoning defaults to low effort with the trace excluded: a summary is one
/// factual line, and deep reasoning on every turn rarely earns its cost.
/// An explicit reasoning setting on the chosen model is preserved.
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

/// Generate one-line summaries for the turns `request` names.
///
/// Every turn goes into the request whole or not at all.
/// A turn whose transcript alone does not fit the budget that
/// `details.context_window` leaves is skipped; the rest are taken newest first
/// until `request.batch_size` are taken or the next one would not fit.
/// When the window is unknown, only the batch size bounds the request.
/// No request is made when no turn is taken.
///
/// The request runs with prompt caching off: the turns it carries are
/// summarized once, so a cache entry written for it would never be read.
///
/// Returns at most one summary per turn sent, in the order the model gave them.
/// A summary naming a turn that was not sent is discarded, as is a blank one,
/// so a turn the model passed over is absent from the result.
///
/// # Errors
///
/// Returns an error if the conversation's config deltas do not merge into a
/// valid config, the thread cannot be built, or the provider request fails
/// after exhausting its retries.
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

    // The request carries no system prompt and no attachments, so the
    // instruction sections and the preamble are the only fixed overhead.
    let budget = details.context_window.map(|context_window| {
        let overhead =
            window::estimate_overhead_chars(None, &sections, &[], &[]) + REQUEST_PREAMBLE.len();
        window::budget_chars(context_window, overhead)
    });

    // The conversation's settings carry over, but the model is replaced whole:
    // a config delta cannot unset a parameter the conversation's model set and
    // the summary model leaves unset.
    let mut config = events.config()?;
    config.assistant.model = model;
    config.assistant.request.cache = CachePolicy::Off;

    let packed = pack(strip(events, &turn_ids), batch_size, budget);
    if packed.is_empty() {
        debug!("No turn to summarize fits a request.");
        return Ok(vec![]);
    }

    let sent: Vec<EventId> = packed.iter().map(|t| t.turn_id.clone()).collect();
    debug!(turns = sent.len(), "Requesting turn summaries.");

    // A fresh stream has its own creation time, which providers derive a
    // session and prompt cache identity from. The request shares no prefix
    // with the conversation, so it gains nothing from the conversation's
    // identity, and under it would compete for the cache the conversation
    // reads from.
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
        // A summary is stored as describing its turn, so a request the provider
        // quietly shortened would yield summaries of turns the model never read
        // in full.
        truncation: Truncation::Forbidden,
    };

    let retry = RetryConfig::default().with_max_response_bytes(max_response_bytes);
    let response = collect_with_retry(provider, details, query, &retry, &notices).await?;

    let Some(data) = event_builder::structured_data(response) else {
        debug!("The turn summary response carried no structured data.");
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

/// Build the transcripts of the turns `turn_ids` names, in stream order.
///
/// Each turn is read from its own events: the conversation's compaction
/// overlays are ignored, so a turn a stored summary stands in for is still
/// transcribed from what it holds.
/// Reasoning is left out, tool call arguments are dropped, and tool output is
/// reduced to a status line: the `ReasoningPolicy::Strip` and
/// `ToolCallPolicy::Strip` compaction policies, applied to the whole turn.
///
/// An ID that names no `TurnStart` in `events` is skipped, as is a turn with
/// nothing left to show.
#[must_use]
pub fn transcripts(events: &ConversationStream, turn_ids: &[EventId]) -> Vec<TurnTranscript> {
    strip(events.clone(), turn_ids)
}

/// [`transcripts`], for a caller that can give up its stream.
fn strip(mut events: ConversationStream, turn_ids: &[EventId]) -> Vec<TurnTranscript> {
    let wanted: HashSet<&EventId> = turn_ids.iter().collect();
    let keep: HashSet<usize> = events
        .iter_events_by_turn()
        .filter(|entry| entry.is_turn_start() && wanted.contains(entry.event_id))
        .map(|entry| entry.turn)
        .collect();

    if keep.is_empty() {
        return vec![];
    }

    // A summary compaction replaces its turns with a synthetic one under a new
    // ID, which would have the model summarize a turn it never saw. Patch
    // overlays stay: they only rewrite metadata, which a transcript does not
    // carry.
    events.remove_compactions();
    events.retain_turns(|turn| keep.contains(&turn));

    let last_turn = events.turn_count().saturating_sub(1);
    events.add_compaction(
        Compaction::new(0, last_turn)
            .with_reasoning(ReasoningPolicy::Strip)
            .with_tool_calls(ToolCallPolicy::Strip {
                request: true,
                response: true,
            }),
    );
    events.apply_projection();

    let mut turns: Vec<(usize, Vec<EventInTurn<'_>>)> = vec![];
    for entry in events.iter_events_by_turn() {
        match turns.last_mut() {
            Some((turn, entries)) if *turn == entry.turn => entries.push(entry),
            _ => turns.push((entry.turn, vec![entry])),
        }
    }

    turns
        .iter()
        .filter_map(|(_, entries)| transcribe(entries))
        .collect()
}

/// Render one turn from its projected entries, the first of which opens it.
///
/// Returns `None` when the turn has nothing a provider would see.
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

/// Append the markup for one event, returning whether it wrote any.
fn render_event(out: &mut String, event: &ConversationEvent) -> bool {
    match &event.kind {
        EventKind::ChatRequest(request) => push_element(out, "user", "", &request.content),
        EventKind::ChatResponse(ChatResponse::Message { message }) => {
            push_element(out, "assistant", "", message);
        }
        EventKind::ChatResponse(ChatResponse::Structured { data }) => {
            push_element(out, "assistant", "", &data.to_string());
        }
        EventKind::ToolCallRequest(request) => {
            let name = format!(" name=\"{}\"", escape(request.name.as_str()));
            let arguments = if request.arguments.is_empty() {
                String::new()
            } else {
                Value::Object(request.arguments.clone()).to_string()
            };
            push_element(out, "tool_call", &name, &arguments);
        }
        EventKind::ToolCallResponse(response) => {
            let tag = if response.result.is_ok() {
                "tool_result"
            } else {
                "tool_error"
            };
            push_element(out, tag, "", response.content());
        }
        // Projection has already dropped reasoning; turn markers and inquiries
        // never reach a provider.
        EventKind::ChatResponse(ChatResponse::Reasoning { .. })
        | EventKind::TurnStart(_)
        | EventKind::InquiryRequest(_)
        | EventKind::InquiryResponse(_) => return false,
    }

    true
}

/// Append `<tag attributes>`, `content` and `</tag>` on lines of their own, or
/// `<tag attributes/>` when `content` is blank.
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

/// Wrap `content` in CDATA when it holds characters that would read as markup.
///
/// A `]]>` inside it is split across two sections, since it would otherwise
/// close the first one early.
fn cdata_if_needed(content: &str) -> Cow<'_, str> {
    if !content.contains(['<', '>', '&']) {
        return Cow::Borrowed(content);
    }

    Cow::Owned(format!(
        "<![CDATA[\n{}\n]]>",
        content.replace("]]>", "]]]]><![CDATA[>")
    ))
}

/// Choose the transcripts one request carries, returned in stream order.
///
/// A transcript longer than `budget` on its own is left out.
/// The rest are taken newest first, whole, until `batch_size` are taken or the
/// next one would push the total past `budget`.
/// A `budget` of `None` bounds nothing.
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
                "Turn is too large to summarize; leaving it out."
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

/// The text of the request: what to do, then each transcript.
fn request_text(transcripts: &[TurnTranscript]) -> String {
    let mut text = REQUEST_PREAMBLE.to_owned();
    for transcript in transcripts {
        text.push_str("\n\n");
        text.push_str(&transcript.text);
    }

    text
}

/// JSON schema for the turn summary structured output.
///
/// Requires a `summaries` array with one `{ turn_id, summary }` entry per turn
/// in `turn_ids`, each `turn_id` one of those IDs.
#[must_use]
#[allow(clippy::missing_panics_doc)]
pub fn summary_schema(turn_ids: &[EventId]) -> Map<String, Value> {
    let count = turn_ids.len();
    let ids: Vec<String> = turn_ids.iter().map(ToString::to_string).collect();

    let schema = json!({
        "type": "object",
        "required": ["summaries"],
        "additionalProperties": false,
        "properties": {
            "summaries": {
                "type": "array",
                "minItems": count,
                "maxItems": count,
                "items": {
                    "type": "object",
                    "required": ["turn_id", "summary"],
                    "additionalProperties": false,
                    "properties": {
                        "turn_id": {
                            "type": "string",
                            "enum": ids,
                            "description": "The id of the turn this summary describes"
                        },
                        "summary": {
                            "type": "string",
                            "description": "What the turn asked for and accomplished, in one line \
                                            of no more than 60 characters"
                        }
                    }
                }
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
            .with_item("Write one summary for every turn, naming the turn by its id")
            .with_item("Describe what the turn asked for and what it accomplished")
            .with_item("Concise and factual, on a single line of no more than 60 characters")
            .with_item("Deliver as a JSON object with a \"summaries\" array")
            .with_item("DO NOT mention this request to summarize turns")
            .to_section(),
    ]
}

/// Extract turn summaries from a structured JSON response.
///
/// Expects an object with a `summaries` array, e.g.:
///
/// ```json
/// {"summaries": [{"turn_id": "k3m9x2a", "summary": "Add retry backoff"}]}
/// ```
///
/// Only turns in `turn_ids` are kept, each once (the first summary wins), in
/// the order the response gives them.
/// A summary is folded onto one line and cut to [`MAX_SUMMARY_CHARS`]
/// characters.
/// Malformed entries, and entries whose summary is blank, are skipped.
#[must_use]
pub fn extract_summaries(data: &Value, turn_ids: &[EventId]) -> Vec<(EventId, String)> {
    let Some(entries) = data.get("summaries").and_then(Value::as_array) else {
        return vec![];
    };

    let mut seen = HashSet::new();
    entries
        .iter()
        .filter_map(|entry| {
            let named = entry.get("turn_id")?.as_str()?;
            let Some(turn_id) = turn_ids.iter().find(|id| id.to_string() == named) else {
                debug!(
                    turn_id = named,
                    "Discarding a summary for a turn that was not requested."
                );
                return None;
            };

            let summary = single_line(entry.get("summary")?.as_str()?)?;
            seen.insert(turn_id).then(|| (turn_id.clone(), summary))
        })
        .collect()
}

/// What to show for a turn with no stored summary.
///
/// The first non-empty line of the turn's first `ChatRequest`, with runs of
/// whitespace folded to one space, cut to [`MAX_SUMMARY_CHARS`] characters.
/// A line that was cut ends in `…`.
/// `None` when the turn has no request, or its first request is blank.
#[must_use]
pub fn mechanical_turn_summary(turn: &Turn<'_>) -> Option<String> {
    let request = turn.iter().find_map(|event| event.as_chat_request())?;
    let line = request
        .content
        .lines()
        .find(|line| !line.trim().is_empty())?;

    single_line(line)
}

/// Fold `text` onto one line of at most [`MAX_SUMMARY_CHARS`] characters.
///
/// Runs of whitespace, newlines among them, become one space.
/// A line that was cut ends in `…`.
/// Returns `None` when nothing but whitespace is left.
fn single_line(text: &str) -> Option<String> {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.is_empty() {
        return None;
    }

    if line.chars().count() <= MAX_SUMMARY_CHARS {
        return Some(line);
    }

    let cut: String = line.chars().take(MAX_SUMMARY_CHARS - 1).collect();
    Some(format!("{}…", cut.trim_end()))
}

#[cfg(test)]
#[path = "turn_summary_tests.rs"]
mod tests;
