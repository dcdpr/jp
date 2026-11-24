//! The plugin's settings, read from `plugins.command.review.options`.
//!
//! ```toml
//! [plugins.command.review.options]
//! dir = ".jp/reviews"
//! cfg = ["skill/review"]
//! base = ["git", "show", "HEAD:./{path}"]
//! assistant = "jp"
//! max_chars = 600
//! bind = "127.0.0.1"
//! port = 3001
//! allowed_hosts = ["my-machine.local"]
//! ```
//!
//! Every option is optional.
//! Keys this plugin doesn't read are ignored, so a configuration written for a
//! newer `jp-review` still loads.
//! A flag on the command line wins over its option ([`Overrides`]).

use std::net::SocketAddr;

use camino::{Utf8Path, Utf8PathBuf};
use jp_plugin::server::Listen;
use serde::Deserialize;
use serde_json::{Map, Value};

/// Where reviews are stored, inside the workspace's user-local storage
/// directory, when `dir` isn't set.
const DEFAULT_DIR: &str = "review";

/// The author written on the assistant's replies and comments when `assistant`
/// isn't set.
const DEFAULT_ASSISTANT: &str = "assistant";

/// Default maximum character count for assistant replies and comments.
const DEFAULT_MAX_CHARS: usize = 600;

/// Default port for `jp review serve`.
const DEFAULT_PORT: u16 = 3001;

/// Config path of the options a pass forwards to its turn.
const OPTIONS_KEY: &str = "plugins.command.review.options";

/// Resolved plugin options from `plugins.command.review.options` and CLI
/// overrides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Options {
    /// Directory where reviews are stored; relative paths resolve against the
    /// workspace root.
    pub dir: Option<Utf8PathBuf>,

    /// `--cfg` arguments applied to every pass turn.
    pub cfg: Vec<String>,

    /// Command run to capture the base version of each file when starting a
    /// review (`{path}` is replaced with the file path).
    pub base: Option<Vec<String>>,

    /// Author name recorded on assistant replies and comments.
    pub assistant: String,

    /// Maximum character count for assistant replies and comments.
    pub max_chars: usize,

    /// Listener settings (`bind`, `port`, and `allowed_hosts`) for `jp review
    /// serve`.
    pub listen: Listen,
}

/// What the command line sets in place of an option.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Overrides {
    /// `--dir`: where reviews are stored, relative to the workspace root.
    pub dir: Option<Utf8PathBuf>,

    /// `jp review serve`'s `--bind`, `--port`, and `--allowed-host`.
    pub listen: Listen,

    /// `jp review serve`'s `--ui-dir`, made absolute: the directory to serve
    /// the browser UI from in place of the one built in.
    pub ui: Option<Utf8PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            dir: None,
            cfg: vec![],
            base: None,
            assistant: DEFAULT_ASSISTANT.to_owned(),
            max_chars: DEFAULT_MAX_CHARS,
            listen: Listen::default(),
        }
    }
}

/// The options as the configuration writes them.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Raw {
    dir: Option<Utf8PathBuf>,
    cfg: Vec<String>,
    base: Option<Vec<String>>,
    assistant: Option<String>,
    max_chars: Option<usize>,
}

impl Options {
    /// Read the options the host passed in `init`.
    ///
    /// The error names the setting and says what is wrong with it, for the user
    /// to fix in their configuration.
    pub(crate) fn read(options: &Map<String, Value>) -> Result<Self, String> {
        let raw: Raw = serde_json::from_value(Value::Object(options.clone()))
            .map_err(|error| format!("`plugins.command.review.options` is invalid: {error}."))?;

        if raw
            .dir
            .as_ref()
            .is_some_and(|dir| dir.as_str().trim().is_empty())
        {
            return Err("`plugins.command.review.options.dir` is empty.".to_owned());
        }

        let no_program =
            |base: &Vec<String>| base.first().is_none_or(|first| first.trim().is_empty());
        if raw.base.as_ref().is_some_and(no_program) {
            return Err(
                "`plugins.command.review.options.base` needs a program to run, as in `[\"git\", \
                 \"show\", \"HEAD:./{path}\"]`."
                    .to_owned(),
            );
        }

        Ok(Self {
            dir: raw.dir,
            cfg: raw.cfg,
            base: raw.base,
            assistant: assistant(raw.assistant)?,
            max_chars: max_chars(raw.max_chars)?,
            listen: Listen::from_options(options, "review")?,
        })
    }

