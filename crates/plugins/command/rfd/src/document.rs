//! Reading and editing an RFD document: its heading, its metadata block, and
//! the documents it links to.
//!
//! An RFD carries its metadata as a block of `- **Key**: value` lines under the
//! heading rather than as front matter (see RFD 001):
//!
//! ```markdown
//! # RFD 042: Title
//!
//! - **Status**: Accepted
//! - **Category**: Design
//! - **Extends**: [RFD 001]
//! - **Summary**: One sentence, long enough to wrap onto
//!   an indented line.
//! ```
//!
//! The block is read and written the way `doc::metadata` reads and writes every
//! document's; what this module adds is particular to RFDs: the `# RFD 042:`
//! heading, the fields that cite other RFDs, and the retired `Tracking Issue`.
//! [`Header`] reads that block and nothing past it, so a metadata line quoted
//! further down, as RFD 001 does when it documents the format, is never read as
//! the document's own.
//! [`Document`] is the one place that writes it: every command that changes a
//! status, a relationship, or the heading goes through it.

use std::sync::LazyLock;

use doc::metadata;
pub(crate) use doc::metadata::NoMetadata;
use regex::Regex;

/// `RFD 042` or `RFD D05`, the way a metadata field cites another RFD.
static CITATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bRFD\s+([0-9]{3}|D[0-9]{2})\b").expect("valid regex"));

/// A link to an RFD through its file name: `042-slug.md`, `D05-slug`.
///
/// The slug has to carry a letter, so a date like `2026-08-05` isn't read as a
/// link to RFD 2026.
static RFD_LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b([0-9]{3}|D[0-9]{2})-[a-z0-9-]*[a-z][a-z0-9-]*(?:\.md)?").expect("valid regex")
});

/// A ticket reference in its canonical form, `T-02wt0kx`.
static TICKET_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bT-([0-9a-z]{7})\b").expect("valid regex"));

/// The prefix of the line a document's heading is on.
const HEADING: &str = "# RFD ";

/// The text of the document's `# RFD 042: Title` heading.
///
/// The first such heading anywhere in the document counts.
/// `None` when there is none.
#[must_use]
pub(crate) fn title(content: &str) -> Option<String> {
    content.lines().find_map(|line| {
        let (id, title) = line.strip_prefix(HEADING)?.split_once(':')?;

        let digits = id.strip_prefix('D').unwrap_or(id);
        let numbered = !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit());

        (numbered && !title.is_empty()).then(|| title.trim().to_owned())
    })
}

/// The document's sections: what follows its heading and its metadata block,
/// without the blank lines around it.
///
/// A document without a metadata block has everything after its heading.
#[must_use]
pub(crate) fn body(content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let block = metadata::block(&lines);
    let start = if block.is_empty() {
        lines
            .iter()
            .position(|line| line.starts_with(HEADING))
            .map_or(0, |heading| heading + 1)
    } else {
        block.end
    };

    lines[start..].join("\n").trim().to_owned()
}

/// The RFD an entry of a relationship field cites: `[RFD 001][tickets]` cites
/// `001`.
#[must_use]
pub(crate) fn cited(entry: &str) -> Option<String> {
    CITATION
        .captures(entry)
        .map(|captures| captures[1].to_owned())
}

/// A document's metadata block, read.
pub(crate) struct Header<'a, S>(metadata::Header<'a, S>);

impl<'a, S: AsRef<str>> Header<'a, S> {
    /// The metadata of the document `lines` holds.
    #[must_use]
    pub(crate) fn of(lines: &'a [S]) -> Self {
        Self(metadata::Header::of(lines))
    }

    /// The first line's value of the field `key`, trimmed.
    ///
    /// `None` when the block has no such field, or it is empty.
    #[must_use]
    pub(crate) fn field(&self, key: &str) -> Option<String> {
        self.0.field(key)
    }

    /// The value of the field `key` with the lines it wraps onto, as one line.
    #[must_use]
    pub(crate) fn wrapped(&self, key: &str) -> Option<String> {
        self.0.wrapped(key)
    }

    /// The RFDs the field `key` cites, in the order it names them.
    ///
    /// `- **Extends**: [RFD 001], [RFD 100]` cites `001` and `100`.
    #[must_use]
    pub(crate) fn citations(&self, key: &str) -> Vec<String> {
        let Some(value) = self.wrapped(key) else {
            return vec![];
        };

        CITATION
            .captures_iter(&value)
            .map(|captures| captures[1].to_owned())
            .collect()
    }

    /// The status the `Status` field opens with: `Accepted` for `Accepted`, and
    /// for `Accepted (2026-08-05)`.
    #[must_use]
    pub(crate) fn status(&self) -> Option<String> {
        let value = self.field("Status")?;
        let status: String = value
            .chars()
            .take_while(char::is_ascii_alphabetic)
            .collect();

        (!status.is_empty()).then_some(status)
    }

    /// Where the retired `Tracking Issue` field points: the link's target when
    /// it is a link, inline or by reference, and its text otherwise.
    ///
    /// `None` when the document has no such field.
    #[must_use]
    pub(crate) fn tracking_issue(&self) -> Option<String> {
        let value = self.wrapped("Tracking Issue")?;

        if let Some((_, target)) = value.split_once("](") {
            return Some(target.trim_end_matches(')').to_owned());
        }

        let label = value
            .strip_prefix('[')
            .and_then(|rest| rest.split_once(']'));
        let defined = label.and_then(|(label, _)| {
            let definition = format!("[{label}]:");
            self.0.lines().iter().find_map(|line| {
                line.as_ref()
                    .strip_prefix(&definition)
                    .map(|target| target.trim().to_owned())
            })
        });

        Some(defined.unwrap_or(value))
    }

