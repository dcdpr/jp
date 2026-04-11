//! Conversation-specific configuration for Jean-Pierre.

pub mod attachment;
pub mod title;
pub mod tool;

use std::{fmt, str::FromStr};

use schematic::{Config, Schematic};
use serde::{Deserialize, Serialize};

use crate::{
    BoxedError,
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    assistant::{
        request::{PartialRequestConfig, RequestConfig},
        sections::SectionConfig,
        tool_choice::ToolChoice,
    },
    conversation::{
        attachment::{AttachmentConfig, PartialAttachmentConfig},
        title::{PartialTitleConfig, TitleConfig},
        tool::{PartialToolsConfig, ToolsConfig},
    },
    delta::{PartialConfigDelta, delta_opt, delta_opt_partial, delta_vec},
    fill::{self, FillDefaults},
    internal::merge::vec_with_strategy,
    model::{ModelConfig, PartialModelConfig},
    partial::{ToPartial, partial_opt, partial_opts},
    types::vec::{MergeableVec, MergedVec, vec_to_mergeable_partial},
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
    #[setting(
        nested,
        partial_via = MergeableVec::<AttachmentConfig>,
        default = default_attachments,
        merge = vec_with_strategy,
    )]
    pub attachments: Vec<AttachmentConfig>,

    /// Inquiry configuration.
    ///
    /// Controls the assistant model and settings used when a tool asks the
    /// assistant a question (via `QuestionTarget::Assistant`).
    #[setting(nested)]
    pub inquiry: InquiryConfig,

    /// Whether to store new conversations in the user-local workspace storage.
    #[setting(default)]
    pub start_local: bool,

    /// Default conversation to target when no session mapping exists and no
    /// `--id` flag is provided.
    ///
    /// - `ask`: show an interactive picker or error in non-interactive mode
    /// - `last-activated` / `last`: most recently activated conversation
    /// - `last-created`: most recently created conversation
    /// - `previous` / `prev`: session's previously active conversation
    /// - `jp-c...`: a specific conversation ID
    pub default_id: Option<DefaultConversationId>,
}

impl AssignKeyValue for PartialConversationConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            _ if kv.p("title") => self.title.assign(kv)?,
            _ if kv.p("tools") => self.tools.assign(kv)?,
            _ if kv.p("attachments") => kv.try_vec_of_nested(self.attachments.as_mut())?,
            _ if kv.p("inquiry") => self.inquiry.assign(kv)?,
            _ if kv.p("start_local") => self.start_local = kv.try_some_bool()?,
            "default_id" => self.default_id = kv.try_some_from_str()?,
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
                    .collect::<Vec<_>>()
                    .into()
            },
            inquiry: self.inquiry.delta(next.inquiry),
            start_local: delta_opt(self.start_local.as_ref(), next.start_local),
            default_id: delta_opt(self.default_id.as_ref(), next.default_id),
        }
    }
}

impl FillDefaults for PartialConversationConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            title: self.title.fill_from(defaults.title),
            tools: self.tools.fill_from(defaults.tools),
            attachments: self.attachments.fill_from(defaults.attachments),
            inquiry: self.inquiry.fill_from(defaults.inquiry),
            start_local: self.start_local.or(defaults.start_local),
            default_id: self.default_id.or(defaults.default_id),
        }
    }
}

impl ToPartial for ConversationConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            title: self.title.to_partial(),
            tools: self.tools.to_partial(),
            attachments: vec_to_mergeable_partial(&self.attachments),
            inquiry: self.inquiry.to_partial(),
            start_local: partial_opt(&self.start_local, defaults.start_local),
            default_id: self.default_id.clone(),
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

impl FillDefaults for PartialInquiryConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            assistant: self.assistant.fill_from(defaults.assistant),
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

impl FillDefaults for PartialAssistantOverrideConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            system_prompt: self.system_prompt.or(defaults.system_prompt),
            system_prompt_sections: self.system_prompt_sections,
            tool_choice: self.tool_choice.or(defaults.tool_choice),
            model: fill::fill_opt(self.model, defaults.model),
            request: fill::fill_opt(self.request, defaults.request),
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

/// Which conversation to default to when no session mapping exists.
///
/// This is read during conversation resolution, before the full config is
/// built. It cannot be set per-conversation (circular dependency).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Schematic)]
#[serde(rename_all = "snake_case")]
pub enum DefaultConversationId {
    /// Show an interactive picker (TTY) or error (non-interactive).
    #[default]
    Ask,

    /// Most recently activated conversation (any session).
    LastActivated,

    /// Most recently created conversation.
    LastCreated,

    /// Session's previously active conversation.
    Previous,

    /// A specific conversation ID.
    #[serde(skip)]
    Id(String),
}

impl<'de> Deserialize<'de> for DefaultConversationId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl DefaultConversationId {
    /// Returns `true` if this is the default `Ask` variant.
    #[must_use]
    pub const fn is_ask(&self) -> bool {
        matches!(self, Self::Ask)
    }
}

impl FromStr for DefaultConversationId {
    type Err = BoxedError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ask" => Ok(Self::Ask),
            "last" | "last-activated" | "last_activated" => Ok(Self::LastActivated),
            "last-created" | "last_created" => Ok(Self::LastCreated),
            "previous" | "prev" => Ok(Self::Previous),
            _ => Ok(Self::Id(s.to_owned())),
        }
    }
}

impl fmt::Display for DefaultConversationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ask => write!(f, "ask"),
            Self::LastActivated => write!(f, "last-activated"),
            Self::LastCreated => write!(f, "last-created"),
            Self::Previous => write!(f, "previous"),
            Self::Id(id) => write!(f, "{id}"),
        }
    }
}

/// Default attachments: empty vec with dedup enabled.
///
/// The `discard_when_merged: true` means the empty vec is thrown away when real
/// attachments arrive, but the `dedup: Some(true)` flag inherits to the
/// replacement (because `next` has `dedup: None` / "inherit").
#[expect(clippy::trivially_copy_pass_by_ref, clippy::unnecessary_wraps)]
const fn default_attachments(
    _: &(),
) -> schematic::TransformResult<MergeableVec<PartialAttachmentConfig>> {
    Ok(MergeableVec::Merged(MergedVec {
        value: vec![],
        strategy: None,
        dedup: Some(true),
        discard_when_merged: true,
    }))
}

#[cfg(test)]
#[path = "conversation_tests.rs"]
mod tests;
