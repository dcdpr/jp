//! Editor configuration for Jean-Pierre.

use std::env;

use duct::Expression;
use schematic::{Config, ConfigEnum};
use serde::{Deserialize, Serialize};

#[cfg(unix)]
use crate::types::command::shell_command_line;
use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt, delta_opt_partial, delta_opt_vec},
    fill::FillDefaults,
    partial::{ToPartial, partial_opt, partial_opt_config},
    types::command::{CommandConfigOrString, PartialCommandConfigOrString},
};

/// Editor configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct EditorConfig {
    /// The command to open the editor.
    ///
    /// Either a string (`cmd = "code --wait"`) or a table (`cmd = { program =
    /// "code", args = ["--wait"] }`).
    /// The file(s) being edited are appended as arguments, so both forms open
    /// `code --wait <file>`.
    ///
    /// A string is split with shell-word semantics and run directly, without a
    /// shell, so a missing editor is reported as an error rather than silently
    /// doing nothing — this is the cross-platform form and the right choice on
    /// Windows.
    /// For pipes, `&&`, or subshells, set `shell = true`.
    /// On Unix the edited path(s) are forwarded to the shell command via
    /// `"$@"`; on Windows prefer `shell = false` and wrap any shell logic in a
    /// script.
    ///
    /// If unset, falls back to `envs`.
    #[setting(nested)]
    pub cmd: Option<CommandConfigOrString>,

    /// The environment variables to use for editing text.
    /// Used if `cmd` is unset.
    ///
    /// Defaults to `JP_EDITOR`, `VISUAL`, and `EDITOR`.
    ///
    /// Values are parsed using shell-word semantics (via `shlex`): the first
    /// token is the program, remaining tokens are arguments.
    /// Shell metacharacters like `|`, `&&`, or `>` are not interpreted — set
    /// `cmd` instead for full shell-mode parsing.
    /// Examples:
    ///
    /// - `JP_EDITOR="subl -w"` runs `subl` with `-w`.
    /// - `JP_EDITOR='code --wait --new-window'` runs `code` with two args.
    /// - `JP_EDITOR='nvim -c "set ft=md"'` runs `nvim` with args `-c` and `set
    ///   ft=md`.
    ///
    /// Values with unbalanced quoting are skipped (the next env var in the list
    /// is tried).
    #[setting(
        default = vec!["JP_EDITOR".into(), "VISUAL".into(), "EDITOR".into()],
        merge = schematic::merge::append_vec,
    )]
    pub envs: Vec<String>,

    /// Settings for the inline reply widget.
    ///
    /// Lives at `editor.inline.*`.
    #[setting(nested)]
    pub inline: InlineEditorConfig,
}

/// Inline reply widget configuration.
///
/// Settings for the inline editor JP shows for short replies (for example after
/// `Ctrl+C` during a query).
/// Independent of which external editor `cmd`/`envs` opens.
#[derive(Debug, Clone, PartialEq, Default, Config)]
#[config(rename_all = "snake_case")]
pub struct InlineEditorConfig {
    /// Editing style of the inline reply buffer.
    ///
    /// - `emacs`: Emacs-style keybindings (default).
    /// - `vi`: Vi-style modal editing (insert/normal modes).
    ///
    /// Controls only the inline buffer's editing style; it is independent of
    /// which external editor opens when you escape to `$EDITOR`.
    #[setting(default)]
    pub edit_mode: InlineEditMode,
}

/// Editing style for the inline reply widget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ConfigEnum)]
#[serde(rename_all = "snake_case")]
pub enum InlineEditMode {
    /// Emacs-style keybindings.
    #[default]
    Emacs,

    /// Vi-style modal editing (insert/normal modes).
    Vi,
}

impl AssignKeyValue for PartialEditorConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            _ if kv.p("cmd") => self.cmd.assign(kv)?,
            _ if kv.p("envs") => kv.try_some_vec_of_strings(&mut self.envs)?,
            _ if kv.p("inline") => self.inline.assign(kv)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialEditorConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            cmd: delta_opt_partial(self.cmd.as_ref(), next.cmd),
            envs: delta_opt_vec(self.envs.as_ref(), next.envs),
            inline: self.inline.delta(next.inline),
        }
    }
}

impl FillDefaults for PartialEditorConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            cmd: self.cmd.or(defaults.cmd),
            envs: self.envs.or(defaults.envs),
            inline: self.inline.fill_from(defaults.inline),
        }
    }
}

