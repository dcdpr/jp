//! Parsing and validation for individual changelog fragments.
//!
//! A fragment is a Markdown file named `<slug>.<type>.md` (e.g.
//! `retry-idle-turns.fixed.md`).
//! The file name encodes the change [`Type`] ([`name`]), and the body contains
//! the release-note entry:
//!
//! ```markdown
//! A turn that stalls while idle is retried instead of failing.
//! ```
//!
//! A fragment may start with a metadata block containing `- **Credits**:`
//! fields (one change request or commit hash per line) to override git history
//! attribution:
//!
//! ```markdown
//! - **Credits**: #1207
//!
//! A turn that stalls while idle is retried instead of failing.
//! ```
//!
//! Breaking changes include an [`Upgrade`] section: a top-level paragraph
//! starting with `BREAKING CHANGE:` or `BREAKING-CHANGE:` (whose remainder
//! forms the section title), followed by one or more body paragraphs explaining
//! how to upgrade.
//! Any line starting with either marker inside a code block or mid-paragraph is
//! rejected so that a plain-text search for the marker always agrees with
//! [`parse`].
//!
//! Reference-style links (`[text][label]`) in both the entry and the upgrade
//! section are rewritten to inline links (`[text](url)`) so each section can be
//! rendered standalone even when link definitions sit at the bottom of the
//! file.

use std::{fmt, ops::Range, str::FromStr};

use comrak::{
    Arena,
    nodes::{NodeValue, Sourcepos},
    parse_document,
};

use crate::{
    document::{self, CREDITS, Field, ListedCredit},
    links::{Links, line_starts},
};

const EXTENSION: &str = ".md";

/// Category of a user-visible change, ordered as sections appear in release
/// notes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Type {
    /// A new user-facing capability.
    Added,
    /// A change to existing behavior.
    Changed,
    /// A feature still working but slated for removal.
    Deprecated,
    /// A removed feature or option.
    Removed,
    /// A user-visible bug fix.
    Fixed,
    /// A security-relevant fix.
    Security,
    /// A user-visible performance improvement.
    Performance,
}

impl Type {
    /// All fragment types in release-note section order.
    pub const ALL: [Self; 7] = [
        Self::Added,
        Self::Changed,
        Self::Deprecated,
        Self::Removed,
        Self::Fixed,
        Self::Security,
        Self::Performance,
    ];

    /// Identifier used in fragment file names (`<slug>.<type>.md`), such as
    /// `added`.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Changed => "changed",
            Self::Deprecated => "deprecated",
            Self::Removed => "removed",
            Self::Fixed => "fixed",
            Self::Security => "security",
            Self::Performance => "performance",
        }
    }

    /// One-sentence description of what this change type covers.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Added => "A new capability the user can now reach.",
            Self::Changed => "A change to existing behavior.",
            Self::Deprecated => "Something still working but slated for removal.",
            Self::Removed => "Something taken away.",
            Self::Fixed => "A bug fix the user would notice.",
            Self::Security => "A fix with security relevance.",
            Self::Performance => "A user-visible performance improvement.",
        }
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

impl FromStr for Type {
    type Err = UnknownType;

    fn from_str(id: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|type_| type_.id() == id)
            .ok_or_else(|| UnknownType(id.to_owned()))
    }
}

/// Error returned when parsing an unrecognized [`Type`] identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownType(pub String);

impl fmt::Display for UnknownType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ids: Vec<&str> = Type::ALL.iter().map(|type_| type_.id()).collect();

        write!(
            f,
            "`{}` is not a changelog type; use one of: {}",
            self.0,
            ids.join(", ")
        )
    }
}

impl std::error::Error for UnknownType {}

/// Error returned by [`name`] when a file name does not match
/// `<slug>.<type>.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameError {
    /// File name does not end with `.md` or has an empty slug.
    NotAFragment,

    /// File name has no `<type>` segment before `.md`.
    MissingType { slug: String },

    /// Segment before `.md` is not a valid [`Type`] identifier.
    UnknownType { type_: String },
}

