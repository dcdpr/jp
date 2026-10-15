//! Reading an RFD document: its heading, its metadata header, and the documents
//! it links to.
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
//! [`Header`] reads that block and nothing past it, so a metadata line quoted
//! further down, as RFD 001 does when it documents the format, is never read as
//! the document's own.

use std::sync::LazyLock;

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

/// The text of the document's `# RFD 042: Title` heading.
///
/// The first such heading anywhere in the document counts.
/// `None` when there is none.
#[must_use]
pub(crate) fn title(content: &str) -> Option<String> {
    content.lines().find_map(|line| {
        let (id, title) = line.strip_prefix("# RFD ")?.split_once(':')?;

        let digits = id.strip_prefix('D').unwrap_or(id);
        let numbered = !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit());

        (numbered && !title.is_empty()).then(|| title.trim().to_owned())
    })
}

/// A document's metadata block.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Header<'a> {
    lines: &'a [&'a str],
}

impl<'a> Header<'a> {
    /// The block in `lines`: the run of lines from the first `- **` line to the
    /// blank line that closes it.
    ///
    /// A section heading (`## Summary`) before any metadata line means the
    /// document has no block.
    #[must_use]
    pub(crate) fn of(lines: &'a [&'a str]) -> Self {
        let mut start = None;
        for (index, line) in lines.iter().enumerate() {
            if line.starts_with("## ") {
                break;
            }
            if line.starts_with("- **") {
                start = Some(index);
                break;
            }
        }

        let Some(start) = start else {
            return Self { lines: &[] };
        };

        let end = lines[start..]
            .iter()
            .position(|line| line.trim().is_empty())
            .map_or(lines.len(), |offset| start + offset);

        Self {
            lines: &lines[start..end],
        }
    }

    /// The first line's value of the field `key`, trimmed.
    ///
    /// `None` when the block has no such field, or it is empty.
    #[must_use]
    pub(crate) fn field(&self, key: &str) -> Option<String> {
        let index = self.position(key)?;
        let value = value_of(self.lines[index], key)?.trim();

        (!value.is_empty()).then(|| value.to_owned())
    }

    /// The value of the field `key` with the lines it wraps onto, as one line.
    ///
    /// A value long enough to wrap continues on lines indented past the list
    /// marker; those are joined back with single spaces.
    #[must_use]
    pub(crate) fn wrapped(&self, key: &str) -> Option<String> {
        let index = self.position(key)?;
        let first = value_of(self.lines[index], key)?;

        let mut value = first.to_owned();
        for line in &self.lines[index + 1..] {
            if !line.starts_with(' ') || line.trim().is_empty() {
                break;
            }
            value.push(' ');
            value.push_str(line);
        }

        let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
        (!value.is_empty()).then_some(value)
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

    fn position(&self, key: &str) -> Option<usize> {
        self.lines
            .iter()
            .position(|line| value_of(line, key).is_some())
    }
}

/// What follows `- **key**:` on `line`, or `None` when the line is not that
/// field.
fn value_of<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.strip_prefix("- **")?
        .strip_prefix(key)?
        .strip_prefix("**:")
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
