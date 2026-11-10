//! The plugin's settings, read from `plugins.command.release.options`.
//!
//! ```toml
//! [plugins.command.release.options]
//! fragments_dir = "changelog.d"
//! forge = "github"
//! bump = "cargo set-version --package jp_cli {version}"
//! build = "bash .config/release/build.sh {source} {out} {profile}"
//! cadence = "2w"
//!
//! [plugins.command.release.options.headings]
//! added = "New features"
//! fixed = "Bug fixes"
//! ```
//!
//! `format_on_write = true` lays every fragment, highlight, and README the
//! plugin writes out in the style of the workspace's `.comfort.toml`; it is off
//! by default.
//!
//! `forge` is where change requests are reviewed, `github`, `gitlab`, or
//! `none`, which decides how release notes credit them and where the credits
//! link; `jp release` itself never talks to it.
//! Unset, it is read from git's `origin` remote: `github` for github.com,
//! `gitlab` for gitlab.com, and `none` for any other host.
//! `headings` renames a type's section of the release notes; a type it doesn't
//! name is headed by its id capitalized, such as `Added`.
//!
//! `bump` is the command that writes a new version wherever the project keeps
//! it, run in the workspace root with `{version}` replaced by the new version.
//! It is split into words as a shell splits them, but runs without a shell.
//! Unset, a version lives only in its tag.
//!
//! `build` is the command `jp release build` runs in the workspace root to
//! build a release's binaries, split into words as `bump` is, with `{version}`,
//! the release's version, `{source}`, a checkout of its code, `{out}`, the
//! empty directory its archives, `checksums.txt`, and `manifest.json` go, and
//! `{profile}`, `release` or `quick`.
//! It has to name `{source}` and `{out}`.
//! Unset, `jp release build` refuses to run.
//!
//! `cadence` is how long after a minor or major release is cut the next one is
//! due, which `jp release due` reports: a duration such as `2w`, `10d`, or
//! `1week 3days`, two weeks by default.
//!
//! Every option is optional.
//! Keys this plugin doesn't read are ignored, so a configuration written for a
//! newer `jp-release` still loads.

use std::{collections::BTreeMap, fmt, time::Duration};

use camino::{Utf8Path, Utf8PathBuf};
use changelog::{Forge, Headings, Type, UnknownType};
use semver::Version;
use serde::Deserialize;
use serde_json::{Map, Value};

/// The fragments directory, when `fragments_dir` isn't set.
const DEFAULT_FRAGMENTS_DIR: &str = "changelog.d";

/// What `bump` writes where the new version goes.
const VERSION: &str = "{version}";

/// The setting `bump` is read from, as errors name it.
pub(crate) const BUMP_KEY: &str = "plugins.command.release.options.bump";

/// The setting `build` is read from, as errors name it.
pub(crate) const BUILD_KEY: &str = "plugins.command.release.options.build";

/// What `build` writes where the checkout of the release's code is.
const SOURCE: &str = "{source}";

/// What `build` writes where the directory the build goes is.
const OUT: &str = "{out}";

/// What `build` writes where the profile to build with goes.
const PROFILE: &str = "{profile}";

/// The cadence, when `cadence` isn't set, as configuration writes it and in
/// seconds.
const DEFAULT_CADENCE: (&str, u64) = ("2w", 14 * 24 * 60 * 60);

/// The settings every command and tool runs with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Options {
    /// The directory holding the fragments and highlights, relative to the
    /// workspace root.
    pub fragments_dir: Utf8PathBuf,

    /// The forge release notes credit change requests on; `None` reads it from
    /// git's `origin` remote.
    pub forge: Option<Forge>,

    /// The heading of each type's section of the release notes.
    pub headings: Headings,

    /// The command that writes a new version where the project keeps it.
    pub bump: Option<Bump>,

    /// The command that builds a release's binaries.
    pub build: Option<Build>,

    /// How long after a minor or major release is cut the next one is due.
    pub cadence: Cadence,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            fragments_dir: Utf8PathBuf::from(DEFAULT_FRAGMENTS_DIR),
            forge: None,
            headings: Headings::default(),
            bump: None,
            build: None,
            cadence: Cadence::default(),
        }
    }
}

