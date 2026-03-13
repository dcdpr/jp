//! Conversation-specific configuration for Jean-Pierre.

pub mod attachment;
pub mod title;
pub mod tool;

use schematic::Config;

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    assistant::{
        request::{PartialRequestConfig, RequestConfig},
        sections::SectionConfig,
        tool_choice::ToolChoice,
    },
    conversation::{
        attachment::AttachmentConfig,
        title::{PartialTitleConfig, TitleConfig},
        tool::{PartialToolsConfig, ToolsConfig},
    },
    delta::{PartialConfigDelta, delta_opt, delta_opt_partial, delta_vec},
    model::{ModelConfig, PartialModelConfig},
    partial::{ToPartial, partial_opts},
};

/// Conversation-specific configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct ConversationConfig {
    /// Title configuration.
    ///
    /// This section configures how conversation titles are generated.
    #[setting(nested)]
    pub title: TitleConfig,

    /// Tool configuration.
    ///
    /// This section configures tool usage within conversations.
    #[setting(nested)]
    pub tools: ToolsConfig,

    /// Attachment configuration.
    ///
    /// This section defines attachments (files, resources) that are added to
    /// conversations.
    #[setting(nested, merge = schematic::merge::append_vec)]
    pub attachments: Vec<AttachmentConfig>,

    /// Inquiry configuration.
    ///
    /// Controls the assistant model and settings used when a tool asks the
    /// assistant a question (via `QuestionTarget::Assistant`).
    #[setting(nested)]
    pub inquiry: InquiryConfig,
}

impl AssignKeyValue for PartialConversationConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            _ if kv.p("title") => self.title.assign(kv)?,
            _ if kv.p("tools") => self.tools.assign(kv)?,
            _ if kv.p("attachments") => kv.try_vec_of_nested(&mut self.attachments)?,
            _ if kv.p("inquiry") => self.inquiry.assign(kv)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialConversationConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            title: self.title.delta(next.title),
            tools: self.tools.delta(next.tools),
            attachments: {
                next.attachments
                    .into_iter()
                    .filter(|v| !self.attachments.contains(v))
                    .collect()
            },
            inquiry: self.inquiry.delta(next.inquiry),
        }
    }
}

impl ToPartial for ConversationConfig {
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            title: self.title.to_partial(),
            tools: self.tools.to_partial(),
            attachments: self.attachments.iter().map(ToPartial::to_partial).collect(),
            inquiry: self.inquiry.to_partial(),
        }
    }
}

/// Inquiry-specific configuration.
///
/// Controls the model and settings used when a tool routes a question to the
/// assistant instead of the user.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct InquiryConfig {
    /// Assistant overrides for inquiry requests.
    ///
    /// Unset fields fall back to the parent assistant config.
    #[setting(nested)]
    pub assistant: AssistantOverrideConfig,
}

impl AssignKeyValue for PartialInquiryConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            _ if kv.p("assistant") => self.assistant.assign(kv)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialInquiryConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            assistant: self.assistant.delta(next.assistant),
        }
    }
}

impl ToPartial for InquiryConfig {
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            assistant: self.assistant.to_partial(),
        }
    }
}

/// Assistant configuration overrides for inquiry requests.
///
/// Mirrors [`AssistantConfig`](crate::assistant::AssistantConfig) but with all
/// fields optional and no defaults. Unset fields are filled from the parent
/// assistant config at runtime.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct AssistantOverrideConfig {
    /// Override the system prompt for inquiry requests.
    pub system_prompt: Option<String>,

    /// Override the system prompt sections.
    #[setting(nested, merge = schematic::merge::append_vec)]
    pub system_prompt_sections: Vec<SectionConfig>,

    /// Override the tool choice.
    pub tool_choice: Option<ToolChoice>,

    /// Override the model.
    #[setting(nested)]
    pub model: Option<ModelConfig>,

    /// Override request behavior (retries, caching).
    #[setting(nested)]
    pub request: Option<RequestConfig>,
}

impl AssignKeyValue for PartialAssistantOverrideConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "system_prompt" => self.system_prompt = kv.try_some_string()?,
            _ if kv.p("system_prompt_sections") => {
                kv.try_vec_of_nested(&mut self.system_prompt_sections)?;
            }
            "tool_choice" => self.tool_choice = kv.try_some_from_str()?,
            _ if kv.p("model") => self.model.assign(kv)?,
            _ if kv.p("request") => self.request.assign(kv)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialAssistantOverrideConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            system_prompt: delta_opt(self.system_prompt.as_ref(), next.system_prompt),
            system_prompt_sections: delta_vec(
                &self.system_prompt_sections,
                next.system_prompt_sections,
            ),
            tool_choice: delta_opt(self.tool_choice.as_ref(), next.tool_choice),
            model: delta_opt_partial(self.model.as_ref(), next.model),
            request: delta_opt_partial(self.request.as_ref(), next.request),
        }
    }
}

impl ToPartial for AssistantOverrideConfig {
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            system_prompt: partial_opts(self.system_prompt.as_ref(), None),
            system_prompt_sections: self
                .system_prompt_sections
                .iter()
                .map(ToPartial::to_partial)
                .collect(),
            tool_choice: partial_opts(self.tool_choice.as_ref(), None),
            model: self.model.as_ref().map(ToPartial::to_partial),
            request: self.request.as_ref().map(ToPartial::to_partial),
        }
    }
}
