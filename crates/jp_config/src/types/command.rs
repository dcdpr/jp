//! Configuration shape for user-configured external commands.
//!
//! [`CommandConfig`] models an external command JP runs on behalf of the user:
//! a program plus arguments, optionally wrapped in a shell.
//! [`CommandConfigOrString`] adds a string-shorthand variant so users can write
//! `command = "cargo check"` and have it parsed as `{ program = "cargo", args =
//! ["check"] }` automatically.
//!
//! String-shorthand parsing uses shell-word semantics via [`shlex`], so quoting
//! is respected:
//!
//! ```ignore
//! "echo 'hello world'" => ["echo", "hello world"]
//! ```
//!
//! Malformed shell quoting (unbalanced quotes, dangling escapes) is rejected at
//! config-parse time via [`PartialCommandConfigOrString::from_str`] rather than
//! producing garbage at spawn time.

use std::{fmt, str::FromStr};

use schematic::Config;
use serde::{Deserialize, Serialize};

use crate::{
    BoxedError,
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt, delta_opt_vec},
    partial::{ToPartial, partial_opt},
};

/// Command configuration, either as a string or a complete configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Config)]
#[config(rename_all = "snake_case", serde(untagged))]
#[serde(untagged)]
pub enum CommandConfigOrString {
    /// A single string, parsed as shell words: first token is the program,
    /// remaining tokens are arguments.
    /// Quoting is respected.
    String(String),

    /// A complete command configuration.
    #[setting(nested)]
    Config(CommandConfig),
}

impl fmt::Display for CommandConfigOrString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String(v) => write!(f, "{v}"),
            Self::Config(v) => write!(f, "{v}"),
        }
    }
}

impl AssignKeyValue for PartialCommandConfigOrString {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object_or_from_str()?,
            _ => match self {
                Self::String(_) => return missing_key(&kv),
                Self::Config(config) => config.assign(kv)?,
            },
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialCommandConfigOrString {
    fn delta(&self, next: Self) -> Self {
        match (self, next) {
            (Self::Config(prev), Self::Config(next)) => Self::Config(prev.delta(next)),
            (_, next) => next,
        }
    }
}

impl ToPartial for CommandConfigOrString {
    fn to_partial(&self) -> Self::Partial {
        match self {
            Self::String(v) => Self::Partial::String(v.to_owned()),
            Self::Config(v) => Self::Partial::Config(v.to_partial()),
        }
    }
}

impl FromStr for PartialCommandConfigOrString {
    type Err = BoxedError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Validate shell quoting at parse time so malformed input fails fast
        // rather than producing garbage at spawn time. Empty / whitespace-only
        // strings are accepted (they parse to an empty token list and produce
        // an empty `program`, which is the same behavior the old
        // `split_whitespace` parser had — execution-time error, not a
        // config-parse error).
        if shlex::split(s).is_none() {
            return Err(format!("invalid shell quoting in command string: {s:?}").into());
        }

        Ok(Self::String(s.to_owned()))
    }
}

impl CommandConfigOrString {
    /// Return the command configuration.
    ///
    /// If the configuration is a string, it is parsed using shell-word
    /// semantics (`shlex::split`): the first token becomes
    /// [`CommandConfig::program`], remaining tokens become
    /// [`CommandConfig::args`].
    /// [`CommandConfig::shell`] is `false`.
    ///
    /// Malformed input is normally rejected at config-parse time by
    /// [`PartialCommandConfigOrString::from_str`].
    /// This method is defensive against direct construction in Rust code: on
    /// `shlex::split` failure it falls back to an empty token list, which
    /// surfaces as a spawn-time error.
    #[must_use]
    pub fn command(self) -> CommandConfig {
        match self {
            Self::String(v) => {
                let mut iter = shlex::split(&v).unwrap_or_default().into_iter();

                CommandConfig {
                    program: iter.next().unwrap_or_default(),
                    args: iter.collect(),
                    shell: false,
                }
            }
            Self::Config(v) => v,
        }
    }
}

/// Build a shell command line from a raw `program` and its discrete `args`.
///
/// `program` is used verbatim — it may itself be shell syntax (`&&`, `|`,
/// redirects).
/// The `args` are shell-quoted with [`shlex::try_join`] so multi-word arguments
/// keep their boundaries instead of being word-split by the shell (`try_join`
/// only fails on an interior NUL byte, which a config value can't carry; a raw
/// space-join is the fallback).
///
/// The caller wraps the result in its own shell invocation (`sh -c <line>`).
#[must_use]
pub fn shell_command_line(program: &str, args: &[String]) -> String {
    if args.is_empty() {
        return program.to_owned();
    }

    shlex::try_join(args.iter().map(String::as_str)).map_or_else(
        |_| {
            std::iter::once(program)
                .chain(args.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" ")
        },
        |quoted| format!("{program} {quoted}"),
    )
}

/// External command configuration.
///
/// A user-facing description of a command JP should run: which program, with
/// which arguments, and whether to wrap the whole thing in a shell.
/// The configured policy around *when* JP is allowed to run the command (prompt
/// or not, confirm `shell = true` invocations, etc.) lives on each consumer
/// (tools, labels, ...), not on this shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Config)]
#[config(rename_all = "snake_case")]
pub struct CommandConfig {
    /// The program to run.
    pub program: String,

    /// The arguments to pass to the program.
    #[setting(default = vec![])]
    pub args: Vec<String>,

    /// Whether to run the command in a shell.
    ///
    /// When enabled, the command runs via `/bin/sh -c`, so pipes, `&&`, and
    /// subshells work.
    /// When disabled (the default), the program is executed directly with its
    /// arguments.
    ///
    /// Consumers may attach their own policy to shell commands — for example,
    /// tools always prompt for confirmation before running a `shell = true`
    /// command, for security reasons.
    #[setting(default)]
    pub shell: bool,
}

impl fmt::Display for CommandConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.shell {
            writeln!(f, "/bin/sh -c'")?;
        }

        write!(f, "{}", self.program)?;
        for arg in &self.args {
            write!(f, " {arg}")?;
        }

        if self.shell {
            write!(f, "'")?;
        }

        Ok(())
    }
}

impl AssignKeyValue for PartialCommandConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "program" => self.program = kv.try_some_string()?,
            _ if kv.p("args") => kv.try_some_vec_of_strings(&mut self.args)?,
            "shell" => self.shell = kv.try_some_bool()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialCommandConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            program: delta_opt(self.program.as_ref(), next.program),
            args: delta_opt_vec(self.args.as_ref(), next.args),
            shell: delta_opt(self.shell.as_ref(), next.shell),
        }
    }
}

impl ToPartial for CommandConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            program: partial_opt(&self.program, defaults.program),
            args: partial_opt(&self.args, defaults.args),
            shell: partial_opt(&self.shell, defaults.shell),
        }
    }
}

#[cfg(test)]
#[path = "command_tests.rs"]
mod tests;
