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
/// `SummaryPolicy`. The summarizer reads the raw (non-compacted) events.
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

    let model_id = model.id.resolved();

    // Build a stream containing only the events in the target range.
    let mut summary_stream = events.clone();
    summary_stream.retain_last_turns(0); // clear events, keep base config
    let mut turn_idx = 0;
    let mut in_range = false;
    let mut range_events: Vec<ConversationEvent> = Vec::new();

    for event_with_cfg in events.iter() {
        if event_with_cfg.event.is_turn_start() {
            if turn_idx > 0 || in_range {
                turn_idx += 1;
            }
            in_range = turn_idx >= range_from && turn_idx <= range_to;
            if !in_range && turn_idx > range_to {
                break;
            }
        }

        // The first TurnStart sets in_range without incrementing.
        if turn_idx == 0 && event_with_cfg.event.is_turn_start() {
            in_range = range_from == 0;
        }

        if in_range {
            range_events.push(event_with_cfg.event.clone());
        }
    }

    // Rebuild a clean stream with just the range events.
    let mut stream = ConversationStream::new(events.base_config());
    stream.extend(range_events);

    // Override the model in the stream config so the provider picks up the
    // summary model.
    let mut partial = PartialAppConfig::empty();
    partial.assistant.model.id =
        jp_config::model::id::PartialModelIdOrAliasConfig::Id(model_id.to_partial());
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
            Event::Patch(_) => {}
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
