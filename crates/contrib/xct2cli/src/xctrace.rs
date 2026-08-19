//! Subprocess wrapper around the `xctrace` CLI.

use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::error::{Error, Result};

pub const DEFAULT_XCTRACE: &str = "/usr/bin/xctrace";

#[derive(Debug, Clone)]
pub struct Xctrace {
    binary: PathBuf,
}

impl Default for Xctrace {
    fn default() -> Self {
        Self {
            binary: PathBuf::from(DEFAULT_XCTRACE),
        }
    }
}

impl Xctrace {
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self {
            binary: path.into(),
        }
    }

    /// Resolve via `$XCTRACE_BIN`, falling back to `/usr/bin/xctrace`.
    pub fn discover() -> Self {
        if let Ok(env_path) = std::env::var("XCTRACE_BIN") {
            return Self::at(env_path);
        }
        Self::default()
    }

    /// Crate-private: raw XML carries the recorded process's environment, so it
    /// must not leave without passing through a parser that models only the
    /// fields this crate understands.
    /// Callers get [`crate::trace::Toc`].
    pub(crate) fn export_toc(&self, trace: &Path) -> Result<Vec<u8>> {
        self.run("export", &[
            OsStr::new("--input"),
            trace.as_os_str(),
            OsStr::new("--toc"),
        ])
    }

    /// Crate-private for the same reason as [`Self::export_toc`].
    /// Callers get [`crate::trace::QueryResult`].
    pub(crate) fn export_xpath(&self, trace: &Path, xpath: &str) -> Result<Vec<u8>> {
        self.run("export", &[
            OsStr::new("--input"),
            trace.as_os_str(),
            OsStr::new("--xpath"),
            OsStr::new(xpath),
        ])
    }

    fn run(&self, sub: &'static str, tail: &[&OsStr]) -> Result<Vec<u8>> {
        let mut args: Vec<OsString> = Vec::with_capacity(tail.len() + 1);
        args.push(sub.into());
        for t in tail {
            args.push((*t).to_owned());
        }
        self.run_args(sub, &args)
    }

    fn run_args(&self, sub: &'static str, args: &[OsString]) -> Result<Vec<u8>> {
        let mut cmd = Command::new(&self.binary);
        cmd.args(args);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        // Safe to log verbatim: every subcommand reached from here passes paths
        // and xpaths. Nothing constructs an `--env` argument, which is what a
        // recording would carry and what must never reach a log.
        tracing::debug!(?args, binary = ?self.binary, "spawning xctrace");
        let out = cmd.output().map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Error::XctraceMissing(self.binary.clone()),
            _ => Error::Io(e),
        })?;
        if !out.status.success() {
            return Err(Error::XctraceFailed {
                subcommand: sub,
                status: out.status,
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            });
        }
        // Every byte xctrace produces leaves through here, so this is the one
        // place that can guarantee the recorded process's environment never
        // reaches a caller.
        Ok(crate::redact::strip_environment(out.stdout))
    }
}
