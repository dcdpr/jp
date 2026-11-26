//! Assemble the [`Thread`] for a provider request from the conversation stream,
//! attachments, and [`AssistantConfig`] system prompt and sections.

use jp_attachment::Attachment;
use jp_config::assistant::{
    AssistantConfig, instructions::InstructionsConfig, sections::SectionConfig,
};
use jp_conversation::{
    ConversationStream,
    thread::{Thread, ThreadBuilder},
};

use crate::error::Result;

/// Build the sorted list of system prompt sections from assistant config.
///
/// Used by both [`build_thread`] and [`LlmInquiryBackend`] construction to
/// ensure the inquiry backend sees the same sections as the main thread.
///
/// [`LlmInquiryBackend`]: crate::cmd::query::tool::inquiry::LlmInquiryBackend
pub(super) fn build_sections(assistant: &AssistantConfig, has_tools: bool) -> Vec<SectionConfig> {
    let mut sections: Vec<_> = assistant.system_prompt_sections.clone();
    sections.extend(
        assistant
            .instructions
            .iter()
            .map(InstructionsConfig::to_section),
    );

    if has_tools {
        let tool_section = InstructionsConfig::default()
            .with_title("Tool Usage")
            .with_description("How to leverage the tools available to you.".to_string())
            .with_item("Use all the tools available to you to give the best possible answer.")
            .with_item("Verify the tool name, description and parameters are correct.")
            .with_item(
                "Even if you've reasoned yourself towards a solution, use any available tool to \
                 verify your answer.",
            )
            .to_section();

        sections.push(tool_section);
    }

    sections.sort_by_key(|s| s.position);
    sections
}

pub(super) fn build_thread(
    events: ConversationStream,
    attachments: Vec<Attachment>,
    assistant: &AssistantConfig,
    has_tools: bool,
) -> Result<Thread> {
    let sections = build_sections(assistant, has_tools);

    let mut thread_builder = ThreadBuilder::default()
        .with_sections(sections)
        .with_attachments(attachments)
        .with_events(events);

    if let Some(system_prompt) = assistant.system_prompt.clone() {
        thread_builder = thread_builder.with_system_prompt(system_prompt);
    }

    Ok(thread_builder.build()?)
}