    /// Whether the line at `index` belongs to the field `key`, continuation
    /// lines included.
    #[must_use]
    pub(crate) fn is_field_line(&self, key: &str, index: usize) -> bool {
        self.0.is_field_line(key, index)
    }
}

/// A document being edited.
///
/// Edits touch the heading and the metadata block, and leave every other line
/// as it was.
/// A file read with `\r\n` line endings is written back with them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Document(metadata::Document);

impl Document {
    #[must_use]
    pub(crate) fn parse(text: &str) -> Self {
        Self(metadata::Document::parse(text))
    }

    #[must_use]
    pub(crate) fn render(&self) -> String {
        self.0.render()
    }

    #[must_use]
    pub(crate) fn lines(&self) -> &[String] {
        self.0.lines()
    }

    #[must_use]
    pub(crate) fn header(&self) -> Header<'_, String> {
        Header::of(self.0.lines())
    }

    /// Set the field `key` to `value`, replacing it whole, wrapped lines
    /// included, or adding it above `Summary`, which RFD 001 keeps last, when
    /// the block doesn't carry it.
    pub(crate) fn set(&mut self, key: &str, value: &str) -> Result<(), NoMetadata> {
        self.0.set(key, value)
    }

    /// Remove the field `key`, wrapped lines included.
    /// `false` when the block doesn't carry it.
    pub(crate) fn remove(&mut self, key: &str) -> bool {
        self.0.remove(key)
    }

    /// Add `link`, which cites the RFD `id`, to the relationship field `key`.
    ///
    /// The entry joins the end of the field, after the line its value wraps
    /// onto last; a field the block doesn't carry is added.
    /// `false` when the field already cites `id`, which leaves it as it was.
    pub(crate) fn cite(&mut self, key: &str, id: &str, link: &str) -> Result<bool, NoMetadata> {
        if self.header().citations(key).iter().any(|cited| cited == id) {
            return Ok(false);
        }

        // An empty field has nothing to append to, so it is replaced.
        if self.header().wrapped(key).is_some() {
            self.0.append(key, &format!(", {link}"));
        } else {
            self.set(key, link)?;
        }

        Ok(true)
    }

    /// Drop the entries of the relationship field `key` that cite an RFD `drop`
    /// accepts, removing the field when none is left.
    ///
    /// Entries are separated by a comma and a space.
    /// The entries kept are written back on one line.
    /// `false` when no entry was dropped.
    pub(crate) fn uncite(&mut self, key: &str, drop: impl Fn(&str) -> bool) -> bool {
        let Some(value) = self.header().wrapped(key) else {
            return false;
        };

        let entries: Vec<&str> = value.split(", ").collect();
        let kept: Vec<&str> = entries
            .iter()
            .copied()
            .filter(|entry| !cited(entry).is_some_and(|id| drop(&id)))
            .collect();

        if kept.len() == entries.len() {
            return false;
        }

        if kept.is_empty() {
            self.0.remove(key)
        } else {
            // The field is there, so `set` replaces it rather than needing a
            // block to add it to.
            self.0.set(key, &kept.join(", ")).is_ok()
        }
    }

    /// The index of the document's heading line, `# RFD 042: Title`.
    fn heading(&self) -> Option<usize> {
        self.0
            .lines()
            .iter()
            .position(|line| line.starts_with(HEADING))
    }

    /// Give the heading the id `id`, keeping its title.
    /// `false` when the document has no `# RFD <id>: <title>` heading.
    pub(crate) fn set_heading_id(&mut self, id: &str) -> bool {
        let Some(index) = self.heading() else {
            return false;
        };
        let Some((_, title)) = self.0.lines()[index][HEADING.len()..].split_once(':') else {
            return false;
        };

        let line = format!("{HEADING}{id}:{title}");
        self.0.lines_mut()[index] = line;
        true
    }

    /// Replace the heading with `# RFD <id>: <title>`.
    /// `false` when the document has no heading to replace.
    pub(crate) fn set_heading(&mut self, id: &str, title: &str) -> bool {
        let Some(index) = self.heading() else {
            return false;
        };

        self.0.lines_mut()[index] = format!("{HEADING}{id}: {title}");
        true
    }

    /// Insert `notice` between the metadata and the first section, followed by
    /// a blank line.
    ///
    /// A document without sections gets it at the end.
    pub(crate) fn insert_before_sections(&mut self, notice: &[String]) {
        self.0.insert_before_sections(notice);
    }
}

/// Every document the text links to: RFDs by their id (`042`, `D05`), in the
/// order they first appear, then tickets (`T-02wt0kx`), likewise.
#[must_use]
pub(crate) fn referenced_labels(content: &str) -> Vec<String> {
    let mut labels: Vec<String> = vec![];
    let mut add = |label: String| {
        if !labels.contains(&label) {
            labels.push(label);
        }
    };

    for captures in RFD_LINK.captures_iter(content) {
        add(captures[1].to_owned());
    }
    for captures in TICKET_LINK.captures_iter(content) {
        add(format!("T-{}", &captures[1]));
    }

    labels
}

#[cfg(test)]
#[path = "document_tests.rs"]
mod tests;
