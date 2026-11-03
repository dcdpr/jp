//! A wiki page: a markdown file, `<id>-<slug>.md`, opening with a `# Title`
//! heading and a metadata block.
//!
//! ```markdown
//! # Ubiquitous Language: Conversation
//!
//! - **Status**: Reference
//! - **Authors**: Jean Mertz <git@jeanmertz.com>
//! - **Date**: 2026-10-30
//! - **Parent**: W-0a1b2c3
//! - **Label**: domain=conversation
//! - **Summary**: Canonical definitions for Conversation, Turn, and Thread.
//!
//! ## Turn
//! ```
//!
//! [`Page`] reads a file leniently, keeping each field as written, so a page
//! edited by hand can still be listed and shown; `jp wiki check` is what holds
//! a page to the format.
//! [`render`] writes a new one.

use std::{fmt, str::FromStr};

use camino::Utf8PathBuf;
use doc::{
    id::{Id, Kind},
    metadata::{self, Header},
};
use jp_label::Labels;
use serde::Serialize;

/// The kind of document a [`PageId`] names.
#[derive(Debug)]
pub(crate) enum WikiKind {}

impl Kind for WikiKind {
    const PREFIX: char = 'W';
    const NOUN: &'static str = "wiki page";
    const WRITER: &'static str = "JP-WIKI";
}

/// A wiki page's id, `W-02wt0kx`.
pub(crate) type PageId = Id<WikiKind>;

/// The metadata key one label is written under, once per `key=value` pair.
pub(crate) const LABEL_KEY: &str = "Label";

/// A page's freshness contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) enum Status {
    /// Documentation of the system as it is, kept up to date with it.
    Reference,
    /// Dated evidence: research, analysis, or a study of prior art.
    Snapshot,
    /// Retired, and kept so links to it still resolve.
    Archived,
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Reference => "Reference",
            Self::Snapshot => "Snapshot",
            Self::Archived => "Archived",
        })
    }
}

impl FromStr for Status {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "reference" => Ok(Self::Reference),
            "snapshot" => Ok(Self::Snapshot),
            "archived" => Ok(Self::Archived),
            _ => Err(format!(
                "`{}` is not a status; use Reference, Snapshot, or Archived.",
                s.trim()
            )),
        }
    }
}

/// A page file, its fields kept as written.
#[derive(Debug, Clone)]
pub(crate) struct Page {
    pub id: PageId,

    /// Where the file is, as an absolute path.
    pub path: Utf8PathBuf,

    pub source: String,

    /// The text of the `# Title` heading the file opens with.
    pub title: Option<String>,

    pub status: Option<String>,
    pub authors: Option<String>,
    pub date: Option<String>,
    pub parent: Option<String>,
    pub superseded_by: Option<String>,

    /// Every label line's value, unescaped, empty ones included.
    pub labels: Vec<String>,

    pub summary: Option<String>,
}

impl Page {
    /// Read the page file at `path`, which holds `source`.
    #[must_use]
    pub(crate) fn read(id: PageId, path: Utf8PathBuf, source: String) -> Self {
        let lines: Vec<&str> = source.lines().collect();
        let header = Header::of(&lines);
        let field = |key: &str| header.wrapped(key);

        let title = title(&source);
        let status = field("Status");
        let authors = field("Authors");
        let date = field("Date");
        let parent = field("Parent");
        let superseded_by = field("Superseded by");
        let summary = field("Summary");
        let labels = header
            .values(LABEL_KEY)
            .iter()
            .map(|value| metadata::unescape(value))
            .collect();

        Self {
            id,
            path,
            source,
            title,
            status,
            authors,
            date,
            parent,
            superseded_by,
            labels,
            summary,
        }
    }

    /// The status, when the page carries one this format defines.
    #[must_use]
    pub(crate) fn status(&self) -> Option<Status> {
        self.status.as_deref()?.parse().ok()
    }

    #[must_use]
    pub(crate) fn is_archived(&self) -> bool {
        self.status() == Some(Status::Archived)
    }

    /// The page its `Parent` names, when that is an id.
    #[must_use]
    pub(crate) fn parent_id(&self) -> Option<PageId> {
        self.parent.as_deref()?.parse().ok()
    }

    /// The labels the page carries, those that read as labels.
    #[must_use]
    pub(crate) fn label_set(&self) -> Labels {
        Labels::from_tokens_lossy(&self.labels).0
    }

    /// The title, or the file name for a page without one.
    #[must_use]
    pub(crate) fn name(&self) -> &str {
        self.title
            .as_deref()
            .or_else(|| self.path.file_stem())
            .unwrap_or_default()
    }

    /// What follows the title and the metadata block.
    #[must_use]
    pub(crate) fn body(&self) -> String {
        body(&self.source)
    }

    /// Whether `query` appears in the title, the summary, or the body, ignoring
    /// case.
    #[must_use]
    pub(crate) fn mentions(&self, query: &str) -> bool {
        let query = query.to_lowercase();
        let contains = |text: &str| text.to_lowercase().contains(&query);

        contains(self.name())
            || self.summary.as_deref().is_some_and(contains)
            || contains(&self.body())
    }
}

/// The text of the `# Title` heading a document opens with: its first line that
/// isn't blank, when that is one.
#[must_use]
pub(crate) fn title(source: &str) -> Option<String> {
    source
        .lines()
        .find(|line| !line.trim().is_empty())
        .and_then(|line| line.strip_prefix("# "))
        .map(|title| title.trim().to_owned())
        .filter(|title| !title.is_empty())
}

/// What follows a document's title and metadata block, without the blank lines
/// around it.
#[must_use]
pub(crate) fn body(source: &str) -> String {
    let lines: Vec<&str> = source.lines().collect();

    lines[body_start(&lines)..].join("\n").trim().to_owned()
}

/// The index of the first line after a document's title and metadata block.
#[must_use]
pub(crate) fn body_start<S: AsRef<str>>(lines: &[S]) -> usize {
    let block = metadata::block(lines);
    if !block.is_empty() {
        return block.end;
    }

    lines
        .iter()
        .position(|line| !line.as_ref().trim().is_empty())
        .filter(|&first| lines[first].as_ref().starts_with("# "))
        .map_or(0, |title| title + 1)
}

/// The parts of a page settled when it is created.
pub(crate) struct NewPage<'a> {
    pub title: &'a str,
    pub status: Status,
    pub authors: &'a str,
    pub date: &'a str,
    pub parent: Option<PageId>,
    pub labels: &'a Labels,

    /// The one-sentence summary; empty writes the field without a value, for
    /// `jp wiki check` to ask for one.
    pub summary: &'a str,

    pub body: &'a str,
}

/// The text of a new page.
///
/// `Summary` is written last, where the format keeps it.
#[must_use]
pub(crate) fn render(new: &NewPage<'_>) -> String {
    let mut out = format!("# {}\n\n", new.title.trim());
    let mut field = |key: &str, value: &str| {
        let line = metadata::line(key, value);
        out.push_str(line.trim_end());
        out.push('\n');
    };

    field("Status", &new.status.to_string());
    field("Authors", new.authors);
    field("Date", new.date);
    if let Some(parent) = new.parent {
        field("Parent", &parent.to_string());
    }
    for token in new.labels.to_tokens() {
        field(LABEL_KEY, &token);
    }
    field("Summary", new.summary.trim());

    let body = new.body.trim();
    if !body.is_empty() {
        out.push('\n');
        out.push_str(body);
        out.push('\n');
    }

    out
}

#[cfg(test)]
#[path = "page_tests.rs"]
mod tests;