impl ToPartial for EditorConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            cmd: partial_opt_config(self.cmd.as_ref(), defaults.cmd),
            envs: partial_opt(&self.envs, defaults.envs),
            inline: self.inline.to_partial(),
        }
    }
}

impl AssignKeyValue for PartialInlineEditorConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "edit_mode" => self.edit_mode = kv.try_some_from_str()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialInlineEditorConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            edit_mode: delta_opt(self.edit_mode.as_ref(), next.edit_mode),
        }
    }
}

impl FillDefaults for PartialInlineEditorConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            edit_mode: self.edit_mode.or(defaults.edit_mode),
        }
    }
}

impl ToPartial for InlineEditorConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            edit_mode: partial_opt(&self.edit_mode, defaults.edit_mode),
        }
    }
}

impl EditorConfig {
    /// Build the editor command, with the edited path(s) appended by the
    /// caller.
    ///
    /// Returns `None` when neither `cmd` is set nor any configured environment
    /// variable resolves to an installed program.
    ///
    /// The returned expression expects the path(s) being edited to be appended
    /// as trailing arguments (e.g. via `duct`'s `before_spawn`):
    ///
    /// - `cmd` with `shell = false` (the default, including the string form)
    ///   spawns the program directly with its arguments, then the appended
    ///   path(s); a missing program surfaces as a spawn error rather than a
    ///   shell exit code.
    /// - `cmd` with `shell = true` runs through the system shell (`/bin/sh` on
    ///   Unix) so `&&`, `|`, and quoting work.
    ///   On Unix the appended path(s) are forwarded via `"$@"` (a command
    ///   already referencing `$@`/`$*` controls placement itself); other
    ///   platforms do not forward the path, so prefer `shell = false` there.
    /// - Env-var values are split with [`shlex::split`] so `JP_EDITOR="code
    ///   -w"` runs `code` with `-w`, then the path(s) as further arguments.
    ///   Values with unbalanced quoting are skipped.
    #[must_use]
    pub fn command(&self) -> Option<Expression> {
        self.cmd
            .clone()
            .map(CommandConfigOrString::command)
            .filter(|c| !c.program.trim().is_empty())
            .map(|c| editor_expression(&c.program, &c.args, c.shell))
            .or_else(|| {
                self.envs.iter().find_map(|v| {
                    let value = env::var(v).ok()?;
                    let mut parts = shlex::split(&value)?.into_iter();
                    let program = parts.next()?;
                    if which::which(&program).is_err() {
                        return None;
                    }
                    let args: Vec<String> = parts.collect();
                    Some(duct::cmd(program, args))
                })
            })
    }
}

/// Build a duct expression for an editor command.
///
/// The caller appends the edited path(s) as trailing arguments.
/// `shell = false` spawns `program` directly with `args` (cross-platform), so a
/// missing program is a spawn error; `shell = true` wraps the command in
/// `/bin/sh -c`, forwarding the appended path(s) via `"$@"`.
#[cfg(unix)]
fn editor_expression(program: &str, args: &[String], shell: bool) -> Expression {
    if !shell {
        return duct::cmd(program, args.to_vec());
    }

    // `program` is shell syntax and used verbatim; `args` are shell-quoted.
    // `sh -c <script>` assigns the first trailing operand to `$0`, so set an
    // explicit `$0` (`jp-editor`) and forward the appended path(s) via `"$@"`.
    // A script that already references its arguments is left untouched.
    let mut script = shell_command_line(program, args);
    if !(script.contains("$@") || script.contains("$*")) {
        script.push_str(r#" "$@""#);
    }

    duct::cmd("/bin/sh", ["-c", script.as_str(), "jp-editor"])
}

/// On non-unix platforms there is no portable way to forward the edited path
/// into a `shell = true` command (the `"$@"` convention is Unix-only), so the
/// command is always spawned directly.
/// `shell = true` is logged as unsupported and degraded to a direct spawn; use
/// `shell = false` and wrap any shell logic in a script on these platforms.
#[cfg(not(unix))]
fn editor_expression(program: &str, args: &[String], shell: bool) -> Expression {
    if shell {
        tracing::warn!(
            "`editor.cmd` with `shell = true` is not supported on this platform; running the \
             program directly. Use `shell = false` and wrap any shell logic in a script."
        );
    }

    duct::cmd(program, args.to_vec())
}

#[cfg(test)]
#[path = "editor_tests.rs"]
mod tests;
