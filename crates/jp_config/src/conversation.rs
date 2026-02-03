//! Conversation-specific configuration for Jean-Pierre.

pub mod attachment;
pub mod title;
pub mod tool;

use schematic::Config;

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    conversation::{
        attachment::AttachmentConfig,
        title::{PartialTitleConfig, TitleConfig},
        tool::{PartialToolsConfig, ToolsConfig},
    },
    delta::PartialConfigDelta,
    partial::ToPartial,
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
}

impl AssignKeyValue for PartialConversationConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            _ if kv.p("title") => self.title.assign(kv)?,
            _ if kv.p("tools") => self.tools.assign(kv)?,
            _ if kv.p("attachments") => kv.try_vec_of_nested(&mut self.attachments)?,
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
        }
    }
}

impl ToPartial for ConversationConfig {
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            title: self.title.to_partial(),
            tools: self.tools.to_partial(),
            attachments: self.attachments.iter().map(ToPartial::to_partial).collect(),
        }
    }
}
