//! The plugin's settings, read from `plugins.command.vite.options`, and what it
//! reads of `jp rfd`'s, `jp ticket`'s, `jp wiki`'s, and `jp release`'s.
//!
//! ```toml
//! [plugins.command.vite.options]
//! # The VitePress source directory.
//! dir = "docs"
//! # Where links to repository files point.
//! repository = "https://github.com/dcdpr/jp"
//! # The branch a page's "edit" link opens the forge's editor on.
//! branch = "main"
//!
//! # Pages that moved: an old route, and the route or wiki page it went to.
//! [plugins.command.vite.options.redirects]
//! "architecture/ubiquitous-language" = "W-0a1b2c3"
//! ```
//!
//! Every option is optional: `dir` defaults to `docs`, `repository` to the web
//! address of git's `origin` remote, `branch` to the default branch git's
//! `origin/HEAD` names, and `redirects` to none.
//! The settings snapshot is committed, so a project whose checkouts disagree
//! about `origin` sets `repository` and `branch` rather than reading them: a CI
//! clone, for one, has no `origin/HEAD`.
//! Keys this plugin doesn't read are ignored, so a configuration written for a
//! newer `jp-vite` still loads.
//!
//! Each setting has one owner.
//! The document directories, the RFD categories, the label keys a ticket or a
//! wiki page names an RFD under, the ticket vocabulary, how much of the Done
//! column the board shows, and the fragments directory, forge, web address, and
//! headings the release notes are rendered with belong to `jp rfd`, `jp
//! ticket`, `jp wiki`, and `jp release`, and are read here from the whole
//! configuration ([`rfd_options`], [`ticket_options`], [`wiki_options`],
//! [`release_options`]), with the defaults those plugins use.

use std::collections::{BTreeMap, BTreeSet};

use camino::Utf8PathBuf;
use changelog::{Forge, Type, UnknownType};
use doc::repository::is_web_address;
pub(crate) use doc::repository::web_address;
use jp_label::Vocabulary;
use serde::Deserialize;
use serde_json::{Map, Value};

/// The VitePress source directory when `dir` isn't set.
const DEFAULT_SITE_DIR: &str = "docs";

/// Where `jp rfd` keeps the RFDs when its `dir` isn't set.
const DEFAULT_RFD_DIR: &str = "docs/rfd";

/// `jp rfd`'s categories when its `categories` isn't set.
const DEFAULT_CATEGORIES: [&str; 4] = ["design", "decision", "guide", "process"];

/// The key a tracking ticket names its RFD under when `jp rfd`'s `tracking.key`
/// isn't set.
const DEFAULT_TRACKING_KEY: &str = "tracking-rfd";

/// The key a ticket names an RFD its work belongs to under when `jp rfd`'s
/// `relevant.key` isn't set.
const DEFAULT_RELEVANT_KEY: &str = "relevant-rfd";

/// Where `jp ticket` keeps the tickets when its `dir` isn't set.
const DEFAULT_TICKET_DIR: &str = "docs/ticket";

/// How many Done tickets the board shows when `jp ticket`'s `board.done_limit`
/// isn't set.
const DEFAULT_DONE_LIMIT: usize = 8;

/// Where `jp wiki` keeps the pages when its `dir` isn't set.
const DEFAULT_WIKI_DIR: &str = "docs/wiki";

/// Where `jp release` keeps the fragments when its `fragments_dir` isn't set.
const DEFAULT_FRAGMENTS_DIR: &str = "changelog.d";