    /// These options, with what `overrides` sets in their place.
    pub(crate) fn with(mut self, overrides: &Overrides) -> Self {
        if let Some(dir) = &overrides.dir {
            self.dir = Some(dir.clone());
        }

        self
    }

    /// The configurations a pass runs its turn under, before any the pass
    /// itself names.
    ///
    /// First `assistant`, `max_chars`, and `dir` when it is set, as these
    /// options hold them, then `cfg`, so a value `cfg` sets wins.
    /// The tools that answer a pass read their options from the turn, and a
    /// conversation's configuration doesn't follow later edits to the config
    /// files, so without these a turn could store its replies in another
    /// directory, or hold them to an older limit.
    /// Each is a JSON assignment (`KEY:=VALUE`), which keeps `max_chars` a
    /// number where `KEY=VALUE` would make it a string.
    pub(crate) fn pass_cfg(&self) -> Vec<String> {
        let mut forwarded = vec![
            ("assistant", Value::from(self.assistant.as_str())),
            ("max_chars", Value::from(self.max_chars)),
        ];
        if let Some(dir) = &self.dir {
            forwarded.push(("dir", Value::from(dir.as_str())));
        }

        forwarded
            .into_iter()
            .map(|(key, value)| format!("{OPTIONS_KEY}.{key}:={value}"))
            .chain(self.cfg.iter().cloned())
            .collect()
    }

    /// The address to listen on: the flags where they are set, then the `bind`
    /// and `port` options, then `127.0.0.1` on port 3001.
    pub(crate) fn listen_on(&self, flags: &Listen) -> SocketAddr {
        self.listen.address(flags, DEFAULT_PORT)
    }

    /// The directory reviews are stored in.
    ///
    /// `dir` against the workspace `root`, or `review` in the workspace's
    /// user-local storage directory, `user_workspace`, when `dir` is unset.
    pub(crate) fn store_dir(
        &self,
        root: &Utf8Path,
        user_workspace: Option<&Utf8Path>,
    ) -> Result<Utf8PathBuf, String> {
        match (&self.dir, user_workspace) {
            (Some(dir), _) => Ok(root.join(dir)),
            (None, Some(storage)) => Ok(storage.join(DEFAULT_DIR)),
            (None, None) => Err(
                "This workspace has no user-local storage to keep reviews in; set \
                 `plugins.command.review.options.dir` to a directory for them."
                    .to_owned(),
            ),
        }
    }
}

/// The `assistant` option: a name on one line, or the default.
fn assistant(raw: Option<String>) -> Result<String, String> {
    let Some(name) = raw else {
        return Ok(DEFAULT_ASSISTANT.to_owned());
    };

    let name = name.trim();
    if name.is_empty() || name.chars().any(char::is_control) {
        return Err(
            "`plugins.command.review.options.assistant` must be a name on one line.".to_owned(),
        );
    }

    Ok(name.to_owned())
}

/// The `max_chars` option: at least one character, or the default.
fn max_chars(raw: Option<usize>) -> Result<usize, String> {
    match raw {
        None => Ok(DEFAULT_MAX_CHARS),
        Some(0) => Err(
            "`plugins.command.review.options.max_chars` must allow at least one character."
                .to_owned(),
        ),
        Some(max) => Ok(max),
    }
}

#[cfg(test)]
#[path = "options_tests.rs"]
mod tests;