impl fmt::Display for NameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAFragment => f.write_str(
                "not a fragment, which is named `<slug>.<type>.md`; rename it, or move it out of \
                 the fragments directory",
            ),
            Self::MissingType { slug } => write!(
                f,
                "no type in the name: name it `{slug}.<type>.md`, such as `{slug}.fixed.md`, with \
                 one of: {}",
                ids()
            ),
            Self::UnknownType { type_ } => write!(
                f,
                "`{type_}` is not a changelog type: the part of the name before `.md` is one of: \
                 {}",
                ids()
            ),
        }
    }
}

impl std::error::Error for NameError {}

/// Parse a fragment file name (`<slug>.<type>.md`) into its slug and [`Type`].
///
/// The slug is everything before `.<type>.md`, including any dots.
///
/// # Errors
///
/// Returns [`NameError`] if `name` does not match `<slug>.<type>.md` with a
/// valid [`Type`].
pub fn name(name: &str) -> Result<(&str, Type), NameError> {
    let stem = name
        .strip_suffix(EXTENSION)
        .filter(|stem| !stem.is_empty())
        .ok_or(NameError::NotAFragment)?;
    let Some((slug, type_)) = stem.rsplit_once('.') else {
        return Err(NameError::MissingType {
            slug: stem.to_owned(),
        });
    };
    if slug.is_empty() {
        return Err(NameError::NotAFragment);
    }

    let type_ = type_
        .parse()
        .map_err(|UnknownType(type_)| NameError::UnknownType { type_ })?;

    Ok((slug, type_))
}

/// Format the fragment file name `<slug>.<type>.md`.
#[must_use]
pub fn file_name(slug: &str, type_: Type) -> String {
    format!("{slug}.{type_}{EXTENSION}")
}

fn ids() -> String {
    let ids: Vec<&str> = Type::ALL.iter().map(|type_| type_.id()).collect();

    ids.join(", ")
}

/// Parsed changelog fragment returned by [`parse`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fragment {
    /// Change category parsed from the file name.
    pub type_: Type,

    /// Explicit `- **Credits**:` overrides from the metadata block, if present.
    pub credits: Option<Vec<ListedCredit>>,

    /// Release-note entry as Markdown (everything before the upgrade section),
    /// with reference links rewritten to inline `[text](url)` links.
    pub entry: String,

    /// Upgrade guide section for breaking changes, if present.
    pub upgrade: Option<Upgrade>,
}

/// Upgrade guide section parsed from a fragment, with reference links rewritten
/// to inline `[text](url)` links.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upgrade {
    /// Single-line section title following the `BREAKING CHANGE:` marker.
    pub title: String,

    /// Markdown paragraphs following the title paragraph.
    pub body: String,
}

/// Validation error returned by [`parse`].
/// Line numbers are 1-based from the start of the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FragmentError {
    /// Metadata block contains a `Type` field, which belongs in the file name.
    TypeField { line: usize },

    /// Metadata block contains a field other than `Credits`.
    UnknownField { line: usize, key: String },

    /// A `Credits` field value is neither a change request nor a commit hash.
    InvalidCredit { line: usize, value: String },

    /// File contains no entry text outside whitespace and metadata.
    EmptyBody,

    /// A line inside a code block starts with an upgrade section marker.
    MarkerInCodeBlock { line: usize, marker: &'static str },

    /// A line starts with an upgrade section marker without opening a top-level
    /// paragraph.
    MarkerOutsideParagraph { line: usize, marker: &'static str },

    /// More than one paragraph starts with an upgrade section marker.
    SecondUpgradeSection { line: usize },

    /// No entry text precedes the upgrade section.
    EmptyEntry { line: usize, marker: &'static str },

    /// The upgrade section's opening paragraph contains only the marker.
    EmptyUpgradeTitle { line: usize, marker: &'static str },

    /// No body paragraphs follow the upgrade section's title paragraph.
    EmptyUpgradeBody { line: usize },
}

impl fmt::Display for FragmentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TypeField { line } => write!(
                f,
                "line {line} names a type, which a fragment's file name holds: `<slug>.<type>.md`"
            ),
            Self::UnknownField { line, key } => write!(
                f,
                "line {line} lists `{key}`, and the only field a fragment takes is `{CREDITS}`"
            ),
            Self::InvalidCredit { line, value } => {
                f.write_str(&document::invalid_credit(*line, value))
            }
            Self::EmptyBody => f.write_str("no entry: say what changed for users"),
            Self::MarkerInCodeBlock { line, marker } => write!(
                f,
                "line {line} starts with `{marker}` inside a code block; to show the token, \
                 indent the line or put the token in inline code"
            ),
            Self::MarkerOutsideParagraph { line, marker } => write!(
                f,
                "line {line} starts with `{marker}` without starting a paragraph; an upgrade \
                 section opens a paragraph of its own, after a blank line, and to show the token \
                 instead, indent the line or put the token in inline code"
            ),
            Self::SecondUpgradeSection { line } => write!(
                f,
                "line {line} opens a second upgrade section, and a fragment has at most one"
            ),
            Self::EmptyEntry { line, marker } => write!(
                f,
                "no entry before the upgrade section on line {line}: say what changed for users \
                 before `{marker}`"
            ),
            Self::EmptyUpgradeTitle { line, marker } => write!(
                f,
                "the upgrade section on line {line} has no title: name what broke after `{marker}`"
            ),
            Self::EmptyUpgradeBody { line } => write!(
                f,
                "the upgrade section on line {line} has no body: say what changed and how to \
                 upgrade in the paragraphs after its title"
            ),
        }
    }
}

