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

    /// The environment variables to use for editing text. Used if `cmd` is
    /// unset.
    ///
    /// Defaults to `JP_EDITOR`, `VISUAL`, and `EDITOR`.
    ///
    /// Note that for security reasons, the value of these environment variables
    /// are split by whitespace, and only the first element is used for the
    /// command. Meaning, you cannot set `JP_EDITOR="subl -w"`, because it will
    /// only run `subl`. You can either create your own wrapper script, and call
    /// that directly (e.g. `sublw`), or set the `cmd` option to `subl -w`, as
    /// that will use all elements of the command.
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
    #[must_use]
    pub fn command(&self) -> Option<Expression> {
        self.cmd.clone().map(duct_sh::sh_dangerous).or_else(|| {
            self.envs.iter().find_map(|v| {
                env::var(v)
                    .ok()
                    .filter(|s| {
                        s.split_ascii_whitespace()
                            .next()
                            .is_some_and(|c| which::which(c).is_ok())
                    })
                    .map(|s| {
                        duct::cmd::<&str, &[&str]>(
                            s.split_ascii_whitespace().next().unwrap_or(&s),
                            &[],
                        )
                    })
            })
        })
    }

    /// Return the path to the editor, if any.
    #[must_use]
    pub fn path(&self) -> Option<Utf8PathBuf> {
        self.cmd.as_ref().map(Utf8PathBuf::from).or_else(|| {
            self.envs.iter().find_map(|v| {
                env::var(v).ok().and_then(|s| {
                    s.split_ascii_whitespace()
                        .next()
                        .and_then(|c| which::which(c).ok().and_then(|p| p.try_into().ok()))
                })
            })
        })
    }
}

#[cfg(test)]
#[path = "editor_tests.rs"]
mod tests;
