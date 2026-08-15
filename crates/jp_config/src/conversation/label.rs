//! Label rules for conversations.
//!
//! Labels are `key=value` annotations stored on a conversation.
//! This section declares the *rules* that produce them; the map key is the
//! label key.
//!
//! ```toml
//! [conversation.labels]
//! team = "platform"
//!
//! [conversation.labels.stage]
//! value = "review"
//! apply_on = { new = true, fork = true }
//! ```
//!
//! A command-shaped value runs at the workspace root and its trimmed stdout
//! becomes the label value:
//!
//! ```toml
//! [conversation.labels.branch]
//! value.cmd = "git rev-parse --abbrev-ref HEAD"
//! apply_on = { new = true, fork = true }
//! ```
//!
//! A label key starts with an ASCII letter, followed by any number of letters,
//! digits, underscores, and hyphens.

use schematic::{Config, ConfigEnum};
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt},
    fill::FillDefaults,
    partial::{ToPartial, partial_opt},
    types::command::{CommandConfigOrString, PartialCommandConfigOrString},
};

/// The label key grammar, in words.
///
/// Every excluded character is significant somewhere the key is used: `.`
/// separates dotted config paths, `=` and `,` are CLI separators, and `:` marks
/// an alias reference.
/// The leading character is narrower still, because a key that starts with `-`
/// would be read as a flag where keys are written as bare command arguments.
const KEY_GRAMMAR: &str = "a label key starts with an ASCII letter, followed by any number of \
                           letters, digits, underscores, and hyphens";

/// Validate a label key against the `[A-Za-z][A-Za-z0-9_-]*` grammar.
///
/// # Errors
///
/// Returns a human-readable message if the key is empty, starts with something
/// other than an ASCII letter, or contains a character outside the grammar.
pub fn validate_key(key: &str) -> Result<(), String> {
    let mut chars = key.chars();

    let Some(first) = chars.next() else {
        return Err("label key must not be empty".to_owned());
    };

    if !first.is_ascii_alphabetic() {
        return Err(format!(
            "label key '{key}' starts with '{first}': {KEY_GRAMMAR}"
        ));
    }

    if let Some(invalid) = chars.find(|c| !c.is_ascii_alphanumeric() && *c != '_' && *c != '-') {
        return Err(format!(
            "invalid character '{invalid}' in label key '{key}': {KEY_GRAMMAR}"
        ));
    }

    Ok(())
}

/// A label declaration.
///
/// Either a bare value (`team = "platform"`) or a table with the full set of
/// options (`team = { value = "platform", apply_on = { fork = true } }`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Config)]
#[config(rename_all = "snake_case", serde(untagged))]
#[serde(untagged)]
pub enum LabelConfig {
    /// Shorthand for a static label value, using the defaults for every other
    /// option.
    Static(String),

    /// The full form, with `value`, `apply_on`, and `run`.
    #[setting(nested)]
    Object(LabelObject),
}

impl LabelConfig {
    /// The value rule for this label.
    #[must_use]
    pub fn value(&self) -> LabelValueRef<'_> {
        match self {
            Self::Static(value) => LabelValueRef::Static(value),
            Self::Object(object) => match &object.value {
                LabelValue::Static(value) => LabelValueRef::Static(value),
                LabelValue::Command(command) => LabelValueRef::Command(&command.cmd),
            },
        }
    }

    /// When this label is automatically applied.
    #[must_use]
    pub fn apply_on(&self) -> ApplyOn {
        match self {
            Self::Static(_) => ApplyOn::default(),
            Self::Object(object) => object.apply_on,
        }
    }

    /// The confirmation policy for a command-shaped value.
    #[must_use]
    pub fn run(&self) -> LabelRunMode {
        match self {
            Self::Static(_) => LabelRunMode::default(),
            Self::Object(object) => object.run,
        }
    }
}

