//! Conversation title generation.
//!
//! [`generate`] is the entry point: it shapes a conversation into a structured
//! output request and returns the candidate titles the model produced.
//! [`resolve_model`] picks the model that request runs on.
//!
//! The schema and instruction helpers are public so callers can measure or
//! inspect the request they are about to make.

use std::sync::Arc;

use jp_config::{
    AppConfig,
    assistant::{
        instructions::InstructionsConfig, sections::SectionConfig, tool_choice::ToolChoice,
    },
    model::{
        ModelConfig,
        id::ModelIdOrAliasConfig,
        parameters::{CustomReasoningConfig, ParametersConfig, ReasoningEffort},
    },
};
use jp_conversation::{ConversationStream, event::ChatRequest, thread::ThreadBuilder};
use serde_json::{Map, Value, json};

use crate::{
    Provider,
    error::Result,
    event_builder,
    model::ModelDetails,
    query::ChatQuery,
    retry::{RetryConfig, collect_with_retry},
    window,
};

/// A request for LLM-generated conversation titles.
#[derive(Debug)]
pub struct TitleRequest {
    /// The conversation to generate titles for.
    ///
    /// Taken by value: the request appends its own turn and may drop older
    /// events to fit the model's context window, so callers pass a clone of the
    /// stream they want to keep.
    pub events: ConversationStream,

    /// The model the request runs on, with the parameters it runs under.
    ///
    /// These parameters are the only ones the request carries: reasoning
    /// effort, max tokens, temperature and provider-specific values are taken
    /// from here and never inherited from the conversation.
    /// See [`resolve_model`].
    pub model: ModelConfig,

    /// How many candidate titles to ask for.
    pub count: usize,

    /// Titles the user already rejected, which the model must avoid.
    pub rejected: Vec<String>,
}