impl std::error::Error for FragmentError {}

/// Parse and validate a changelog fragment of category `type_`.
///
/// Normalizes `\r\n` line endings and strips a leading UTF-8 BOM.
///
/// # Errors
///
/// Returns the first [`FragmentError`] encountered in `source`.
pub fn parse(type_: Type, source: &str) -> Result<Fragment, FragmentError> {
    let source = document::normalize(source);
    let lines: Vec<&str> = source.lines().collect();
    let first = lines
        .iter()
        .position(|line| !line.trim().is_empty())
        .unwrap_or(lines.len());

    let (fields, end) = document::fields(&lines, first);
    let credits = credits(&fields)?;
    let offset = line_starts(&source)
        .get(end)
        .copied()
        .unwrap_or(source.len());
    let (entry, upgrade) = body(&source[offset..], end + 1)?;

    Ok(Fragment {
        type_,
        credits,
        entry,
        upgrade,
    })
}

fn credits(fields: &[Field]) -> Result<Option<Vec<ListedCredit>>, FragmentError> {
    let mut credits = vec![];
    for field in fields {
        if field.key.eq_ignore_ascii_case(CREDITS) {
            let credit = document::credit(field).ok_or_else(|| FragmentError::InvalidCredit {
                line: field.line,
                value: field.value.clone(),
            })?;
            credits.push(credit);
        } else if field.key.eq_ignore_ascii_case("type") {
            return Err(FragmentError::TypeField { line: field.line });
        } else {
            return Err(FragmentError::UnknownField {
                line: field.line,
                key: field.key.clone(),
            });
        }
    }

    Ok((!credits.is_empty()).then_some(credits))
}

/// Paragraph that opens a fragment's upgrade section.
struct Opening {
    /// 0-based line range within `body`.
    lines: Range<usize>,

    /// 1-based line number in the full file.
    line: usize,

    marker: &'static str,
}

/// Parse the entry and optional upgrade section from `body` (whose first line
/// is 1-based `first_line` in the full file), then inline any reference links.
fn body(body: &str, first_line: usize) -> Result<(String, Option<Upgrade>), FragmentError> {
    if body.trim().is_empty() {
        return Err(FragmentError::EmptyBody);
    }

    let lines: Vec<&str> = body.lines().collect();
    let opening = opening(body, &lines, first_line)?;
    let links = Links::read(body);
    let Some(opening) = opening else {
        let entry = links.standalone(body, 0..body.len()).trim().to_owned();
        if entry.is_empty() {
            return Err(FragmentError::EmptyBody);
        }
        return Ok((entry, None));
    };
    let (entry, upgrade) = upgrade(body, &links, opening)?;

    Ok((entry, Some(upgrade)))
}

