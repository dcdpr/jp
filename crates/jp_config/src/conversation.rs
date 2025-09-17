//! Conversation-specific configuration for Jean-Pierre.

pub mod title;
pub mod tool;

use schematic::Config;
use serde_json::Value;

use crate::{
    assignment::{missing_key, type_error, AssignKeyValue, AssignResult, KvAssignment},
    conversation::{
        title::{PartialTitleConfig, TitleConfig},
        tool::{PartialToolsConfig, ToolsConfig},
    },
};

/// Conversation-specific configuration.
#[derive(Debug, Config)]
#[config(rename_all = "snake_case")]
pub struct ConversationConfig {
    /// Title configuration.
    #[setting(nested)]
    pub title: TitleConfig,

    /// Tool configuration.
    #[setting(nested)]
    pub tools: ToolsConfig,

    /// Attachment configuration.
    #[setting(default, merge = schematic::merge::append_vec)]
    pub attachments: Vec<url::Url>,
}

impl AssignKeyValue for PartialConversationConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            _ if kv.p("title") => self.title.assign(kv)?,
            _ if kv.p("tools") => self.tools.assign(kv)?,
            _ if kv.p("attachments") => {
                let parser = |kv: KvAssignment| match kv.value.clone().into_value() {
                    Value::String(v) => url::Url::parse(&v).map_err(Into::into),
                    _ => type_error(kv.key(), &kv.value, &["string"]).map_err(Into::into),
                };

                kv.try_vec(self.attachments.get_or_insert_default(), parser)?;
            }
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}