/// Resolve the model that conversation-title generation runs on.
///
/// `override_id` wins when set; otherwise `conversation.title.generate.model`
/// is used, falling back to the assistant model's ID.
/// Parameters come from `conversation.title.generate.model` when it is
/// configured and start fresh otherwise, so a title request never inherits the
/// conversation's own parameters.
///
/// Reasoning defaults to low effort with the trace excluded: a title is a short
/// factual summary, and deep reasoning on every new conversation rarely earns
/// its cost.
/// An explicit reasoning setting on the title model is preserved.
#[must_use]
pub fn resolve_model(
    config: &AppConfig,
    override_id: Option<&ModelIdOrAliasConfig>,
) -> ModelConfig {
    let mut model = config
        .conversation
        .title
        .generate
        .model
        .clone()
        .unwrap_or_else(|| ModelConfig {
            id: config.assistant.model.id.clone(),
            parameters: ParametersConfig::default(),
        });

    if let Some(id) = override_id {
        model.id = id.clone();
    }

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

/// Generate candidate titles for a conversation.
///
/// `details` describes the model named by `request.model`; its context window
/// bounds the request, with older events dropped when the conversation doesn't
/// fit (see [`window::truncate_to_fit`]).
///
/// Returns the titles in the order the model produced them, or an empty vec
/// when the response carried no structured data — callers decide whether that
/// is an error.
///
/// # Errors
///
/// Returns an error if the thread cannot be built or the provider request fails
/// after exhausting its retries.
pub async fn generate(
    provider: &dyn Provider,
    details: &ModelDetails,
    request: TitleRequest,
) -> Result<Vec<String>> {
    let TitleRequest {
        mut events,
        model,
        count,
        rejected,
    } = request;

    let sections = title_instructions(count, &rejected);

    // Resolve compaction overlays up front: `rebase_on_model` carries
    // conversation events only, so a stored summary has to already be part of
    // the event list by the time it runs.
    events.apply_projection();

    // The request carries no system prompt and no attachments, so the
    // instruction sections are the only fixed overhead sharing the window.
    if let Some(context_window) = details.context_window {
        let overhead = window::estimate_overhead_chars(None, &sections, &[], &[]);
        window::truncate_to_fit(&mut events, context_window, overhead);
    }

    let mut thread = ThreadBuilder::default()
        .with_events(rebase_on_model(&events, model))
        .with_sections(sections)
        .build()?;

    thread.events.start_turn(ChatRequest {
        content: if count == 1 {
            "Generate a title for this conversation.".into()
        } else {
            "Generate titles for this conversation.".into()
        },
        schema: Some(title_schema(count)),
        author: None,
    });

    let query = ChatQuery {
        thread,
        tools: vec![],
        tool_choice: ToolChoice::default(),
    };

    let events = collect_with_retry(provider, details, query, &RetryConfig::default()).await?;

    Ok(event_builder::structured_data(events)
        .as_ref()
        .map(extract_titles)
        .unwrap_or_default())
}

/// Rebuild `events` so `model` is the only assistant model the request sees.
///
/// A config delta cannot express "unset": merging one carries every parameter
/// the conversation had set but `model` leaves unset, so an assistant
/// `max_tokens` outlives the switch to a smaller title model and is sent to it.
/// Putting `model` in the base config and carrying the events over without
/// their deltas replaces the parameters outright.
///
/// Dropping the deltas costs nothing here: the request has no tools and no
/// attachments, so the assistant model is the only config it reads.
///
/// Only conversation events are carried over, so `events` must already be
/// projected (see [`ConversationStream::apply_projection`]) or its compaction
/// overlays are lost.
fn rebase_on_model(events: &ConversationStream, model: ModelConfig) -> ConversationStream {
    let mut base = (*events.base_config()).clone();
    base.assistant.model = model;

    let mut rebased = ConversationStream::new(Arc::new(base));
    rebased.extend(events.iter().map(|e| e.event.clone()));
    rebased
}

/// JSON schema for the title generation structured output.
///
/// Returns a schema requiring an object with a `titles` array of exactly
/// `count` string elements.
#[must_use]
#[allow(clippy::missing_panics_doc)]
pub fn title_schema(count: usize) -> Map<String, Value> {
    let schema = json!({
        "type": "object",
        "required": ["titles"],
        "additionalProperties": false,
        "properties": {
            "titles": {
                "type": "array",
                "items": {
                    "type": "string",
                    "description": "A concise, descriptive title for the conversation"
                },
                "minItems": count,
                "maxItems": count,
            },
        },
    });

    schema
        .as_object()
        .expect("schema is always an object")
        .clone()
}

/// Build instruction sections for title generation.
///
/// Returns one or two sections: the main generation instructions, and
/// optionally a "rejected titles" section if `rejected` is non-empty.
#[must_use]
pub fn title_instructions(count: usize, rejected: &[String]) -> Vec<SectionConfig> {
    let mut sections = vec![
        InstructionsConfig::default()
            .with_title("Title Generation")
            .with_description("Generate titles to summarize the active conversation")
            .with_item(format!("Generate exactly {count} titles"))
            .with_item("Concise, descriptive, factual")
            .with_item("Short and to the point, no more than 50 characters")
            .with_item("Deliver as a JSON object with a \"titles\" array of strings")
            .with_item("DO NOT mention this request to generate titles")
            .to_section(),
    ];

    if !rejected.is_empty() {
        let mut rejected_instruction = InstructionsConfig::default()
            .with_title("Rejected Titles")
            .with_description("These listed titles were rejected by the user and must be avoided");

        for title in rejected {
            rejected_instruction = rejected_instruction.with_item(title);
        }

        sections.push(rejected_instruction.to_section());
    }

    sections
}

/// Extract title strings from a structured JSON response.
///
/// Expects a JSON object with a `titles` array of strings, e.g.:
///
/// ```json
/// {"titles": ["My Title", "Another Title"]}
/// ```
///
/// Returns an empty vec if the structure doesn't match.
#[must_use]
pub fn extract_titles(data: &Value) -> Vec<String> {
    data.get("titles")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "title_tests.rs"]
mod tests;
