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
//! Minijinja template spans (`{{ … }}`, `{% … %}`, `{# … #}`) are treated as
//! atomic during the split, so an expression may contain spaces without being
//! torn across arguments:
//!
//! ```ignore
//! "just x {{ a | default('') }}" => ["just", "x", "{{ a | default('') }}"]
//! ```
//!
//! Spans are atomic for every string-form command, not only for the ones that
//! are rendered: tool commands are rendered by
//! `jp_llm::tool::run_tool_command`, while `editor.cmd` runs verbatim and keeps
//! a span as a literal argument.
//!
//! Malformed shell quoting (unbalanced quotes, dangling escapes) is rejected at
//! config-parse time via [`PartialCommandConfigOrString::from_str`] rather than
//! producing garbage at spawn time.
//! Quoting *inside* a span is minijinja text rather than shell syntax, so it is
//! not part of that check.

use std::{fmt, str::FromStr};

use schematic::Config;
use serde::{Deserialize, Serialize};

use crate::{
    BoxedError,
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt},
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
    ///
    /// Marked as the expanded form: the string above is a shorthand for this
    /// table, so `cmd.program` and `cmd.args` address a command however it was
    /// written.
    #[setting(nested, expanded)]
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

            // A key addressing the table's fields expands the shorthand first,
            // rather than discarding the program it named. `editor.cmd = "code
            // --wait"` followed by `editor.cmd.args = ["--foo"]` keeps `code`.
            _ => match self {
                Self::String(shorthand) => {
                    let mut config = expand_shorthand(shorthand);
                    config.assign(kv)?;
                    *self = Self::Config(config);
                }
                Self::Config(config) => config.assign(kv)?,
            },
        }

        Ok(())
    }
}

