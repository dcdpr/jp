//! Editor configuration for Jean-Pierre.

use std::{env, path::PathBuf};

use duct::Expression;
use schematic::Config;

use crate::{
    assignment::{missing_key, AssignKeyValue, AssignResult, KvAssignment},
    delta::{delta_opt, delta_opt_vec, PartialConfigDelta},
    partial::{partial_opt, partial_opts, ToPartial},
};

/// Editor configuration.
#[derive(Debug, Config)]
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
    /// # Safety
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
            "" => *self = kv.try_object()?,
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
    pub fn path(&self) -> Option<PathBuf> {
        self.cmd.as_ref().map(PathBuf::from).or_else(|| {
            self.envs.iter().find_map(|v| {
                env::var(v).ok().and_then(|s| {
                    s.split_ascii_whitespace()
                        .next()
                        .and_then(|c| which::which(c).ok())
                })
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use serial_test::serial;

    use super::*;
    use crate::{assignment::KvAssignment, util::EnvVarGuard};

    #[test]
    fn test_editor_config_cmd() {
        let mut p = PartialEditorConfig::default();

        let kv = KvAssignment::try_from_cli("cmd", "vim").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.cmd, Some("vim".into()));

        let kv = KvAssignment::try_from_cli("cmd", "subl -w").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.cmd, Some("subl -w".into()));
    }

    #[test]
    fn test_editor_config_envs() {
        let mut p = PartialEditorConfig::default();

        let kv = KvAssignment::try_from_cli("envs", "EDITOR,VISUAL").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.envs, Some(vec!["EDITOR".into(), "VISUAL".into()]));

        let kv = KvAssignment::try_from_cli("envs:", r#"["EDITOR","VISUAL"]"#).unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.envs, Some(vec!["EDITOR".into(), "VISUAL".into()]));

        let kv = KvAssignment::try_from_cli("envs.0", "EDIT").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.envs, Some(vec!["EDIT".into(), "VISUAL".into()]));

        let kv = KvAssignment::try_from_cli("envs+:", r#"["OTHER"]"#).unwrap();
        p.assign(kv).unwrap();
        assert_eq!(
            p.envs,
            Some(vec!["EDIT".into(), "VISUAL".into(), "OTHER".into()])
        );

        let kv = KvAssignment::try_from_cli("envs+", "LAST").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(
            p.envs,
            Some(vec![
                "EDIT".into(),
                "VISUAL".into(),
                "OTHER".into(),
                "LAST".into()
            ])
        );
    }

    #[test]
    #[serial(env_vars)]
    fn test_editor_config_path() {
        let mut p = EditorConfig {
            cmd: Some("vim".into()),
            envs: vec![],
        };

        assert_eq!(p.path(), Some(PathBuf::from("vim")));

        p.cmd = Some("subl -w".into());
        assert_eq!(p.path(), Some(PathBuf::from("subl -w")));

        p.cmd = Some("/usr/bin/vim".into());
        assert_eq!(p.path(), Some(PathBuf::from("/usr/bin/vim")));

        p.cmd = None;
        p.envs = vec![];
        assert_eq!(p.path(), None);

        let _env = EnvVarGuard::set("JP_EDITOR1", "vi");
        p.envs = vec!["JP_EDITOR1".into()];
        assert!(p.path().unwrap().to_string_lossy().ends_with("/bin/vi"));

        let _env = EnvVarGuard::set("JP_EDITOR2", "doesnotexist");
        p.envs = vec!["JP_EDITOR2".into()];
        assert_eq!(p.path(), None);
    }
}
