//! Configuration under `plugins.command.release.options`.
//!
//! ```toml
//! [plugins.command.release.options]
//! fragments_dir = "changelog.d"
//! forge = "github"
//! repository = "https://github.com/dcdpr/jp"
//! bump = "cargo set-version --package jp_cli {version}"
//! build = "bash .config/release/build.sh {version} {source} {out} {profile}"
//! cadence = "2w"
//! format_on_write = true
//!
//! [plugins.command.release.options.headings]
//! added = "New features"
//! fixed = "Bug fixes"
//! ```
//!
//! - `fragments_dir`, `forge`, `repository`, and `headings` are shared with
//!   other plugins via [`release_options::Notes`].
//! - `format_on_write` (default `false`) formats written fragments, highlights,
//!   and `README.md` using the workspace's `.comfort.toml`.
//! - `bump` is the command run in the workspace root (with `{version}`
//!   substituted) to update version numbers in project manifests.
//!   It is split into words with shell quoting rules and executed directly
//!   without a shell.
//! - `build` is the command run by `jp release build` in the workspace root,
//!   with `{version}`, `{source}`, `{out}`, and `{profile}` substituted.
//!   Both `{source}` and `{out}` are required.
//! - `cadence` (default `"2w"`) is the duration after a minor or major release
//!   cut before `jp release due` reports the next release as due.
//!
//! Unknown option keys are ignored for forward compatibility.

use std::{fmt, time::Duration};

use camino::Utf8Path;
use release_options::Notes;
use semver::Version;
use serde::Deserialize;
use serde_json::{Map, Value};

const VERSION: &str = "{version}";
pub(crate) const BUMP_KEY: &str = "plugins.command.release.options.bump";
pub(crate) const BUILD_KEY: &str = "plugins.command.release.options.build";
const SOURCE: &str = "{source}";
const OUT: &str = "{out}";
const PROFILE: &str = "{profile}";
const DEFAULT_CADENCE: (&str, u64) = ("2w", 14 * 24 * 60 * 60);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Options {
    pub notes: Notes,
    pub bump: Option<Bump>,
    pub build: Option<Build>,
    pub cadence: Cadence,
}

/// Release cadence duration paired with its original configuration string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Cadence {
    text: String,
    duration: Duration,
}

impl Cadence {
    fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let duration = humantime::parse_duration(text).map_err(|error| {
            format!(
                "`plugins.command.release.options.cadence`: `{text}` is not a duration, such as \
                 `2w` or `10d`: {error}."
            )
        })?;

        Ok(Self {
            text: text.to_owned(),
            duration,
        })
    }

    pub(crate) fn seconds(&self) -> i64 {
        i64::try_from(self.duration.as_secs()).unwrap_or(i64::MAX)
    }
}

impl Default for Cadence {
    fn default() -> Self {
        let (text, seconds) = DEFAULT_CADENCE;

        Self {
            text: text.to_owned(),
            duration: Duration::from_secs(seconds),
        }
    }
}

impl fmt::Display for Cadence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

/// Parsed `bump` command template containing at least one `{version}`
/// placeholder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Bump {
    words: Vec<String>,
}

impl Bump {
    fn parse(command: &str) -> Result<Self, String> {
        let words = split(BUMP_KEY, command)?;
        if !words.iter().any(|word| word.contains(VERSION)) {
            return Err(format!(
                "`{BUMP_KEY}` names no `{VERSION}`, so it couldn't write the new version."
            ));
        }

        Ok(Self { words })
    }

    /// Return `(program, args)` with `{version}` replaced by `version`.
    pub(crate) fn command(&self, version: &Version) -> (String, Vec<String>) {
        let version = version.to_string();
        let mut words = self
            .words
            .iter()
            .map(|word| word.replace(VERSION, &version));
        let program = words.next().unwrap_or_default();

        (program, words.collect())
    }
}

/// Parsed `build` command template containing `{source}` and `{out}`
/// placeholders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Build {
    words: Vec<String>,
}

impl Build {
    fn parse(command: &str) -> Result<Self, String> {
        let words = split(BUILD_KEY, command)?;
        for (placeholder, why) in [
            (SOURCE, "so it couldn't find the release's code"),
            (OUT, "so it couldn't say where the build goes"),
        ] {
            if !words.iter().any(|word| word.contains(placeholder)) {
                return Err(format!("`{BUILD_KEY}` names no `{placeholder}`, {why}."));
            }
        }

        Ok(Self { words })
    }

    /// Return `(program, args)` with `{version}`, `{source}`, `{out}`, and
    /// `{profile}` substituted.
    pub(crate) fn command(
        &self,
        version: &Version,
        source: &Utf8Path,
        out: &Utf8Path,
        profile: &str,
    ) -> (String, Vec<String>) {
        let version = version.to_string();
        let values = [
            (VERSION, version.as_str()),
            (SOURCE, source.as_str()),
            (OUT, out.as_str()),
            (PROFILE, profile),
        ];
        let mut words = self.words.iter().map(|word| fill(word, &values));
        let program = words.next().unwrap_or_default();

        (program, words.collect())
    }
}

fn split(key: &str, command: &str) -> Result<Vec<String>, String> {
    let words = shlex::split(command)
        .ok_or_else(|| format!("`{key}` can't be split into words; check its quotes."))?;
    if words.is_empty() {
        return Err(format!("`{key}` is empty."));
    }

    Ok(words)
}

/// Substitute all placeholders in `word` in a single left-to-right pass so a
/// replacement value that contains a placeholder literal is not substituted a
/// second time.
fn fill(word: &str, values: &[(&str, &str)]) -> String {
    let mut filled = String::with_capacity(word.len());
    let mut rest = word;
    'scan: while !rest.is_empty() {
        for (placeholder, value) in values {
            if let Some(after) = rest.strip_prefix(placeholder) {
                filled.push_str(value);
                rest = after;
                continue 'scan;
            }
        }

        let mut chars = rest.chars();
        filled.extend(chars.next());
        rest = chars.as_str();
    }

    filled
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Settings {
    bump: Option<String>,
    build: Option<String>,
    cadence: Option<String>,
}

impl Options {
    pub(crate) fn read(options: &Map<String, Value>) -> Result<Self, String> {
        let notes = Notes::read(options)?;
        let settings: Settings = serde_json::from_value(Value::Object(options.clone()))
            .map_err(|error| format!("`plugins.command.release.options` is invalid: {error}."))?;

        let bump = settings.bump.as_deref().map(Bump::parse).transpose()?;
        let build = settings.build.as_deref().map(Build::parse).transpose()?;

        let cadence = settings
            .cadence
            .as_deref()
            .map(Cadence::parse)
            .transpose()?
            .unwrap_or_default();

        Ok(Self {
            notes,
            bump,
            build,
            cadence,
        })
    }
}

#[cfg(test)]
#[path = "options_tests.rs"]
mod tests;
