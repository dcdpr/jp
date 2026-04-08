//! Assistant-specific configuration for Jean-Pierre.
//!
//! These configuration options tweak the behavior of the assistant. The
//! "assistant" is defined as the technique powering the response generation
//! (typically a GPT/LLM model), with additional options built on top for
//! improved performance.

pub mod instructions;
pub mod request;
pub mod sections;
pub mod tool_choice;

use schematic::{Config, TransformResult};

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    assistant::{
        instructions::{InstructionsConfig, PartialInstructionsConfig},
        request::{PartialRequestConfig, RequestConfig},
        sections::{PartialSectionConfig, SectionConfig},
        tool_choice::ToolChoice,
    },
    delta::{PartialConfigDelta, delta_opt, delta_opt_partial},
    internal::merge::{string_with_strategy, vec_with_strategy},
    model::{ModelConfig, PartialModelConfig},
    partial::{ToPartial, partial_opt, partial_opts},
    types::{
        string::{MergeableString, PartialMergeableString, PartialMergedString},
        vec::{MergeableVec, MergedVec, vec_to_mergeable_partial},
    },
};

/// Assistant-specific configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct AssistantConfig {
    /// The name of the assistant.
    ///
    /// This is purely cosmetic and currently not used in the UI.
    pub name: Option<String>,

    /// The system prompt to use for the assistant.
    ///
    /// The system prompt is the initial instruction given to the assistant to
    /// define its behavior, tone, and role.
    #[setting(
        nested,
        partial_via = MergeableString,
        default = default_system_prompt,
        merge = string_with_strategy,
    )]
    pub system_prompt: Option<String>,

    /// A list of system prompt sections for the assistant.
    #[setting(
        nested,
        partial_via = MergeableVec::<SectionConfig>,
        default = default_sections,
        merge = vec_with_strategy,
    )]
    pub system_prompt_sections: Vec<SectionConfig>,

    /// A list of instructions for the assistant.
    ///
    /// Instructions are similar to system prompts but are organized into a list
    /// of titled sections. This allows for better organization and easier
    /// overriding or extending of specific instructions when merging multiple
    /// configurations.
    #[setting(
        nested,
        partial_via = MergeableVec::<InstructionsConfig>,
        default = default_instructions,
        merge = vec_with_strategy,
    )]
    pub instructions: Vec<InstructionsConfig>,

    /// How the assistant should choose tools to call.
    #[setting(default)]
    pub tool_choice: ToolChoice,

    /// LLM model configuration.
    #[setting(nested)]
    pub model: ModelConfig,

    /// LLM request behavior configuration.
    ///
    /// Controls retry logic for transient errors like rate limits, timeouts,
    /// and connection failures.
    #[setting(nested)]
    pub request: RequestConfig,
}

impl AssignKeyValue for PartialAssistantConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "name" => self.name = kv.try_some_string()?,
            "system_prompt" => self.system_prompt = kv.try_some_object_or_from_str()?,
            _ if kv.p("instructions") => kv.try_vec_of_nested(self.instructions.as_mut())?,
            _ if kv.p("system_prompt_sections") => {
                kv.try_vec_of_nested(self.system_prompt_sections.as_mut())?;
            }
            "tool_choice" => self.tool_choice = kv.try_some_from_str()?,
            _ if kv.p("model") => self.model.assign(kv)?,
            _ if kv.p("request") => self.request.assign(kv)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialAssistantConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            name: delta_opt(self.name.as_ref(), next.name),
            system_prompt: delta_opt_partial(self.system_prompt.as_ref(), next.system_prompt),
            instructions: next
                .instructions
                .into_iter()
                .filter(|v| !self.instructions.contains(v))
                .collect::<Vec<_>>()
                .into(),
            system_prompt_sections: next
                .system_prompt_sections
                .into_iter()
                .filter(|v| !self.system_prompt_sections.contains(v))
                .collect(),
            tool_choice: delta_opt(self.tool_choice.as_ref(), next.tool_choice),
            model: self.model.delta(next.model),
            request: self.request.delta(next.request),
        }
    }
}

impl ToPartial for AssistantConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            name: partial_opts(self.name.as_ref(), defaults.name),
            system_prompt: self
                .system_prompt
                .as_ref()
                .map(|v| PartialMergeableString::String(v.clone())),
            instructions: vec_to_mergeable_partial(&self.instructions),
            system_prompt_sections: vec_to_mergeable_partial(&self.system_prompt_sections),
            tool_choice: partial_opt(&self.tool_choice, defaults.tool_choice),
            model: self.model.to_partial(),
            request: self.request.to_partial(),
        }
    }
}

/// The default instructions for the assistant.
#[expect(clippy::trivially_copy_pass_by_ref, clippy::unnecessary_wraps)]
fn default_instructions(_: &()) -> TransformResult<MergeableVec<PartialInstructionsConfig>> {
    Ok(MergeableVec::Merged(MergedVec {
        strategy: None,
        discard_when_merged: true,
        value: vec![PartialInstructionsConfig {
            title: Some("How to respond to the user".into()),
            items: Some(vec![
                "Be concise".into(),
                "Use simple sentences. But feel free to use technical jargon.".into(),
                "Do NOT overexplain basic concepts. Assume the user is technically proficient."
                    .into(),
                "AVOID flattering, corporate-ish or marketing language. Maintain a neutral \
                 viewpoint."
                    .into(),
                "AVOID vague and / or generic claims which may seem correct but are not \
                 substantiated by the context."
                    .into(),
            ]),
            ..Default::default()
        }],
    }))
}

/// The default instructions for the assistant.
#[expect(clippy::trivially_copy_pass_by_ref, clippy::unnecessary_wraps)]
const fn default_sections(_: &()) -> TransformResult<MergeableVec<PartialSectionConfig>> {
    Ok(MergeableVec::Vec(vec![]))
}

/// The default system prompt for the assistant.
#[expect(clippy::trivially_copy_pass_by_ref, clippy::unnecessary_wraps)]
fn default_system_prompt(_: &()) -> TransformResult<Option<PartialMergeableString>> {
    Ok(Some(PartialMergeableString::Merged(PartialMergedString {
        value: Some("You are a helpful assistant.".to_owned()),
        strategy: None,
        separator: None,
        discard_when_merged: Some(true),
    })))
}

#[cfg(test)]
#[path = "assistant_tests.rs"]
mod tests;
