//! Interrupt (Ctrl-C) behavior configuration.
//!
//! Controls what pressing Ctrl-C does during a query.
//! By default JP shows an interactive menu and lets you choose; each context
//! can instead be set to a fixed action that runs immediately, without the
//! menu.
//!
//! ```toml
//! [interrupt.streaming]
//! action = "prompt"   # while the assistant is generating content
//!
//! [interrupt.tool_call]
//! action = "prompt"   # while tools are executing
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
    /// Ctrl-C behavior while the assistant is generating content.
    #[setting(nested)]
    pub streaming: StreamingInterruptConfig,

    /// Ctrl-C behavior while tools are executing.
    #[setting(nested)]
    pub tool_call: ToolInterruptConfig,
}

/// Ctrl-C behavior while the assistant is generating content.
#[derive(Debug, Clone, PartialEq, Default, Config)]
#[config(rename_all = "snake_case")]
pub struct StreamingInterruptConfig {
    /// What Ctrl-C does.
    ///
    /// - `prompt`: Show the interrupt menu and choose (default).
    /// - `continue`: Resume the response (keep waiting for it, or continue from
    ///   the part already generated if the stream has stopped).
    /// - `stop`: Save the response generated so far and exit.
    /// - `abort`: Discard the in-progress response and exit.
    /// - `reply`: Stop the response and send a new message; what was generated
    ///   so far is kept as context.
    #[setting(default)]
    pub action: StreamingInterruptAction,

    /// Compose the reply in the external editor instead of the inline widget.
    ///
    /// Defaults to `false`.
    /// When `true`, the `reply` action opens `editor.cmd` directly instead of
    /// the inline reply prompt; `false` starts in the inline widget, where
    /// `Ctrl+X` escapes to the editor on demand.
    /// Falls back to the inline widget when no editor is configured, and has no
    /// effect in non-interactive (no-tty) mode.
    #[setting(default = false)]
    pub compose_in_editor: bool,
}

/// Ctrl-C behavior while tools are executing.
#[derive(Debug, Clone, PartialEq, Default, Config)]
#[config(rename_all = "snake_case")]
pub struct ToolInterruptConfig {
    /// What Ctrl-C does.
    ///
    /// - `prompt`: Show the interrupt menu and choose (default).
    /// - `continue`: Keep waiting for the running tools to finish.
    /// - `restart`: Cancel the running tools and run them again.
    /// - `respond`: Cancel the running tools and send a message back to the
    ///   assistant in their place.
    #[setting(default)]
    pub action: ToolInterruptAction,

    /// Compose the response in the external editor instead of the inline
    /// widget.
    ///
    /// Defaults to `false`.
    /// When `true`, the `respond` action opens `editor.cmd` directly instead of
    /// the inline reply prompt; `false` starts in the inline widget, where
    /// `Ctrl+X` escapes to the editor on demand.
    /// Falls back to the inline widget when no editor is configured, and has no
    /// effect in non-interactive (no-tty) mode.
    #[setting(default = false)]
    pub compose_in_editor: bool,
}

/// What Ctrl-C does while the assistant is generating content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ConfigEnum)]
#[serde(rename_all = "snake_case")]
pub enum StreamingInterruptAction {
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
pub enum ToolInterruptAction {
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
    Respond,
}

impl AssignKeyValue for PartialInterruptConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            _ if kv.p("streaming") => self.streaming.assign(kv)?,
            _ if kv.p("tool_call") => self.tool_call.assign(kv)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl AssignKeyValue for PartialStreamingInterruptConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "action" => self.action = kv.try_some_from_str()?,
            "compose_in_editor" => self.compose_in_editor = kv.try_some_bool()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl AssignKeyValue for PartialToolInterruptConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "action" => self.action = kv.try_some_from_str()?,
            "compose_in_editor" => self.compose_in_editor = kv.try_some_bool()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialInterruptConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            streaming: self.streaming.delta(next.streaming),
            tool_call: self.tool_call.delta(next.tool_call),
        }
    }
}

impl PartialConfigDelta for PartialStreamingInterruptConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            action: delta_opt(self.action.as_ref(), next.action),
            compose_in_editor: delta_opt(self.compose_in_editor.as_ref(), next.compose_in_editor),
        }
    }
}

impl PartialConfigDelta for PartialToolInterruptConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            action: delta_opt(self.action.as_ref(), next.action),
            compose_in_editor: delta_opt(self.compose_in_editor.as_ref(), next.compose_in_editor),
        }
    }
}

impl FillDefaults for PartialInterruptConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            streaming: self.streaming.fill_from(defaults.streaming),
            tool_call: self.tool_call.fill_from(defaults.tool_call),
        }
    }
}

impl FillDefaults for PartialStreamingInterruptConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            action: self.action.or(defaults.action),
            compose_in_editor: self.compose_in_editor.or(defaults.compose_in_editor),
        }
    }
}

impl FillDefaults for PartialToolInterruptConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            action: self.action.or(defaults.action),
            compose_in_editor: self.compose_in_editor.or(defaults.compose_in_editor),
        }
    }
}

impl ToPartial for InterruptConfig {
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            streaming: self.streaming.to_partial(),
            tool_call: self.tool_call.to_partial(),
        }
    }
}

impl ToPartial for StreamingInterruptConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            action: partial_opt(&self.action, defaults.action),
            compose_in_editor: partial_opt(&self.compose_in_editor, defaults.compose_in_editor),
        }
    }
}

impl ToPartial for ToolInterruptConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            action: partial_opt(&self.action, defaults.action),
            compose_in_editor: partial_opt(&self.compose_in_editor, defaults.compose_in_editor),
        }
    }
}

#[cfg(test)]
#[path = "interrupt_tests.rs"]
mod tests;