/// The plugin's own settings.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub(crate) struct Options {
    /// The VitePress source directory, relative to the workspace root.
    pub dir: Utf8PathBuf,

    /// Where links to repository files point; `None` takes git's `origin`.
    pub repository: Option<String>,

    /// The branch a page is edited on; `None` takes git's `origin/HEAD`.
    pub branch: Option<String>,

    /// Pages that moved: each old route, `/architecture/glossary`, and where it
    /// went, a route or a wiki page id, `W-0a1b2c3`.
    pub redirects: BTreeMap<String, String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            dir: Utf8PathBuf::from(DEFAULT_SITE_DIR),
            repository: None,
            branch: None,
            redirects: BTreeMap::new(),
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

        let branch = match read.branch.as_deref().map(str::trim) {
            None => None,
            Some(branch) if !branch.is_empty() && !branch.contains(char::is_whitespace) => {
                Some(branch.to_owned())
            }
            Some(_) => {
                return Err(
                    "`plugins.command.vite.options.branch` must be a branch name, such as `main`."
                        .to_owned(),
                );
            }
        };

        let mut redirects = BTreeMap::new();
        for (from, to) in &read.redirects {
            let refused = |why: String| {
                format!("`plugins.command.vite.options.redirects` maps `{from}`: {why}")
            };
            let from = route(from).map_err(refused)?;
            let to = wiki_page(to)
                .map_or_else(|| route(to), Ok)
                .map_err(refused)?;
            if redirects.insert(from.clone(), to).is_some() {
                return Err(format!(
                    "`plugins.command.vite.options.redirects` maps `{from}` more than once."
                ));
            }
        }

        Ok(Self {
            repository,
            branch,
            redirects,
            ..read
        })
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

    /// The label key a tracking ticket names its RFD under.
    pub tracking_key: String,

    /// The label key a ticket names an RFD its work belongs to under.
    pub relevant_key: String,
}

/// A route on the site as a redirect names it: below the site's root, with `/`
/// in front, without `.html` or `.md`, and with a trailing `/` for a
/// directory's index.
///
/// The error says what is wrong with `text`.
fn route(text: &str) -> Result<String, String> {
    let text = text.trim();
    let bare = text.trim_start_matches('/');
    let bare = bare
        .strip_suffix(".html")
        .or_else(|| bare.strip_suffix(".md"))
        .unwrap_or(bare);

    if bare.is_empty() {
        return Err("a route names a page below the site's root.".to_owned());
    }
    if bare.contains(['?', '#', '\\']) || bare.chars().any(char::is_whitespace) {
        return Err("a route holds no `?`, `#`, `\\`, or whitespace.".to_owned());
    }
    if bare.split('/').any(|part| part == ".." || part == ".") {
        return Err("a route holds no `.` or `..` part.".to_owned());
    }

    Ok(format!("/{bare}"))
}

/// The wiki page `text` names, as `W-0a1b2c3`, when it is a page id: `W-`, or
/// `W` alone, then the seven characters of the id, in any case.
fn wiki_page(text: &str) -> Option<String> {
    let text = text.trim().to_ascii_lowercase();
    let body = text.strip_prefix("w-").or_else(|| text.strip_prefix('w'))?;
    let id = body.len() == 7
        && body
            .bytes()
            .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase());

    id.then(|| format!("W-{body}"))
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

/// What the site reads of `jp wiki`'s options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WikiOptions {
    /// Where the pages live, as configured.
    pub dir: Utf8PathBuf,

    /// The label key a page names an RFD it relates to under.
    pub relevant_key: String,
}

/// What the site reads of `jp release`'s options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReleaseOptions {
    /// Where the fragments live, as configured.
    pub dir: Utf8PathBuf,

    /// The forge the release notes credit change requests on; `None` leaves it
    /// to the host of the project's web address.
    pub forge: Option<Forge>,

    /// The project's web address, which the credits link from, without a `/` at
    /// its end; `None` leaves it to git's `origin` remote.
    pub repository: Option<String>,

    /// The heading of each type's section of the release notes, by the type's
    /// id; a type left out keeps its default heading.
    pub headings: BTreeMap<String, String>,
}

/// `jp release`'s options, as the configuration sets them.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ReleaseSection {
    fragments_dir: Option<Utf8PathBuf>,
    forge: Option<String>,
    repository: Option<String>,
    headings: BTreeMap<String, String>,
}

/// `jp wiki`'s options, as the configuration sets them.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct WikiSection {
    dir: Option<Utf8PathBuf>,
    relevant: KeySection,
}

