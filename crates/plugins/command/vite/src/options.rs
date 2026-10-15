//! The plugin's settings, read from `plugins.command.vite.options`, and what it
//! reads of `jp rfd`'s and `jp ticket`'s.
//!
//! ```toml
//! [plugins.command.vite.options]
//! # The VitePress source directory.
//! dir = "docs"
//! # Where links to repository files point.
//! repository = "https://github.com/dcdpr/jp"
//! ```
//!
//! Every option is optional: `dir` defaults to `docs`, and `repository` to the
//! web address of git's `origin` remote.
//! Keys this plugin doesn't read are ignored, so a configuration written for a
//! newer `jp-vite` still loads.
//!
//! Each setting has one owner.
//! The document directories, the RFD categories, the ticket vocabulary, and how
//! much of the Done column the board shows belong to `jp rfd` and `jp ticket`,
//! and are read here from the whole configuration ([`rfd_options`],
//! [`ticket_options`]), with the defaults those plugins use.

use std::collections::BTreeSet;

use camino::Utf8PathBuf;
use jp_label::Vocabulary;
use serde::Deserialize;
use serde_json::{Map, Value};

/// The VitePress source directory when `dir` isn't set.
const DEFAULT_SITE_DIR: &str = "docs";

/// Where `jp rfd` keeps the RFDs when its `dir` isn't set.
const DEFAULT_RFD_DIR: &str = "docs/rfd";

/// `jp rfd`'s categories when its `categories` isn't set.
const DEFAULT_CATEGORIES: [&str; 4] = ["design", "decision", "guide", "process"];

/// Where `jp ticket` keeps the tickets when its `dir` isn't set.
const DEFAULT_TICKET_DIR: &str = "docs/ticket";

/// How many Done tickets the board shows when `jp ticket`'s `board.done_limit`
/// isn't set.
const DEFAULT_DONE_LIMIT: usize = 8;

/// The plugin's own settings.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub(crate) struct Options {
    /// The VitePress source directory, relative to the workspace root.
    pub dir: Utf8PathBuf,

    /// Where links to repository files point; `None` takes git's `origin`.
    pub repository: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            dir: Utf8PathBuf::from(DEFAULT_SITE_DIR),
            repository: None,
        }
    }
}

impl Options {
    /// Read the options the host passed in `init`, or in a reloaded
    /// configuration.
    ///
    /// The error names the setting and says what is wrong with it, for the user
    /// to fix in their configuration.
    pub(crate) fn read(options: &Map<String, Value>) -> Result<Self, String> {
        let read: Self = serde_json::from_value(Value::Object(options.clone()))
            .map_err(|error| format!("`plugins.command.vite.options` is invalid: {error}."))?;

        if read.dir.as_str().trim().is_empty() {
            return Err("`plugins.command.vite.options.dir` is empty.".to_owned());
        }

        let repository = match read.repository {
            None => None,
            Some(repository) if is_web_address(&repository) => {
                Some(repository.trim_end_matches('/').to_owned())
            }
            Some(_) => {
                return Err(
                    "`plugins.command.vite.options.repository` must be an `https://` address."
                        .to_owned(),
                );
            }
        };

        Ok(Self { repository, ..read })
    }

    /// The options in a configuration: the ones under
    /// `plugins.command.vite.options`, or the defaults.
    pub(crate) fn from_config(config: &Value) -> Result<Self, String> {
        match config.pointer("/plugins/command/vite/options") {
            Some(Value::Object(options)) => Self::read(options),
            None | Some(Value::Null) => Ok(Self::default()),
            Some(_) => Err("`plugins.command.vite.options` is not a table.".to_owned()),
        }
    }
}

/// What the site reads of `jp rfd`'s options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RfdOptions {
    /// Where the RFDs live, as configured.
    pub dir: Utf8PathBuf,

    /// The categories an RFD can carry, lowercase, each once.
    pub categories: Vec<String>,
}

/// What the site reads of `jp ticket`'s options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TicketOptions {
    /// Where the tickets live, as configured.
    pub dir: Utf8PathBuf,

    /// How many Done tickets the board shows.
    pub done_limit: usize,

    /// The labels a ticket can carry.
    pub labels: Vocabulary,
}

/// `jp rfd`'s options, as the configuration sets them.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RfdSection {
    dir: Option<Utf8PathBuf>,
    categories: Option<Vec<String>>,
}

/// `jp ticket`'s options, as the configuration sets them.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct TicketSection {
    dir: Option<Utf8PathBuf>,
    board: BoardSection,
    labels: Option<Value>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct BoardSection {
    done_limit: Option<usize>,
}

