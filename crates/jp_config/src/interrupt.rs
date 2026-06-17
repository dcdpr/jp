//! Interrupt (Ctrl-C) behavior configuration.
//!
//! Controls what pressing Ctrl-C does during a query.
//! By default JP shows an interactive menu and lets you choose; each context
//! can instead be set to a fixed action that runs immediately, without the
//! menu.
//!
//! ```toml
//! [interrupt]
//! streaming = "prompt"   # while the assistant is generating content
//! tool_call = "prompt"   # while tools are executing
//! ```

use schematic::{Config, ConfigEnum};
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt},
    fill::FillDefaults,
    partial::{ToPartial, partial_opt},
};

/// Behavior of Ctrl-C during a query.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct InterruptConfig {
    /// What Ctrl-C does while the assistant is generating content.
    ///
    /// - `prompt`: Show the interrupt menu and choose (default).
    /// - `continue`: Resume the response (keep waiting for it, or continue from
    ///   the part already generated if the stream has stopped).
    /// - `stop`: Save the response generated so far and exit.
    /// - `abort`: Discard the in-progress response and exit.
    /// - `reply`: Stop the response and send a new message; what was generated
    ///   so far is kept as context.
    #[setting(default)]
    pub streaming: StreamingInterrupt,

    /// What Ctrl-C does while tools are executing.
    ///
    /// - `prompt`: Show the interrupt menu and choose (default).
    /// - `continue`: Keep waiting for the running tools to finish.
    /// - `restart`: Cancel the running tools and run them again.
    /// - `stop_reply`: Cancel the running tools and send a message back to the
    ///   assistant in their place.
    #[setting(default)]
    pub tool_call: ToolInterrupt,
}

/// What Ctrl-C does while the assistant is generating content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ConfigEnum)]
#[serde(rename_all = "snake_case")]
pub enum StreamingInterrupt {
    /// Show the interrupt menu and let the user choose.
    #[default]
    Prompt,

    /// Resume the response: if it is still streaming, keep waiting for it; if
    /// the stream has already stopped, re-request and continue from the part
    /// generated so far.
    Continue,

    /// Save the response generated so far and exit.
    Stop,

    /// Discard the in-progress response and exit without saving it.
    Abort,

    /// Stop the response and send a new message.
    /// What was generated so far is kept as context for the reply.
    Reply,
}

/// What Ctrl-C does while tools are executing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ConfigEnum)]
#[serde(rename_all = "snake_case")]
pub enum ToolInterrupt {
    /// Show the interrupt menu and let the user choose.
    #[default]
    Prompt,

    /// Keep waiting for the running tools to finish.
    Continue,

    /// Cancel the running tools and run the same batch again.
    Restart,

    /// Cancel the running tools and send a message back to the assistant in
    /// place of their results.
    /// An empty message uses a canned rejection notice.
    StopReply,
}

impl AssignKeyValue for PartialInterruptConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "streaming" => self.streaming = kv.try_some_from_str()?,
            "tool_call" => self.tool_call = kv.try_some_from_str()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialInterruptConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            streaming: delta_opt(self.streaming.as_ref(), next.streaming),
            tool_call: delta_opt(self.tool_call.as_ref(), next.tool_call),
        }
    }
}

impl FillDefaults for PartialInterruptConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            streaming: self.streaming.or(defaults.streaming),
            tool_call: self.tool_call.or(defaults.tool_call),
        }
    }
}

impl ToPartial for InterruptConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            streaming: partial_opt(&self.streaming, defaults.streaming),
            tool_call: partial_opt(&self.tool_call, defaults.tool_call),
        }
    }
}

#[cfg(test)]
#[path = "interrupt_tests.rs"]
mod tests;