/// Expand the string shorthand into the table form it abbreviates.
///
/// Splits exactly as [`CommandConfigOrString::command`] does, so expanding
/// before assigning a field cannot change which command ends up running.
/// That includes keeping a minijinja template span in one argument even when it
/// contains spaces.
/// `shell` and any empty part are left unset so their defaults still apply, and
/// so a later layer can still fill them.
fn expand_shorthand(shorthand: &str) -> PartialCommandConfig {
    let mut tokens = split_command_words(shorthand)
        .unwrap_or_default()
        .into_iter();
    let program = tokens.next();
    let args: Vec<_> = tokens.collect();

    PartialCommandConfig {
        program,
        args: (!args.is_empty()).then_some(args),
        shell: None,
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
        // NUL is reserved as the placeholder delimiter used while masking
        // template spans, and no OS accepts it in an argument anyway.
        if s.contains('\0') {
            return Err(format!("command string contains a NUL byte: {s:?}").into());
        }

        // Validate shell quoting at parse time so malformed input fails fast
        // rather than producing garbage at spawn time. Empty / whitespace-only
        // strings are accepted (they parse to an empty token list and produce
        // an empty `program`, which is the same behavior the old
        // `split_whitespace` parser had — execution-time error, not a
        // config-parse error).
        if split_command_words(s).is_none() {
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
    /// Minijinja template spans (`{{ … }}`, `{% … %}`, `{# … #}`) stay in a
    /// single argument even when they contain spaces.
    ///
    /// Malformed input is normally rejected at config-parse time by
    /// [`PartialCommandConfigOrString::from_str`].
    /// This method is defensive against input that skips that check (direct
    /// construction in Rust code, or a config format that can encode a NUL
    /// byte): a split failure falls back to an empty token list, which surfaces
    /// as a spawn-time error.
    #[must_use]
    pub fn command(self) -> CommandConfig {
        match self {
            Self::String(v) => {
                let mut iter = split_command_words(&v).unwrap_or_default().into_iter();

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
/// only fails on an interior NUL byte; a raw space-join is the fallback).
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
            // `args` replaces on merge rather than appending, so a change to
            // any argument has to record the whole list.
            args: delta_opt(self.args.as_ref(), next.args),
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

/// Split a command string into shell words, treating minijinja template spans
/// as atomic.
///
/// The shell split happens before template rendering, so a rendered value can
/// never inject extra arguments.
/// A template span may itself contain spaces (e.g. `{{ x | default('') }}`);
/// masking the spans before the split keeps each one inside a single argument.
/// Returns `None` on unbalanced shell quoting (mirroring [`shlex::split`]) and
/// on an interior NUL byte, which the masking step reserves for its
/// placeholders.
fn split_command_words(input: &str) -> Option<Vec<String>> {
    if input.contains('\0') {
        return None;
    }

    let (masked, spans) = mask_template_spans(input);
    let words = shlex::split(&masked)?;
    Some(
        words
            .into_iter()
            .map(|word| restore_spans(&word, &spans))
            .collect(),
    )
}

/// Replace every minijinja template span (`{{ … }}`, `{% … %}`, `{# … #}`)
/// with a NUL-delimited placeholder, returning the masked string and the
/// original span texts in order.
///
/// Callers reject input containing NUL (see [`split_command_words`]), so a
/// placeholder cannot collide with real command text.
/// The placeholder holds no whitespace or quote characters, so `shlex` keeps it
/// inside a single word.
fn mask_template_spans(input: &str) -> (String, Vec<String>) {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut spans: Vec<String> = Vec::new();
    let mut i = 0;

    while i < bytes.len() {
        let Some(open) = find_span_open(bytes, i) else {
            out.push_str(&input[i..]);
            break;
        };

        out.push_str(&input[i..open]);
        let end = find_span_end(bytes, open);
        out.push('\0');
        out.push_str(&spans.len().to_string());
        out.push('\0');
        spans.push(input[open..end].to_owned());
        i = end;
    }

    (out, spans)
}

/// Find the next `{{` / `{%` / `{#` opener at or after `from`.
fn find_span_open(bytes: &[u8], from: usize) -> Option<usize> {
    (from..bytes.len().saturating_sub(1))
        .find(|&j| bytes[j] == b'{' && matches!(bytes[j + 1], b'{' | b'%' | b'#'))
}

/// Find the byte index just past the closer of the span opened at `open`.
///
/// Comment spans (`{# … #}`) are raw text.
/// Expression (`{{ … }}`) and statement (`{% … %}`) spans skip string
/// literals, so a closer inside a quoted string does not end the span early,
/// and track `()`, `[]` and `{}` nesting, mirroring minijinja, which only
/// accepts an end delimiter at nesting depth zero.
/// In `{{ {"a": 1} }}` the first `}` closes the map, not the span.
/// An unterminated span extends to end of input; the render step then reports
/// the real error.
fn find_span_end(bytes: &[u8], open: usize) -> usize {
    match bytes[open + 1] {
        b'#' => find_literal_close(bytes, open + 2, b'#'),
        kind => {
            let closer = if kind == b'{' { b'}' } else { b'%' };
            let mut j = open + 2;
            let mut quote: Option<u8> = None;
            let mut depth: usize = 0;

            while j < bytes.len() {
                let b = bytes[j];
                if let Some(q) = quote {
                    if b == b'\\' {
                        j += 2;
                        continue;
                    }
                    if b == q {
                        quote = None;
                    }
                    j += 1;
                } else if b == b'\'' || b == b'"' {
                    quote = Some(b);
                    j += 1;
                } else if depth == 0 && b == closer && bytes.get(j + 1) == Some(&b'}') {
                    return j + 2;
                } else {
                    match b {
                        b'(' | b'[' | b'{' => depth += 1,
                        b')' | b']' | b'}' => depth = depth.saturating_sub(1),
                        _ => {}
                    }
                    j += 1;
                }
            }

            bytes.len()
        }
    }
}

/// Find the byte index just past a literal `first`+`}` closer at or after `j`.
fn find_literal_close(bytes: &[u8], mut j: usize, first: u8) -> usize {
    while j < bytes.len() {
        if bytes[j] == first && bytes.get(j + 1) == Some(&b'}') {
            return j + 2;
        }
        j += 1;
    }
    bytes.len()
}

/// Substitute masked placeholders in `token` back to their original span text.
fn restore_spans(token: &str, spans: &[String]) -> String {
    if !token.contains('\0') {
        return token.to_owned();
    }

    let mut out = String::with_capacity(token.len());
    let mut rest = token;

    while let Some(start) = rest.find('\0') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];

        let Some(end) = after.find('\0') else {
            // Unbalanced marker (not one this masker produced): emit verbatim.
            out.push('\0');
            rest = after;
            continue;
        };

        match after[..end].parse::<usize>() {
            Ok(idx) if idx < spans.len() => out.push_str(&spans[idx]),
            _ => {
                out.push('\0');
                out.push_str(&after[..=end]);
            }
        }
        rest = &after[end + 1..];
    }

    out.push_str(rest);
    out
}

#[cfg(test)]
#[path = "command_tests.rs"]
mod tests;
