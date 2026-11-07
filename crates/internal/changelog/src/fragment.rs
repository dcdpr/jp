//! One changelog fragment: the entry recording a change for the people using
//! the software.
//!
//! A fragment is a Markdown file named `<slug>.<type>.md`, such as
//! `retry-idle-turns.fixed.md`: the name holds the change's [`Type`]
//! ([`name`]), and the text is the entry the release notes list:
//!
//! ```markdown
//! A turn that stalls while idle is retried instead of failing.
//! ```
//!
//! A fragment can open with a metadata block whose `Credits` fields replace the
//! credits history names, one per line, each a change request or a commit's
//! hash:
//!
//! ```markdown
//! - **Credits**: #1207
//!
//! A turn that stalls while idle is retried instead of failing.
//! ```
//!
//! A change that requires users to act carries its section of the release's
//! upgrade guide, an [`Upgrade`]: a paragraph that starts with `BREAKING
//! CHANGE:` or `BREAKING-CHANGE:`, the rest of which is the section's title,
//! and every paragraph after it, which are its body.
//! A line that starts with either token anywhere else, inside a code block or
//! partway through a paragraph, is an error rather than text.
//! That keeps a plain search for the token in step with [`parse`]: a fragment
//! the search finds has an upgrade section, or fails to parse.
//!
//! The entry and the upgrade section are each given with their links inline,
//! `[text](url)`, so each stands alone wherever it is placed: a reference-style
//! link reads its URL from a definition anywhere in the file, which a formatter
//! puts at its end, below the upgrade section.
//!
//! Paragraphs and code blocks are read as GitHub renders Markdown.

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

/// The extension a fragment's file name ends in.
const EXTENSION: &str = ".md";

/// What a change means for the people using the software.
///
/// The ids are fixed, so a fragment means the same in every project.
/// Types order as release notes show them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Type {
    /// A new capability the user can now reach.
    Added,
    /// A change to existing behavior.
    Changed,
    /// Something still working but slated for removal.
    Deprecated,
    /// Something taken away.
    Removed,
    /// A bug fix the user would notice.
    Fixed,
    /// A fix with security relevance.
    Security,
    /// A user-visible performance improvement.
    Performance,
}

impl Type {
    /// Every type, in the order release notes show them.
    pub const ALL: [Self; 7] = [
        Self::Added,
        Self::Changed,
        Self::Deprecated,
        Self::Removed,
        Self::Fixed,
        Self::Security,
        Self::Performance,
    ];

    /// The id a fragment's file name holds the type by, such as `added`.
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

    /// What the type is used for, in one sentence.
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

    /// Read a type from its id, written as a file name writes it.
    fn from_str(id: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|type_| type_.id() == id)
            .ok_or_else(|| UnknownType(id.to_owned()))
    }
}

/// A string that is none of the seven type ids.
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

/// What a file name is, when it isn't a fragment's: `<slug>.<type>.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameError {
    /// The name doesn't end in `.md`, or nothing comes before its type.
    NotAFragment,

    /// Nothing but a slug comes before `.md`.
    MissingType { slug: String },

    /// What comes between the slug and `.md` is none of the seven type ids.
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

/// The slug and the type the file name `name` holds: `retry-idle-turns` and
/// [`Type::Fixed`] for `retry-idle-turns.fixed.md`.
///
/// The slug is everything before the type, dots included.
///
/// # Errors
///
/// Returns why `name` isn't a fragment's file name.
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

/// The file name of the fragment of type `type_` named `slug`:
/// `<slug>.<type>.md`.
#[must_use]
pub fn file_name(slug: &str, type_: Type) -> String {
    format!("{slug}.{type_}{EXTENSION}")
}

/// Every type's id, as an error lists them.
fn ids() -> String {
    let ids: Vec<&str> = Type::ALL.iter().map(|type_| type_.id()).collect();

    ids.join(", ")
}

/// A fragment, as [`parse`] reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fragment {
    /// What the change means for users, which the file name holds.
    pub type_: Type,

    /// The credits to give in place of the ones history names, when the file
    /// lists them.
    pub credits: Option<Vec<ListedCredit>>,

    /// The entry the release notes list, as Markdown: the body before its
    /// upgrade section, or all of it.
    ///
    /// Its links are inline, `[text](url)`, whichever form the file wrote them
    /// in.
    pub entry: String,

    /// The fragment's section of the upgrade guide, for a change that requires
    /// users to act.
    pub upgrade: Option<Upgrade>,
}