/// What the site reads of `jp rfd`'s options, from the whole configuration the
/// host passed.
pub(crate) fn rfd_options(config: &Value) -> Result<RfdOptions, String> {
    let section: RfdSection = section(config, "rfd")?;

    let categories = match section.categories {
        None => DEFAULT_CATEGORIES.map(ToOwned::to_owned).to_vec(),
        Some(categories) => {
            let mut seen = BTreeSet::new();
            let categories: Vec<String> = categories
                .iter()
                .map(|category| category.trim().to_lowercase())
                .filter(|category| !category.is_empty() && seen.insert(category.clone()))
                .collect();

            if categories.is_empty() {
                return Err("`plugins.command.rfd.options.categories` is empty.".to_owned());
            }
            categories
        }
    };

    Ok(RfdOptions {
        dir: directory(section.dir, DEFAULT_RFD_DIR, "rfd")?,
        categories,
    })
}

/// What the site reads of `jp ticket`'s options, from the whole configuration
/// the host passed.
pub(crate) fn ticket_options(config: &Value) -> Result<TicketOptions, String> {
    let section: TicketSection = section(config, "ticket")?;

    let labels = match section.labels {
        None | Some(Value::Null) => Vocabulary::default(),
        Some(labels) => Vocabulary::from_value(labels)
            .map_err(|error| format!("`plugins.command.ticket.options.labels`: {error}"))?,
    };

    Ok(TicketOptions {
        dir: directory(section.dir, DEFAULT_TICKET_DIR, "ticket")?,
        done_limit: section.board.done_limit.unwrap_or(DEFAULT_DONE_LIMIT),
        labels,
    })
}

/// The options of the plugin `name`, read into `T`.
fn section<T: Default + for<'de> Deserialize<'de>>(
    config: &Value,
    name: &str,
) -> Result<T, String> {
    match config.pointer(&format!("/plugins/command/{name}/options")) {
        None | Some(Value::Null) => Ok(T::default()),
        Some(options) => serde_json::from_value(options.clone())
            .map_err(|error| format!("`plugins.command.{name}.options` is invalid: {error}.")),
    }
}

/// A plugin's configured directory, or its default.
fn directory(dir: Option<Utf8PathBuf>, default: &str, name: &str) -> Result<Utf8PathBuf, String> {
    match dir {
        None => Ok(Utf8PathBuf::from(default)),
        Some(dir) if dir.as_str().trim().is_empty() => {
            Err(format!("`plugins.command.{name}.options.dir` is empty."))
        }
        Some(dir) => Ok(dir),
    }
}

/// Whether `address` is somewhere a browser can follow a link to.
fn is_web_address(address: &str) -> bool {
    let rest = address
        .strip_prefix("https://")
        .or_else(|| address.strip_prefix("http://"));

    rest.is_some_and(|rest| {
        !rest.trim_matches('/').is_empty() && !rest.contains(char::is_whitespace)
    })
}

/// The web address of a git remote, for links to the files in it: no
/// credentials, and no `.git` suffix.
///
/// Reads `https://` and `http://` remotes, `ssh://` and `git://` ones, and the
/// `git@host:owner/repo` form, the last three as `https://` addresses.
/// `None` for a remote no address can be derived from, such as a path on disk.
pub(crate) fn web_address(remote: &str) -> Option<String> {
    let remote = remote.trim();

    // A port on an `ssh://` or `git://` remote belongs to that protocol, and
    // has to go; one on a web remote is part of the web address.
    let (scheme, rest, web) = if let Some(rest) = remote.strip_prefix("https://") {
        ("https", rest, true)
    } else if let Some(rest) = remote.strip_prefix("http://") {
        ("http", rest, true)
    } else if let Some(rest) = remote
        .strip_prefix("ssh://")
        .or_else(|| remote.strip_prefix("git://"))
    {
        ("https", rest, false)
    } else if remote.contains("://") {
        // `file://` and anything else with no web side.
        return None;
    } else if let Some((host, path)) = remote.split_once(':') {
        // `[user@]host:owner/repo`, which git reads as ssh when no slash comes
        // before the colon. A single letter there is a Windows drive.
        if host.contains('/') || host.len() < 2 || path.contains('\\') {
            return None;
        }
        return address("https", host, path, false);
    } else {
        return None;
    };

    let (host, path) = rest.split_once('/')?;
    address(scheme, host, path, web)
}

/// `scheme://host/path`, with the user taken off `host`, and `.git` and slashes
/// off `path`.
///
/// `keep_port` keeps a port on `host`; without it, a port is dropped.
fn address(scheme: &str, host: &str, path: &str, keep_port: bool) -> Option<String> {
    let host = host.rsplit_once('@').map_or(host, |(_, host)| host);
    let host = match host.rsplit_once(':') {
        Some((name, port)) if !keep_port && port.chars().all(|c| c.is_ascii_digit()) => name,
        _ => host,
    };

    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    if host.is_empty() || path.is_empty() || host.contains('/') {
        return None;
    }

    Some(format!("{scheme}://{host}/{path}"))
}

#[cfg(test)]
#[path = "options_tests.rs"]
mod tests;
