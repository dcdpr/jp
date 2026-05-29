//! Editor configuration for Jean-Pierre.

use std::env;

use camino::Utf8PathBuf;
use duct::Expression;
use schematic::Config;

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
}

impl AssignKeyValue for PartialEditorConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "cmd" => self.cmd = kv.try_some_string()?,
            _ if kv.p("envs") => kv.try_some_vec_of_strings(&mut self.envs)?,
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
        }
    }
}

impl FillDefaults for PartialEditorConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            cmd: self.cmd.or(defaults.cmd),
            envs: self.envs.or(defaults.envs),
        }
    }
}

impl ToPartial for EditorConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            cmd: partial_opts(self.cmd.as_ref(), defaults.cmd),
            envs: partial_opt(&self.envs, defaults.envs),
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

    /// Return the path to the editor, if any.
    ///
    /// For env-var fallback, the first shlex token is taken as the binary; any
    /// additional arguments are dropped (use [`Self::command`] when the caller
    /// can invoke the program with arguments).
    #[must_use]
    pub fn path(&self) -> Option<Utf8PathBuf> {
        self.cmd.as_ref().map(Utf8PathBuf::from).or_else(|| {
            self.envs.iter().find_map(|v| {
                let value = env::var(v).ok()?;
                let program = shlex::split(&value)?.into_iter().next()?;
                which::which(&program).ok().and_then(|p| p.try_into().ok())
            })
        })
    }
}

#[cfg(test)]
#[path = "editor_tests.rs"]
mod tests;
