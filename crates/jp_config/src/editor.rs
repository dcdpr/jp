//! Editor configuration for Jean-Pierre.

use std::env;

use duct::Expression;
use schematic::{Config, ConfigEnum};
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt, delta_opt_vec},
    fill::FillDefaults,
    partial::{ToPartial, partial_opt, partial_opts},
};

/// Editor configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct EditorConfig {
    /// The command to use for editing text.
    ///
    /// If unset, falls back to `envs`.
    pub cmd: Option<String>,

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
            "cmd" => self.cmd = kv.try_some_string()?,
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
            cmd: delta_opt(self.cmd.as_ref(), next.cmd),
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
            cmd: partial_opts(self.cmd.as_ref(), defaults.cmd),
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
    /// The command to use for editing text.
    ///
    /// If no command is configured, and no configured environment variables are
    /// set, returns `None`.
    ///
    /// Env-var values are split with [`shlex::split`] so `JP_EDITOR="code -w"`
    /// correctly runs `code` with `-w` as an argument.
    /// Values with unbalanced quoting are skipped.
    /// The `cmd` field uses full shell-mode parsing (via
    /// `duct_sh::sh_dangerous`) for backwards compatibility with shell
    /// metacharacters like `&&` and `|`.
    #[must_use]
    pub fn command(&self) -> Option<Expression> {
        self.cmd.clone().map(duct_sh::sh_dangerous).or_else(|| {
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

#[cfg(test)]
#[path = "editor_tests.rs"]
mod tests;