/// A borrowed view of a label's value rule, flattening the shorthand and full
/// forms into one shape.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LabelValueRef<'a> {
    /// A literal value, used as-is.
    Static(&'a str),

    /// A command whose trimmed stdout becomes the value.
    Command(&'a CommandConfigOrString),
}

/// A label declaration in its full form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Config)]
#[config(rename_all = "snake_case")]
pub struct LabelObject {
    /// The label's value.
    ///
    /// A string is used literally.
    /// Defaults to an empty value, which renders as a bare label.
    #[setting(nested)]
    #[serde(default)]
    pub value: LabelValue,

    /// When the label is automatically applied.
    ///
    /// Independent of naming the label on the CLI, which always applies it.
    #[setting(nested)]
    #[serde(default)]
    pub apply_on: ApplyOn,

    /// Whether to confirm before running a command-shaped `value`.
    ///
    /// - `ask`: Prompt before each run (the default).
    /// - `unattended`: Run without prompting.
    /// - `deny`: Never run; the label is skipped.
    ///
    /// Ignored for literal values.
    #[setting(default)]
    #[serde(default)]
    pub run: LabelRunMode,
}

/// How a label's value is produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Config)]
#[config(rename_all = "snake_case", serde(untagged))]
#[serde(untagged)]
pub enum LabelValue {
    /// A literal value: `value = "review"`.
    Static(String),

    /// A command whose trimmed stdout becomes the value.
    #[setting(nested)]
    Command(LabelCommand),
}

impl Default for LabelValue {
    fn default() -> Self {
        Self::Static(String::new())
    }
}

/// A command-backed label value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Config)]
#[config(rename_all = "snake_case")]
pub struct LabelCommand {
    /// The command to run.
    ///
    /// Either a string that is split into program and arguments (`cmd = "git
    /// rev-parse --abbrev-ref HEAD"`) or a table with `program`, `args`, and
    /// `shell`.
    #[setting(nested)]
    pub cmd: CommandConfigOrString,
}

/// When a label is automatically applied.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Config)]
#[config(rename_all = "snake_case")]
pub struct ApplyOn {
    /// Apply the label when a new conversation is created (`jp query --new`).
    ///
    /// Defaults to `true`.
    #[setting(default = true)]
    #[serde(default = "default_true")]
    pub new: bool,

    /// Re-apply the label when a conversation is forked (`jp conversation
    /// fork`).
    ///
    /// Defaults to `false`, where the source conversation's value is inherited
    /// verbatim.
    #[setting(default)]
    #[serde(default)]
    pub fork: bool,
}

/// Serde default for [`ApplyOn::new`], which is on unless the user turns it
/// off.
const fn default_true() -> bool {
    true
}

impl Default for ApplyOn {
    fn default() -> Self {
        Self {
            new: true,
            fork: false,
        }
    }
}

/// Whether to confirm before running a command-shaped label value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ConfigEnum)]
#[serde(rename_all = "lowercase")]
pub enum LabelRunMode {
    /// Prompt before each run.
    #[default]
    Ask,

    /// Run without prompting.
    Unattended,

    /// Never run; the label is skipped.
    Deny,
}

impl AssignKeyValue for PartialLabelConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object_or_from_str()?,
            _ => match self {
                Self::Static(_) => return missing_key(&kv),
                Self::Object(object) => object.assign(kv)?,
            },
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialLabelConfig {
    fn delta(&self, next: Self) -> Self {
        match (self, next) {
            (Self::Object(prev), Self::Object(next)) => Self::Object(prev.delta(next)),
            (_, next) => next,
        }
    }
}

impl ToPartial for LabelConfig {
    fn to_partial(&self) -> Self::Partial {
        match self {
            Self::Static(value) => Self::Partial::Static(value.clone()),
            Self::Object(object) => Self::Partial::Object(object.to_partial()),
        }
    }
}

impl std::str::FromStr for PartialLabelConfig {
    type Err = crate::BoxedError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::Static(s.to_owned()))
    }
}

impl AssignKeyValue for PartialLabelObject {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            _ if kv.p("value") => self.value.assign(kv)?,
            _ if kv.p("apply_on") => self.apply_on.assign(kv)?,
            "run" => self.run = kv.try_some_from_str()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialLabelObject {
    fn delta(&self, next: Self) -> Self {
        Self {
            value: self.value.delta(next.value),
            apply_on: self.apply_on.delta(next.apply_on),
            run: delta_opt(self.run.as_ref(), next.run),
        }
    }
}

impl FillDefaults for PartialLabelObject {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            value: self.value,
            apply_on: self.apply_on.fill_from(defaults.apply_on),
            run: self.run.or(defaults.run),
        }
    }
}

impl ToPartial for LabelObject {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            value: self.value.to_partial(),
            apply_on: self.apply_on.to_partial(),
            run: partial_opt(&self.run, defaults.run),
        }
    }
}

impl AssignKeyValue for PartialLabelValue {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object_or_from_str()?,
            _ => match self {
                Self::Static(_) => return missing_key(&kv),
                Self::Command(command) => command.assign(kv)?,
            },
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialLabelValue {
    fn delta(&self, next: Self) -> Self {
        match (self, next) {
            (Self::Command(prev), Self::Command(next)) => Self::Command(prev.delta(next)),
            (_, next) => next,
        }
    }
}

impl ToPartial for LabelValue {
    fn to_partial(&self) -> Self::Partial {
        match self {
            Self::Static(value) => Self::Partial::Static(value.clone()),
            Self::Command(command) => Self::Partial::Command(command.to_partial()),
        }
    }
}

impl std::str::FromStr for PartialLabelValue {
    type Err = crate::BoxedError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::Static(s.to_owned()))
    }
}

impl AssignKeyValue for PartialLabelCommand {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            _ if kv.p("cmd") => self.cmd.assign(kv)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialLabelCommand {
    fn delta(&self, next: Self) -> Self {
        Self {
            cmd: self.cmd.delta(next.cmd),
        }
    }
}

impl ToPartial for LabelCommand {
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            cmd: self.cmd.to_partial(),
        }
    }
}

impl AssignKeyValue for PartialApplyOn {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "new" => self.new = kv.try_some_bool()?,
            "fork" => self.fork = kv.try_some_bool()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialApplyOn {
    fn delta(&self, next: Self) -> Self {
        Self {
            new: delta_opt(self.new.as_ref(), next.new),
            fork: delta_opt(self.fork.as_ref(), next.fork),
        }
    }
}

impl FillDefaults for PartialApplyOn {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            new: self.new.or(defaults.new),
            fork: self.fork.or(defaults.fork),
        }
    }
}

impl ToPartial for ApplyOn {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            new: partial_opt(&self.new, defaults.new),
            fork: partial_opt(&self.fork, defaults.fork),
        }
    }
}

#[cfg(test)]
#[path = "label_tests.rs"]
mod tests;
