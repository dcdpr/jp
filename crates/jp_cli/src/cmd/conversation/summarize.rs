//! LLM-assisted conversation summarization for compaction.

use jp_config::{
    AppConfig, PartialAppConfig, ToPartial as _, conversation::compaction::SummaryConfig,
};
use jp_conversation::{
    ConversationEvent, ConversationStream,
    event::{ChatRequest, ChatResponse},
    thread::ThreadBuilder,
};
use jp_llm::{
    event::Event,
    event_builder::EventBuilder,
    provider,
    retry::{RetryConfig, collect_with_retry},
};

use crate::error::Result;

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

    let thread = ThreadBuilder::default()
        .with_events(stream.clone())
        .with_system_prompt(instructions.to_owned())
        .build()?;

    let mut thread_events = thread.events.clone();
    thread_events.start_turn(ChatRequest::from("Summarize the conversation above."));

    let query = jp_llm::query::ChatQuery {
        thread: jp_conversation::thread::Thread {
            events: thread_events,
            ..thread
        },
        tools: vec![],
        tool_choice: jp_config::assistant::tool_choice::ToolChoice::default(),
    };

    let provider = provider::get_provider(model_id.provider, &app_cfg.providers.llm)?;
    let model_details = provider.model_details(&model_id.name).await?;

    let retry_config = RetryConfig::default();
    let llm_events =
        collect_with_retry(provider.as_ref(), &model_details, query, &retry_config).await?;

    // Collect the response text.
    let mut builder = EventBuilder::new();
    let mut flushed = Vec::new();
    for event in llm_events {
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
            Event::Finished(_) => flushed.extend(builder.drain()),
            // `Patch` is applied upstream; `KeepAlive` is a liveness signal.
            Event::Patch(_) | Event::KeepAlive => {}
        }
    }

    let summary = flushed
        .into_iter()
        .filter_map(ConversationEvent::into_chat_response)
        .filter_map(|r| match r {
            ChatResponse::Message { message } => Some(message),
            _ => None,
        })
        .collect::<String>();

    if summary.is_empty() {
        return Err(crate::error::Error::Compaction(
            "Summarizer returned an empty response".into(),
        ));
    }

    Ok(summary)
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
