//! LLM-assisted conversation summarization for compaction.

use jp_config::{
    AppConfig, PartialAppConfig, ToPartial as _, conversation::compaction::SummaryConfig,
    model::id::ModelIdConfig,
};
use jp_conversation::{
    ConversationEvent, ConversationStream,
    event::{ChatRequest, ChatResponse},
    thread::ThreadBuilder,
};
use jp_llm::{
    Provider,
    event::{Event, EventPatch, FinishReason, apply_patches},
    event_builder::EventBuilder,
    model::ModelDetails,
    provider,
    retry::{RetryConfig, collect_with_retry},
    window,
};
use tracing::debug;

use crate::error::{Error, Result};

const DEFAULT_INSTRUCTIONS: &str = "\
Summarize the preceding conversation for continuity. The summary will replace the original \
                                    messages, so it must be self-contained.

Preserve:
- File paths and code structures discussed
- Key decisions and their rationale
- Errors encountered and how they were resolved
- Current task state and next steps
- Any constraints or requirements established

Be concise but thorough. The reader should be able to continue the conversation without having \
                                    seen the original messages.";

/// Generate a summary of the given conversation events using an LLM.
///
/// The summary is a plain text string suitable for storing in a
/// `SummaryPolicy`.
/// The summarizer reads the raw (non-compacted) events.
pub async fn generate_summary(
    events: &ConversationStream,
    range_from: usize,
    range_to: usize,
    summary_cfg: Option<&SummaryConfig>,
    app_cfg: &AppConfig,
) -> Result<String> {
    let model = summary_cfg
        .and_then(|c| c.model.clone())
        .unwrap_or_else(|| app_cfg.assistant.model.clone());

    // Aliases are resolved by `AppConfig::resolve_aliases` (including compaction
    // summary models) before we get here, so `resolved()` is safe. The owned id
    // is reused for provider lookup below.
    let model_id = model.id.resolved().clone();

    let range_events = collect_range_events(events, range_from, range_to);

    // Rebuild a clean stream with just the range events.
    let mut stream = ConversationStream::new(events.base_config());
    stream.extend(range_events);

    // Override the full assistant model (id plus parameters) so a
    // summary-specific model can also set max tokens, temperature, reasoning,
    // and provider-specific parameters — not just the model id.
    let mut partial = PartialAppConfig::empty();
    partial.assistant.model = model.to_partial();
    stream.add_config_delta(partial);

    let instructions = summary_cfg
        .and_then(|c| c.instructions.as_deref())
        .unwrap_or(DEFAULT_INSTRUCTIONS);

    // Extra context is supplementary guidance for this one summary, so it rides
    // on the user turn rather than replacing the (cache-friendly) system prompt.
    let context = summary_cfg
        .and_then(|c| c.context.as_deref())
        .map(str::trim)
        .filter(|c| !c.is_empty());
    let user_message = match context {
        Some(context) => {
            format!("Summarize the conversation above.\n\nAdditional context: {context}")
        }
        None => "Summarize the conversation above.".to_owned(),
    };

    let provider = provider::get_provider(model_id.provider, &app_cfg.providers.llm)?;
    let model_details = provider.model_details(&model_id.name).await?;

    // The instructions ride in the system prompt and the request in its own
    // turn; both share the window with the range.
    let overhead =
        window::estimate_overhead_chars(Some(instructions), &[], &[], &[]) + user_message.len();

    if let Some(overflow) = window_overflow(&stream, model_details.context_window, overhead) {
        return Err(Error::Summarize {
            model: model_id.to_string(),
            // Stored indices are 0-based; turn numbers shown to the user are
            // 1-based.
            reason: format!(
                "turns {}..{} {overflow}; compact a smaller range (`--from`/`--to`) or summarize \
                 with a larger-window model",
                range_from + 1,
                range_to + 1,
            ),
        });
    }

    summarize_stream(
        provider.as_ref(),
        &model_details,
        &model_id,
        stream,
        instructions,
        &user_message,
        app_cfg.assistant.request.max_response_bytes,
    )
    .await
}

/// Describe why `stream` does not fit `context_window`, or `None` when it does.
///
/// A summary stands in for every turn it covers, so a range that doesn't fit is
/// rejected rather than shortened: summarizing only the tail would leave a
/// compaction that claims a range it never read, and the projected conversation
/// would quietly lose the rest.
///
/// `overhead_chars` is the size of everything else sharing the window (see
/// [`window::estimate_overhead_chars`]).
/// An unknown window always fits — there is no budget to measure against.
fn window_overflow(
    stream: &ConversationStream,
    context_window: Option<u32>,
    overhead_chars: usize,
) -> Option<String> {
    let context_window = context_window?;
    let budget = window::budget_chars(context_window, overhead_chars);
    let needed = window::estimate_chars(stream);

    (needed > budget).then(|| {
        format!(
            "are roughly {needed} characters, which exceeds the ~{budget} that fit in the model's \
             {context_window} token context window"
        )
    })
}