/// The `cadence` option: a duration, kept as configuration writes it to show it
/// the same way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Cadence {
    text: String,
    duration: Duration,
}

impl Cadence {
    /// Read `text`, a duration as `jp query --tmp` takes one, such as `2w`.
    ///
    /// The error says what is wrong with it.
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

    /// The cadence in whole seconds.
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

/// The command `bump` configures: words as a shell splits them, `{version}` in
/// at least one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Bump {
    words: Vec<String>,
}

impl Bump {
    /// Split `command` into the program and its arguments.
    ///
    /// The error says what is wrong with it.
    fn parse(command: &str) -> Result<Self, String> {
        let words = split(BUMP_KEY, command)?;
        if !words.iter().any(|word| word.contains(VERSION)) {
            return Err(format!(
                "`{BUMP_KEY}` names no `{VERSION}`, so it couldn't write the new version."
            ));
        }

        Ok(Self { words })
    }

    /// The program to run, and its arguments, with every `{version}` replaced
    /// by `version`.
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

/// The command `build` configures: words as a shell splits them, `{source}` and
/// `{out}` each in at least one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Build {
    words: Vec<String>,
}

impl Build {
    /// Split `command` into the program and its arguments.
    ///
    /// The error says what is wrong with it.
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

    /// The program to run, and its arguments, with `{version}`, `{source}`,
    /// `{out}`, and `{profile}` replaced.
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

/// `command` split into words as a shell splits them, for the setting `key`.
///
/// The error says it can't be split, or holds no words.
fn split(key: &str, command: &str) -> Result<Vec<String>, String> {
    let words = shlex::split(command)
        .ok_or_else(|| format!("`{key}` can't be split into words; check its quotes."))?;
    if words.is_empty() {
        return Err(format!("`{key}` is empty."));
    }

    Ok(words)
}

/// `word` with each placeholder of `values` replaced by its value, in one pass,
/// so a value holding a placeholder, such as a path with `{out}` in it, is left
/// as it is.
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

/// The options as the configuration writes them.
#[derive(Deserialize)]
#[serde(default)]
struct Settings {
    fragments_dir: Utf8PathBuf,
    forge: Option<String>,
    headings: BTreeMap<String, String>,
    bump: Option<String>,
    build: Option<String>,
    cadence: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            fragments_dir: Utf8PathBuf::from(DEFAULT_FRAGMENTS_DIR),
            forge: None,
            headings: BTreeMap::new(),
            bump: None,
            build: None,
            cadence: None,
        }
    }
}

impl Options {
    /// Read the options the host passed in `init`.
    ///
    /// The error names the setting and says what is wrong with it, for the user
    /// to fix in their configuration.
    pub(crate) fn read(options: &Map<String, Value>) -> Result<Self, String> {
        let settings: Settings = serde_json::from_value(Value::Object(options.clone()))
            .map_err(|error| format!("`plugins.command.release.options` is invalid: {error}."))?;

        if settings.fragments_dir.as_str().trim().is_empty() {
            return Err("`plugins.command.release.options.fragments_dir` is empty.".to_owned());
        }

        let forge = settings
            .forge
            .map(|forge| forge.parse::<Forge>())
            .transpose()
            .map_err(|error| format!("`plugins.command.release.options.forge`: {error}."))?;

        let mut headings = Headings::default();
        for (type_, heading) in &settings.headings {
            let type_ = type_.parse::<Type>().map_err(|error: UnknownType| {
                format!("`plugins.command.release.options.headings`: {error}.")
            })?;
            let heading = heading.trim();
            if heading.is_empty() {
                return Err(format!(
                    "`plugins.command.release.options.headings.{type_}` is empty."
                ));
            }
            headings.set(type_, heading);
        }

        let bump = settings.bump.as_deref().map(Bump::parse).transpose()?;
        let build = settings.build.as_deref().map(Build::parse).transpose()?;

        let cadence = settings
            .cadence
            .as_deref()
            .map(Cadence::parse)
            .transpose()?
            .unwrap_or_default();

        Ok(Self {
            fragments_dir: settings.fragments_dir,
            forge,
            headings,
            bump,
            build,
            cadence,
        })
    }
}

#[cfg(test)]
#[path = "options_tests.rs"]
mod tests;