/// A fragment's section of a release's upgrade guide.
///
/// Its links are inline, `[text](url)`, whichever form the file wrote them in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upgrade {
    /// What broke: the rest of the paragraph the token opens, on one line.
    pub title: String,

    /// What changed, why, and how to upgrade, as Markdown: every paragraph
    /// after the title.
    pub body: String,
}

/// A document that doesn't hold a well-formed fragment.
///
/// Lines count from 1 at the top of the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FragmentError {
    /// The metadata block lists a type, which the file name holds.
    TypeField { line: usize },

    /// The metadata block lists a field other than `Credits`.
    UnknownField { line: usize, key: String },

    /// A `Credits` field names neither a change request nor a commit.
    InvalidCredit { line: usize, value: String },

    /// Nothing but whitespace, or the metadata block, is in the file.
    EmptyBody,

    /// A line inside a code block starts with an upgrade section's token.
    MarkerInCodeBlock { line: usize, marker: &'static str },

    /// A line starts with an upgrade section's token without starting a
    /// paragraph.
    MarkerOutsideParagraph { line: usize, marker: &'static str },

    /// A second paragraph opens an upgrade section.
    SecondUpgradeSection { line: usize },

    /// Nothing comes before the upgrade section.
    EmptyEntry { line: usize, marker: &'static str },

    /// The paragraph opening the upgrade section holds nothing but its token.
    EmptyUpgradeTitle { line: usize, marker: &'static str },

    /// Nothing follows the upgrade section's title.
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

/// Read `source` as a fragment of type `type_`, which its file name holds, held
/// to every rule of the format.
///
/// Lines may end in `\n` or `\r\n`, and a leading byte order mark is skipped.
///
/// # Errors
///
/// Returns the first rule `source` breaks, reading from the top.
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

/// The credits a fragment's metadata block lists, which takes no field but
/// `Credits`; `None` when it lists none.
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

/// The paragraph that opens a fragment's upgrade section.
struct Opening {
    /// Its lines, as 0-based indices into the body's.
    lines: Range<usize>,

    /// The number of its first line in the file.
    line: usize,

    /// The token it starts with.
    marker: &'static str,
}

/// The entry and the upgrade section of a fragment's `body`, whose first line
/// is line `first_line` of the file.
///
/// The rules are checked against the file as written, so a line an error names
/// is the file's; each part then reads its links' URLs from the whole file.
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

/// The paragraph opening the upgrade section of `body`, split into `lines`, if
/// it has one.
///
/// Every line starting with a token has to open a paragraph, and only one may.
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

/// The entry before `opening` in `body`, and the upgrade section it opens, with
/// the links `links` found in `body` inline.
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

/// The title `paragraph` gives an upgrade section: the paragraph after its
/// `marker`, its lines joined into one.
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

/// Every line in `lines` that starts with an upgrade section's token, as its
/// 0-based index and the token.
fn marked_lines(lines: &[&str]) -> Vec<(usize, &'static str)> {
    lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| document::marker(line).map(|marker| (index, marker)))
        .collect()
}

/// Where a body's paragraphs and code blocks sit, as ranges of 0-based line
/// indices.
struct Blocks {
    /// The paragraphs at the top level of the body, outside any list or quote.
    paragraphs: Vec<Range<usize>>,

    /// The code blocks, fenced or indented, wherever they sit.
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

    /// Whether the line at `index` is inside a code block.
    fn in_code(&self, index: usize) -> bool {
        self.code.iter().any(|block| block.contains(&index))
    }

    /// The lines of the paragraph starting at `index`, if one does.
    fn paragraph_at(&self, index: usize) -> Option<Range<usize>> {
        self.paragraphs
            .iter()
            .find(|paragraph| paragraph.start == index)
            .cloned()
    }
}

/// The 0-based indices of the lines a block spans, from comrak's 1-based line
/// numbers.
fn indices(sourcepos: &Sourcepos) -> Range<usize> {
    sourcepos.start.line.saturating_sub(1)..sourcepos.end.line
}

#[cfg(test)]
#[path = "fragment_tests.rs"]
mod tests;
