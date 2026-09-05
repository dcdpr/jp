//! Conversation-specific configuration for Jean-Pierre.

pub mod attachment;
pub mod compaction;
pub mod label;
pub mod title;
pub mod tool;

use std::{fmt, str::FromStr};

use schematic::{Config, ConfigError, HandlerError, Schematic};
use serde::{Deserialize, Serialize};

use crate::{
    BoxedError,
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    assistant::{AssistantConfig, PartialAssistantConfig},
    conversation::{
        attachment::{AttachmentConfig, PartialAttachmentConfig},
        compaction::{CompactionConfig, PartialCompactionConfig},
        label::{LabelConfig, PartialLabelConfig},
        title::{PartialTitleConfig, TitleConfig},
        tool::{PartialToolsConfig, ToolsConfig},
    },
    delta::{PartialConfigDelta, delta_opt, path},
    fill::FillDefaults,
    internal::merge::{map_with_strategy, vec_with_strategy},
    partial::{ToPartial, partial_opt},
    types::{
        map::{MergeableMap, MergedMap, MergedMapStrategy, map_to_mergeable_partial},
        vec::{MergeableVec, MergedVec, vec_to_mergeable_partial},
    },
    validate::Validator,
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

    /// Compaction configuration.
    ///
    /// Controls how conversation compaction works, including rules for
    /// stripping reasoning, tool calls, and summarization.
    #[setting(nested)]
    pub compaction: CompactionConfig,

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

    /// Label rules applied to conversations.
    ///
    /// Each entry declares how one label's value is produced and when it is
    /// applied; the map key is the label key.
    /// See the `conversation.labels` section for the accepted shapes.
    #[setting(nested, merge = map_with_strategy)]
    pub labels: MergeableMap<LabelConfig>,

    /// Inquiry configuration.
    ///
    /// Controls the assistant model and settings used when a tool asks the
    /// assistant a question (via `QuestionTarget::Assistant`).
    #[setting(nested)]
    pub inquiry: InquiryConfig,

    /// Whether new conversations start local-only.
    ///
    /// A local conversation is kept out of the workspace's `.jp/conversations/`
    /// directory, so it is never committed to version control.
    /// Defaults to `false`, where new conversations are projected into the
    /// workspace and can be committed.
    /// Equivalent to passing `--local` to `jp query`.
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

impl Validator for ConversationConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        self.tools.validate()?;

        for key in self.labels.keys() {
            label::validate_key(key)
                .map_err(|error| HandlerError::new(format!("conversation.labels: {error}")))?;
        }

        Ok(())
    }
}

impl AssignKeyValue for PartialConversationConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            _ if kv.p("title") => self.title.assign(kv)?,
            _ if kv.p("tools") => self.tools.assign(kv)?,
            _ if kv.p("compaction") => self.compaction.assign(kv)?,
            _ if kv.p("attachments") => kv.try_vec_of_nested(self.attachments.as_mut())?,
            _ if kv.p("labels") => kv.assign_to_entry(&mut self.labels)?,
            _ if kv.p("inquiry") => self.inquiry.assign(kv)?,
            _ if kv.p("start_local") => self.start_local = kv.try_some_bool()?,
            "default_id" => self.default_id = kv.try_some_from_str()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConversationConfig {
    /// The attachments `next` adds.
    fn attachments_delta(
        &self,
        next: &MergeableVec<PartialAttachmentConfig>,
    ) -> MergeableVec<PartialAttachmentConfig> {
        next.iter()
            .filter(|v| !self.attachments.contains(v))
            .cloned()
            .collect::<Vec<_>>()
            .into()
    }

    /// The label rules `next` changes.
    fn labels_delta(
        &self,
        next: MergeableMap<PartialLabelConfig>,
    ) -> MergeableMap<PartialLabelConfig> {
        // A key in the previous state that is absent from the next one
        // can only have been dropped by a replacing layer, and a
        // minimal delta has no way to spell "removed": it carries
        // entries, and a missing entry means "unchanged". Emit the
        // whole wrapper in that case so the fold replaces the map
        // instead of deep-merging the dropped rule back in.
        let dropped = self.labels.keys().any(|key| !next.contains_key(key));

        if dropped {
            // Force replace semantics rather than trusting the shape
            // `next` arrived in: a plain `Map` deep-merges on the fold
            // and resurrects the dropped rule.
            MergeableMap::Merged(MergedMap {
                value: next.into_map(),
                strategy: Some(MergedMapStrategy::Replace),
                discard_when_merged: false,
            })
        } else {
            next.into_iter()
                .filter_map(|(key, next)| match self.labels.get(&key) {
                    Some(prev) if prev == &next => None,
                    Some(prev) => Some((key, prev.delta(next))),
                    None => Some((key, next)),
                })
                .collect()
        }
    }
}

impl PartialConfigDelta for PartialConversationConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            title: self.title.delta(next.title),
            tools: self.tools.delta(next.tools),
            compaction: self.compaction.delta(next.compaction),
            attachments: self.attachments_delta(&next.attachments),
            inquiry: self.inquiry.delta(next.inquiry),
            start_local: delta_opt(self.start_local.as_ref(), next.start_local),
            default_id: delta_opt(self.default_id.as_ref(), next.default_id),
            labels: self.labels_delta(next.labels),
        }
    }

    fn delta_with_unsets(&self, next: Self, prefix: &str, unsets: &mut Vec<String>) -> Self {
        Self {
            title: self
                .title
                .delta_with_unsets(next.title, &path(prefix, "title"), unsets),
            tools: self.tools.delta(next.tools),
            compaction: self.compaction.delta(next.compaction),
            attachments: self.attachments_delta(&next.attachments),
            inquiry: self
                .inquiry
                .delta_with_unsets(next.inquiry, &path(prefix, "inquiry"), unsets),
            start_local: delta_opt(self.start_local.as_ref(), next.start_local),
            default_id: delta_opt(self.default_id.as_ref(), next.default_id),
            labels: self.labels_delta(next.labels),
        }
    }
}

impl FillDefaults for PartialConversationConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            title: self.title.fill_from(defaults.title),
            tools: self.tools.fill_from(defaults.tools),
            compaction: self.compaction.fill_from(defaults.compaction),
            attachments: self.attachments.fill_from(defaults.attachments),
            labels: self.labels.fill_from(defaults.labels),
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
            compaction: self.compaction.to_partial(),
            attachments: vec_to_mergeable_partial(&self.attachments),
            labels: map_to_mergeable_partial(self.labels.iter()),
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
    /// Assistant settings for inquiry requests.
    ///
    /// Accepts every key from the top-level `assistant` section.
    /// Keys left unset here take the value from `assistant`, so
    /// `conversation.inquiry.assistant.model.id` can point an inquiry at a
    /// cheaper model while its system prompt and request settings stay whatever
    /// the main assistant uses.
    #[setting(nested)]
    pub assistant: AssistantConfig,
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

    fn delta_with_unsets(&self, next: Self, prefix: &str, unsets: &mut Vec<String>) -> Self {
        Self {
            assistant: self.assistant.delta_with_unsets(
                next.assistant,
                &path(prefix, "assistant"),
                unsets,
            ),
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

/// Which conversation to default to when no session mapping exists.
///
/// This is read during conversation resolution, before the full config is
/// built.
/// It cannot be set per-conversation (circular dependency).
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
