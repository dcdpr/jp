//! User configuration for conversations.

use schematic::Config;

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt},
    fill::FillDefaults,
    partial::{ToPartial, partial_opts},
};

/// User-specific configuration for conversations.
///
/// Captures display attributes of the human contributing to a conversation,
/// used for per-turn attribution in transcripts.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct UserConfig {
    /// Display name of the user contributing to conversations.
    ///
    /// Stamped onto each [`ChatRequest`] event at creation time as
    /// [`author`], so transcripts attribute each turn correctly even when
    /// teammates with different local configs continue the conversation.
    ///
    /// Typically set in user-local config (run `jp init` for an
    /// interactive setup). When unset, transcripts fall back to a generic
    /// `"user"` label.
    ///
    /// [`ChatRequest`]: jp_conversation::event::ChatRequest
    /// [`author`]: jp_conversation::event::ChatRequest::author
    pub name: Option<String>,
}

impl AssignKeyValue for PartialUserConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "name" => self.name = kv.try_some_string()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialUserConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            name: delta_opt(self.name.as_ref(), next.name),
        }
    }
}

impl FillDefaults for PartialUserConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            name: self.name.or(defaults.name),
        }
    }
}

impl ToPartial for UserConfig {
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            name: partial_opts(self.name.as_ref(), None),
        }
    }
}