/// Request a summary of `stream`, honouring provider rebuild requests.
///
/// A provider that answers with [`FinishReason::Retry`] supplies patches that
/// make the request acceptable; each round applies them to `stream` and
/// resends.
/// Providers degrade a bounded subset of bad events per round (see Anthropic's
/// `build_thinking_patches` and Google's `build_thought_signature_patch`), so a
/// stream carrying several bad events legitimately needs several rounds.
///
/// The loop terminates because every round requires at least one applied patch,
/// and the only action available (`RemoveMetadata`) strictly shrinks a finite
/// set of metadata entries.
/// A round that changes nothing ends it.
async fn summarize_stream(
    provider: &dyn Provider,
    model_details: &ModelDetails,
    model_id: &ModelIdConfig,
    mut stream: ConversationStream,
    instructions: &str,
    user_message: &str,
    max_response_bytes: u32,
) -> Result<String> {
    let retry_config = RetryConfig::default().with_max_response_bytes(max_response_bytes);

    loop {
        let thread = ThreadBuilder::default()
            .with_events(stream.clone())
            .with_system_prompt(instructions.to_owned())
            .build()?;

        let mut thread_events = thread.events.clone();
        thread_events.start_turn(ChatRequest::from(user_message.to_owned()));

        let query = jp_llm::query::ChatQuery {
            thread: jp_conversation::thread::Thread {
                events: thread_events,
                ..thread
            },
            tools: vec![],
            tool_choice: jp_config::assistant::tool_choice::ToolChoice::default(),
        };

        let llm_events = collect_with_retry(provider, model_details, query, &retry_config).await?;

        let patches = match summarize_events(llm_events) {
            StreamOutcome::Summary(summary) => return Ok(summary),
            StreamOutcome::Unusable(reason) => {
                return Err(Error::Summarize {
                    model: model_id.to_string(),
                    reason,
                });
            }
            StreamOutcome::Retry(patches) => patches,
        };

        // The patches target events in the local range stream, so applying them
        // here leaves the stored conversation untouched: repairing the user's
        // history is the query loop's job, not a side effect of summarizing.
        let applied = apply_patches(&mut stream, &patches);
        if applied == 0 {
            return Err(Error::Summarize {
                model: model_id.to_string(),
                reason: "the provider asked to rebuild the request but sent no fix that changed \
                         it, so resending would fail the same way"
                    .to_owned(),
            });
        }

        debug!(
            applied,
            "Provider requested a rebuild; patched the summarizer stream and resending."
        );
    }
}

/// What one completed summarizer stream yielded.
#[derive(Debug, PartialEq)]
enum StreamOutcome {
    /// Usable summary text.
    Summary(String),

    /// The provider wants the request rebuilt and sent again, after applying
    /// these patches to the events it was built from.
    Retry(Vec<EventPatch>),

    /// Nothing usable came back; the string explains why.
    Unusable(String),
}

/// Reduce one summarizer stream to its outcome.
///
/// Only a stream that both finishes with [`FinishReason::Completed`] and
/// carries message text yields a summary.
/// Every other terminal reason is unusable even when text was streamed first: a
/// truncated or declined response would otherwise be stored as the summary and
/// replace the turns it was meant to stand in for, silently dropping whatever
/// the model never got to.
fn summarize_events(events: Vec<Event>) -> StreamOutcome {
    let mut builder = EventBuilder::new();
    let mut flushed = Vec::new();
    let mut patches = Vec::new();
    let mut finish = None;
    for event in events {
        match event {
            Event::Part {
                index,
                part,
                metadata,
            } => {
                builder.handle_part(index, part, metadata);
            }
            Event::Flush { index, metadata } => {
                flushed.extend(builder.handle_flush(index, metadata));
            }
            Event::Finished(reason) => {
                flushed.extend(builder.drain());
                finish = Some(reason);
            }
            // Providers emit patches alongside `FinishReason::Retry`; nothing
            // else consumes them on this path, so keep them for the rebuild.
            Event::Patch(mut p) => patches.append(&mut p),
            // `KeepAlive` is a liveness signal.
            Event::KeepAlive => {}
        }
    }

    if let Some(FinishReason::Retry) = finish {
        return StreamOutcome::Retry(patches);
    }

    let summary = flushed
        .into_iter()
        .filter_map(ConversationEvent::into_chat_response)
        .filter_map(|r| match r {
            ChatResponse::Message { message } => Some(message),
            _ => None,
        })
        .collect::<String>();

    if matches!(finish, Some(FinishReason::Completed)) && !summary.is_empty() {
        return StreamOutcome::Summary(summary);
    }

    StreamOutcome::Unusable(failure_reason(finish.as_ref()))
}

/// Explain why a summarization attempt produced no usable summary.
///
/// `finish` is the stream's terminal reason, or `None` when the stream ended
/// without one.
fn failure_reason(finish: Option<&FinishReason>) -> String {
    match finish {
        Some(FinishReason::Refused {
            category,
            explanation,
        }) => {
            let category = category
                .as_deref()
                .map_or_else(String::new, |c| format!(" ({c})"));
            let explanation = explanation
                .as_deref()
                .map_or_else(String::new, |e| format!(": {e}"));
            format!("the model declined to summarize this conversation{category}{explanation}")
        }
        Some(FinishReason::MaxTokens) => "the model hit its max output token limit, so any \
                                          summary it produced would be truncated"
            .to_owned(),
        Some(FinishReason::Other(value)) => {
            let detail = value
                .as_str()
                .map_or_else(|| value.to_string(), str::to_owned);
            format!("the model stopped early ({detail}), so any summary it produced is incomplete")
        }
        Some(FinishReason::Retry) => "the provider asked to rebuild the request".to_owned(),
        Some(FinishReason::Completed) | None => "the model returned an empty response".to_owned(),
    }
}

/// Collect all events in the inclusive turn range `[range_from, range_to]`.
///
/// Each covered turn contributes its full event sequence, including the leading
/// `TurnStart`.
/// Out-of-range and missing turns contribute nothing.
fn collect_range_events(
    events: &ConversationStream,
    range_from: usize,
    range_to: usize,
) -> Vec<ConversationEvent> {
    events
        .iter_turns()
        .filter(|turn| turn.index() >= range_from && turn.index() <= range_to)
        .flat_map(|turn| turn.into_iter().map(|e| e.event.clone()))
        .collect()
}

#[cfg(test)]
#[path = "summarize_tests.rs"]
mod tests;