/// Locate the paragraph opening the upgrade section in `body`, if any.
///
/// Rejects markers inside code blocks, markers that do not start a top-level
/// paragraph, and multiple upgrade sections.
fn opening(
    body: &str,
    lines: &[&str],
    first_line: usize,
) -> Result<Option<Opening>, FragmentError> {
    let marked = marked_lines(lines);
    if marked.is_empty() {
        return Ok(None);
    }

    let blocks = Blocks::read(body);
    let mut opening = None;
    for (index, marker) in marked {
        let line = first_line + index;
        if blocks.in_code(index) {
            return Err(FragmentError::MarkerInCodeBlock { line, marker });
        }
        let Some(paragraph) = blocks.paragraph_at(index) else {
            return Err(FragmentError::MarkerOutsideParagraph { line, marker });
        };
        if opening.is_some() {
            return Err(FragmentError::SecondUpgradeSection { line });
        }
        opening = Some(Opening {
            lines: paragraph,
            line,
            marker,
        });
    }

    Ok(opening)
}

fn upgrade(
    body: &str,
    links: &Links,
    opening: Opening,
) -> Result<(String, Upgrade), FragmentError> {
    let Opening {
        lines: paragraph,
        line,
        marker,
    } = opening;

    let starts = line_starts(body);
    let offset = |index: usize| starts.get(index).copied().unwrap_or(body.len());
    let (title_start, rest_start) = (offset(paragraph.start), offset(paragraph.end));

    let entry = links.standalone(body, 0..title_start).trim().to_owned();
    if entry.is_empty() {
        return Err(FragmentError::EmptyEntry { line, marker });
    }

    let title = title(&links.standalone(body, title_start..rest_start), marker);
    if title.is_empty() {
        return Err(FragmentError::EmptyUpgradeTitle { line, marker });
    }

    let rest = links
        .standalone(body, rest_start..body.len())
        .trim()
        .to_owned();
    if rest.is_empty() {
        return Err(FragmentError::EmptyUpgradeBody { line });
    }

    Ok((entry, Upgrade { title, body: rest }))
}

/// Strip `marker` from the opening paragraph and collapse wrapped lines into a
/// single-line title.
fn title(paragraph: &str, marker: &str) -> String {
    let mut parts = vec![];
    for (index, line) in paragraph.lines().enumerate() {
        let line = if index == 0 {
            line.strip_prefix(marker).unwrap_or(line)
        } else {
            line
        };
        let line = line.trim();
        if !line.is_empty() {
            parts.push(line);
        }
    }

    parts.join(" ")
}

fn marked_lines(lines: &[&str]) -> Vec<(usize, &'static str)> {
    lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| document::marker(line).map(|marker| (index, marker)))
        .collect()
}

/// Line ranges (0-based) of top-level paragraphs and code blocks in a fragment
/// body.
struct Blocks {
    paragraphs: Vec<Range<usize>>,
    code: Vec<Range<usize>>,
}

impl Blocks {
    fn read(body: &str) -> Self {
        let arena = Arena::new();
        let root = parse_document(&arena, body, &document::markdown_options());

        let paragraphs = root
            .children()
            .filter_map(|node| {
                let data = node.data();
                matches!(data.value, NodeValue::Paragraph).then(|| indices(&data.sourcepos))
            })
            .collect();
        let code = root
            .descendants()
            .filter_map(|node| {
                let data = node.data();
                matches!(data.value, NodeValue::CodeBlock(_)).then(|| indices(&data.sourcepos))
            })
            .collect();

        Self { paragraphs, code }
    }

    fn in_code(&self, index: usize) -> bool {
        self.code.iter().any(|block| block.contains(&index))
    }

    fn paragraph_at(&self, index: usize) -> Option<Range<usize>> {
        self.paragraphs
            .iter()
            .find(|paragraph| paragraph.start == index)
            .cloned()
    }
}

/// Convert comrak's 1-based inclusive line numbers to a 0-based `Range<usize>`.
fn indices(sourcepos: &Sourcepos) -> Range<usize> {
    sourcepos.start.line.saturating_sub(1)..sourcepos.end.line
}

#[cfg(test)]
#[path = "fragment_tests.rs"]
mod tests;
