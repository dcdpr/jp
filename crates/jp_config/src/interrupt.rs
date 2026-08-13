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

use std::{fmt, str::FromStr};

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
    /// Seconds the Ctrl-C escalation counter survives without a new press.
    ///
    /// Defaults to `2`.
    /// Presses within the window escalate: the first opens an interrupt menu
    /// (or begins a graceful shutdown when nothing can show one), the second
    /// begins a graceful shutdown, and any press after shutdown has begun exits
    /// immediately.
    /// A press arriving after the window counts as a fresh first press.
    #[setting(default = 2)]
    pub escalation_cooldown_secs: u32,

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

    /// Where the `reply` is composed: inline widget or external editor.
    ///
    /// Accepts `true`/`false` or `"always"`/`"never"`:
    ///
    /// - `false` (default): start in the inline widget; `Ctrl+X` escapes to
    ///   `editor.cmd` on demand.
    /// - `true`: open `editor.cmd` directly; if it cannot open, fall back to
    ///   the inline widget.
    /// - `"always"`: open `editor.cmd` directly; if it cannot open, return to
    ///   the menu — never the inline widget.
    /// - `"never"`: inline widget only; the `Ctrl+X` editor escape is disabled.
    ///
    /// An empty or cancelled editor returns to the menu.
    /// Has no effect in non-interactive (no-tty) mode.
    #[setting(default)]
    pub compose_in_editor: ComposeInEditor,
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
    /// - `stop`: Cancel the running tools, record each tool's configured
    ///   `cancellation_response`, and end the turn without asking the assistant
    ///   to continue.
    #[setting(default)]
    pub action: ToolInterruptAction,

    /// Where the `respond` message is composed: inline widget or external
    /// editor.
    ///
    /// Accepts `true`/`false` or `"always"`/`"never"`:
    ///
    /// - `false` (default): start in the inline widget; `Ctrl+X` escapes to
    ///   `editor.cmd` on demand.
    /// - `true`: open `editor.cmd` directly; if it cannot open, fall back to
    ///   the inline widget.
    /// - `"always"`: open `editor.cmd` directly; if it cannot open, return to
    ///   the menu — never the inline widget.
    /// - `"never"`: inline widget only; the `Ctrl+X` editor escape is disabled.
    ///
    /// An empty or cancelled editor returns to the menu.
    /// Has no effect in non-interactive (no-tty) mode.
    #[setting(default)]
    pub compose_in_editor: ComposeInEditor,
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
    /// An empty message uses each tool's configured `cancellation_response`.
    Respond,

    /// Cancel the running tools, record each tool's configured
    /// `cancellation_response`, and end the turn without asking the assistant
    /// to continue.
    Stop,
}

/// Where a reply or response is composed.
///
/// Accepts `true`/`false` or `"always"`/`"never"`:
///
/// - `false` (default): start in the inline widget; `Ctrl+X` escapes to the
///   external editor on demand.
/// - `true`: start in the external editor; fall back to the inline widget if it
///   cannot open.
/// - `"always"`: start in the external editor; on failure return to the menu,
///   never the inline widget.
/// - `"never"`: inline widget only; the `Ctrl+X` editor escape is disabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ComposeInEditor {
    /// Inline widget only; the `Ctrl+X` editor escape is disabled.
    /// (`"never"`)
    Never,

    /// Start in the inline widget; `Ctrl+X` escapes to the editor.
    /// (`false`)
    #[default]
    Inline,

    /// Start in the editor; fall back to the inline widget on failure.
    /// (`true`)
    Editor,

    /// Start in the editor; on failure return to the menu, never the inline
    /// widget.
    /// (`"always"`)
    Always,
}

impl ComposeInEditor {
    /// Whether composing starts in the external editor (`true` / `"always"`).
    #[must_use]
    pub const fn starts_in_editor(self) -> bool {
        matches!(self, Self::Editor | Self::Always)
    }

    /// Whether the inline widget should wire the `Ctrl+X` editor escape.
    /// Disabled only for `"never"`.
    #[must_use]
    pub const fn editor_escape(self) -> bool {
        !matches!(self, Self::Never)
    }

    /// Whether a failed editor falls back to the inline widget (`true`) rather
    /// than returning to the menu (`"always"`).
    #[must_use]
    pub const fn falls_back_to_inline(self) -> bool {
        matches!(self, Self::Editor)
    }
}

impl From<bool> for ComposeInEditor {
    fn from(v: bool) -> Self {
        if v { Self::Editor } else { Self::Inline }
    }
}

impl FromStr for ComposeInEditor {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "true" => Ok(Self::Editor),
            "false" => Ok(Self::Inline),
            "always" => Ok(Self::Always),
            "never" => Ok(Self::Never),
            _ => Err(format!(
                "invalid compose_in_editor value: '{s}', expected one of: true, false, always, \
                 never"
            )),
        }
    }
}

impl fmt::Display for ComposeInEditor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Never => write!(f, "never"),
            Self::Inline => write!(f, "false"),
            Self::Editor => write!(f, "true"),
            Self::Always => write!(f, "always"),
        }
    }
}

impl Serialize for ComposeInEditor {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Inline => serializer.serialize_bool(false),
            Self::Editor => serializer.serialize_bool(true),
            Self::Never => serializer.serialize_str("never"),
            Self::Always => serializer.serialize_str("always"),
        }
    }
}

impl<'de> Deserialize<'de> for ComposeInEditor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ComposeVisitor;

        impl serde::de::Visitor<'_> for ComposeVisitor {
            type Value = ComposeInEditor;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a boolean or one of: \"always\", \"never\"")
            }

            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<ComposeInEditor, E> {
                Ok(ComposeInEditor::from(v))
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<ComposeInEditor, E> {
                v.parse().map_err(serde::de::Error::custom)
            }
        }

        deserializer.deserialize_any(ComposeVisitor)
    }
}

impl schematic::Schematic for ComposeInEditor {
    fn schema_name() -> Option<String> {
        Some("ComposeInEditor".to_owned())
    }

    fn build_schema(mut schema: schematic::SchemaBuilder) -> schematic::Schema {
        use schematic::schema::{BooleanType, EnumType, LiteralValue, UnionType};

        schema.union(UnionType::new_any([
            schema.nest().boolean(BooleanType::default()),
            schema.nest().enumerable(EnumType::new([
                LiteralValue::String("always".into()),
                LiteralValue::String("never".into()),
            ])),
        ]))
    }
}

impl AssignKeyValue for PartialInterruptConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "escalation_cooldown_secs" => {
                self.escalation_cooldown_secs = kv.try_some_u32()?;
            }
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
            "compose_in_editor" => self.compose_in_editor = kv.try_some_bool_or_from_str()?,
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
            "compose_in_editor" => self.compose_in_editor = kv.try_some_bool_or_from_str()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialInterruptConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            escalation_cooldown_secs: delta_opt(
                self.escalation_cooldown_secs.as_ref(),
                next.escalation_cooldown_secs,
            ),
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
            escalation_cooldown_secs: self
                .escalation_cooldown_secs
                .or(defaults.escalation_cooldown_secs),
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
        let defaults = Self::Partial::default();

        Self::Partial {
            escalation_cooldown_secs: partial_opt(
                &self.escalation_cooldown_secs,
                defaults.escalation_cooldown_secs,
            ),
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