/// `jp rfd`'s options, as the configuration sets them.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RfdSection {
    dir: Option<Utf8PathBuf>,
    categories: Option<Vec<String>>,
    tracking: KeySection,
    relevant: KeySection,
}

/// A `jp rfd` setting naming a label key, of which the site reads the key.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct KeySection {
    key: Option<String>,
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

    Ok(RfdOptions {
        dir: directory(section.dir, DEFAULT_RFD_DIR, "rfd")?,
        categories: categories(section.categories, &DEFAULT_CATEGORIES, "rfd")?,
        tracking_key: section
            .tracking
            .key
            .unwrap_or_else(|| DEFAULT_TRACKING_KEY.to_owned()),
        relevant_key: section
            .relevant
            .key
            .unwrap_or_else(|| DEFAULT_RELEVANT_KEY.to_owned()),
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

/// What the site reads of `jp wiki`'s options, from the whole configuration the
/// host passed.
pub(crate) fn wiki_options(config: &Value) -> Result<WikiOptions, String> {
    let section: WikiSection = section(config, "wiki")?;

    Ok(WikiOptions {
        dir: directory(section.dir, DEFAULT_WIKI_DIR, "wiki")?,
        relevant_key: section
            .relevant
            .key
            .unwrap_or_else(|| DEFAULT_RELEVANT_KEY.to_owned()),
    })
}

/// What the site reads of `jp release`'s options, from the whole configuration
/// the host passed.
///
/// The forge, the web address, and the headings are held to what `jp release`
/// accepts, so a value it refuses never reaches the snapshot.
pub(crate) fn release_options(config: &Value) -> Result<ReleaseOptions, String> {
    let section: ReleaseSection = section(config, "release")?;

    let dir = match section.fragments_dir {
        None => Utf8PathBuf::from(DEFAULT_FRAGMENTS_DIR),
        Some(dir) if dir.as_str().trim().is_empty() => {
            return Err("`plugins.command.release.options.fragments_dir` is empty.".to_owned());
        }
        Some(dir) => dir,
    };

    let forge = section
        .forge
        .map(|forge| forge.parse::<Forge>())
        .transpose()
        .map_err(|error| format!("`plugins.command.release.options.forge`: {error}."))?;

    let repository = match section.repository {
        None => None,
        Some(repository) if is_web_address(&repository) => {
            Some(repository.trim_end_matches('/').to_owned())
        }
        Some(_) => {
            return Err(
                "`plugins.command.release.options.repository` must be an `https://` address."
                    .to_owned(),
            );
        }
    };

    let mut headings = BTreeMap::new();
    for (type_, heading) in &section.headings {
        let type_ = type_.parse::<Type>().map_err(|error: UnknownType| {
            format!("`plugins.command.release.options.headings`: {error}.")
        })?;
        let heading = heading.trim();
        if heading.is_empty() {
            return Err(format!(
                "`plugins.command.release.options.headings.{type_}` is empty."
            ));
        }
        headings.insert(type_.id().to_owned(), heading.to_owned());
    }

    Ok(ReleaseOptions {
        dir,
        forge,
        repository,
        headings,
    })
}

/// A plugin's configured categories, lowercase and each once, or `default`.
fn categories(
    configured: Option<Vec<String>>,
    default: &[&str],
    name: &str,
) -> Result<Vec<String>, String> {
    let Some(configured) = configured else {
        return Ok(default
            .iter()
            .map(|category| (*category).to_owned())
            .collect());
    };

    let mut seen = BTreeSet::new();
    let categories: Vec<String> = configured
        .iter()
        .map(|category| category.trim().to_lowercase())
        .filter(|category| !category.is_empty() && seen.insert(category.clone()))
        .collect();

    if categories.is_empty() {
        return Err(format!(
            "`plugins.command.{name}.options.categories` is empty."
        ));
    }

    Ok(categories)
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

#[cfg(test)]
#[path = "options_tests.rs"]
mod tests;
