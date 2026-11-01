//! Transclusion: a section of another document, or lines of a source file,
//! shown where a page links to it with an image that stands alone in its
//! paragraph.
//!
//! ```markdown
//! ![Pinned Conversation](0a1b2c3-ubiquitous-language.md#pinned-conversation)
//!
//! ![Links::by_rfd](../../crates/plugins/command/rfd/src/links.rs#L88-L125)
//! ```
//!
//! An embedded section is read as part of its whole source document: its
//! reference-style links resolve against that document's definitions, its
//! relative links are rewritten to work from the embedding page, and a
//! `#fragment` pointing into it keeps pointing at the source.
//! Its headings move to sit below the heading the embed appears under, and an
//! embed inside it is expanded from where it now appears.
//! Each expansion is wrapped in markers naming its source, so a reader can tell
//! embedded text from the page's own:
//!
//! ```markdown
//! <!-- embed W-0a1b2c3#pinned-conversation -->
//! ...
//! <!-- /embed -->
//! ```
//!
//! Every file an embed reads is asked of the command's access, nested embeds
//! included: reading a page grants nothing about what it embeds.
//! An embed of a file the access doesn't let the command read is replaced by a
//! marker naming it and the refusal, and the rest of the text is expanded.

use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use doc::heading::headings;
use jp_tool::Capability;

use crate::{
    access::Access,
    library::Library,
    links::{self, Link, has_scheme, line_range, line_starts, resolve, split_fragment},
    section::{self, Section},
};

/// What an embed shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Shown {
    /// The section of a markdown document with this anchor.
    Section(String),

    /// These 1-based lines of a file, first to last.
    Lines(usize, usize),
}

/// An embed a document carries.
#[derive(Debug, Clone)]
pub(crate) struct Embed {
    pub link: Link,

    /// The file it shows part of.
    pub target: Utf8PathBuf,

    pub shown: Shown,
}

/// Why an embed wasn't expanded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Problem {
    /// Expanding it would expand itself again, through the embeds named in
    /// order.
    Loop(Vec<String>),

    /// Its target couldn't be read or doesn't hold what it names.
    Unresolved(String),

    /// The command's access doesn't let it read the target: the embed's
    /// destination, and the refusal.
    Denied { url: String, refusal: String },
}

/// Every embed in `source`, a document in `dir`.
///
/// An image counts when it stands alone in its paragraph and points at a
/// section of a markdown file (`.md#section`) or a line range of any other
/// (`#L12-L24`).
#[must_use]
pub(crate) fn embeds(source: &str, dir: &Utf8Path) -> Vec<Embed> {
    links::links(source)
        .into_iter()
        .filter(|link| link.image && link.standalone.is_some())
        .filter(|link| !has_scheme(&link.url) && !link.url.starts_with(['/', '#']))
        .filter_map(|link| {
            let (path, fragment) = split_fragment(&link.url);
            let fragment = fragment.filter(|fragment| !fragment.is_empty())?;
            let target = resolve(dir, path);
            let shown = if target.extension() == Some("md") {
                Shown::Section(fragment.to_owned())
            } else {
                let (start, end) = line_range(fragment)?;
                Shown::Lines(start, end)
            };

            Some(Embed {
                link,
                target,
                shown,
            })
        })
        .collect()
}

/// Expands the embeds in a document.
pub(crate) struct Expander<'a> {
    /// The workspace root, which no embed reaches outside of.
    pub root: &'a Utf8Path,

    /// The wiki directory, whose pages an embed marker names by id.
    pub wiki: &'a Utf8Path,

    pub library: &'a Library,

    /// What the command may read.
    pub access: &'a Access,

    /// What couldn't be expanded, in the order it was met.
    pub problems: Vec<Problem>,
}

impl<'a> Expander<'a> {
    #[must_use]
    pub(crate) fn new(
        root: &'a Utf8Path,
        wiki: &'a Utf8Path,
        library: &'a Library,
        access: &'a Access,
    ) -> Self {
        Self {
            root,
            wiki,
            library,
            access,
            problems: vec![],
        }
    }

    /// `text`, part of the document at `path`, with its embeds expanded.
    ///
    /// `within` is the section of that document `text` is, when it is one, so
    /// an embed of that section from inside it is read as the loop it is.
    pub(crate) fn expand(&mut self, text: &str, path: &Utf8Path, within: Option<&str>) -> String {
        let mut stack = vec![(path.to_path_buf(), within.unwrap_or_default().to_owned())];

        self.expand_in(text, path, &mut stack)
    }

    /// `text`, which reads as if it were in the document at `path`, with its
    /// embeds expanded; `stack` holds the sections being expanded around it.
    fn expand_in(
        &mut self,
        text: &str,
        path: &Utf8Path,
        stack: &mut Vec<(Utf8PathBuf, String)>,
    ) -> String {
        let dir = path.parent().unwrap_or(self.root);
        let found = embeds(text, dir);
        if found.is_empty() {
            return text.to_owned();
        }

        let lines: Vec<&str> = text.lines().collect();
        let titles = headings(text);
        let mut out: Vec<String> = vec![];
        let mut at = 0;
        for embed in found {
            let Some(paragraph) = embed.link.standalone.clone() else {
                continue;
            };
            let enclosing = titles
                .iter()
                .rfind(|heading| heading.line < paragraph.start)
                .map_or(0, |heading| heading.level);

            out.extend(
                lines[at..paragraph.start]
                    .iter()
                    .map(|line| (*line).to_owned()),
            );
            match self.render(&embed, text, path, enclosing, stack) {
                Some(expanded) => out.push(expanded),
                None => out.extend(
                    lines[paragraph.clone()]
                        .iter()
                        .map(|line| (*line).to_owned()),
                ),
            }
            at = paragraph.end;
        }
        out.extend(lines[at..].iter().map(|line| (*line).to_owned()));

        out.join("\n")
    }

    /// What `embed`, in `text`, expands to, or `None` when it can't be.
    fn render(
        &mut self,
        embed: &Embed,
        text: &str,
        path: &Utf8Path,
        enclosing: u8,
        stack: &mut Vec<(Utf8PathBuf, String)>,
    ) -> Option<String> {
        let name = self.name(&embed.target);
        if !embed.target.starts_with(self.root) {
            self.problems.push(Problem::Unresolved(format!(
                "{} points outside the workspace.",
                embed.link.url
            )));
            return None;
        }

        if let Err(refusal) = self.access.check(Capability::Read, &embed.target) {
            let shown = match &embed.shown {
                Shown::Section(anchor) => format!("#{anchor}"),
                Shown::Lines(start, end) => format!("#L{start}-L{end}"),
            };
            let marker = format!("<!-- embed {name}{shown}: not shown ({refusal}) -->");
            self.problems.push(Problem::Denied {
                url: embed.link.url.clone(),
                refusal,
            });
            return Some(marker);
        }

        let source = match fs::read_to_string(&embed.target) {
            Ok(source) => source,
            Err(error) => {
                self.problems.push(Problem::Unresolved(format!(
                    "{} can't be read: {error}.",
                    embed.link.url
                )));
                return None;
            }
        };

        match &embed.shown {
            Shown::Lines(start, end) => {
                let label = embed.link.text.clone().map_or("", |range| &text[range]);
                self.lines(&source, embed, &name, label, (*start, *end))
            }
            Shown::Section(anchor) => {
                let section = match section::find(&source, anchor) {
                    Ok(section) => section,
                    Err(error) => {
                        self.problems
                            .push(Problem::Unresolved(format!("{}: {error}", embed.link.url)));
                        return None;
                    }
                };

                let key = (embed.target.clone(), section.heading.slug.clone());
                if let Some(first) = stack.iter().position(|entry| *entry == key) {
                    let mut chain: Vec<String> = stack[first..]
                        .iter()
                        .map(|(path, slug)| format!("{}#{slug}", self.name(path)))
                        .collect();
                    chain.push(format!("{name}#{}", section.heading.slug));
                    self.problems.push(Problem::Loop(chain));
                    return None;
                }

                let dir = path.parent().unwrap_or(self.root);
                let rebased = rebase_section(&source, &section, &embed.target, dir, enclosing);

                stack.push(key);
                let nested = self.expand_in(&rebased, path, stack);
                stack.pop();

                Some(format!(
                    "<!-- embed {name}#{} -->\n{nested}\n<!-- /embed -->",
                    section.heading.slug
                ))
            }
        }
    }

    /// Lines `start` to `end` of `source`, the file `embed` points at, as a
    /// fenced code block under a link to them.
    fn lines(
        &mut self,
        source: &str,
        embed: &Embed,
        name: &str,
        label: &str,
        (start, end): (usize, usize),
    ) -> Option<String> {
        let lines: Vec<&str> = source.lines().collect();
        if start == 0 || end < start || end > lines.len() {
            self.problems.push(Problem::Unresolved(format!(
                "{} names lines {start} to {end}, and the file has {}.",
                embed.link.url,
                lines.len()
            )));
            return None;
        }

        let excerpt = lines[start - 1..end].join("\n");
        let longest = longest_run(&excerpt, '`');
        let fence = "`".repeat(longest.max(2) + 1);
        let language = embed.target.extension().unwrap_or_default();
        let (path, _) = split_fragment(&embed.link.url);

        Some(format!(
            "<!-- embed {name}#L{start}-L{end} \
             -->\n[{label}]({path}#L{start}-L{end})\n\n{fence}{language}\n{excerpt}\n{fence}\n<!\
             -- /embed -->"
        ))
    }

    /// How a marker names the file at `path`: by id for a wiki page, RFD, or
    /// ticket, and by its path below the workspace root otherwise.
    fn name(&self, path: &Utf8Path) -> String {
        self.library.name_of(path, self.wiki).unwrap_or_else(|| {
            path.strip_prefix(self.root)
                .unwrap_or(path)
                .as_str()
                .replace('\\', "/")
        })
    }
}

/// The text of `section` of `source`, the document at `from`, made to read the
/// same from a document in `to`, under a heading of level `enclosing`.
///
/// Every link that doesn't name a scheme is rewritten inline, its destination
/// rebased: a reference-style one too, since the definitions it resolves
/// against stay behind.
#[must_use]
pub(crate) fn rebase_section(
    source: &str,
    section: &Section,
    from: &Utf8Path,
    to: &Utf8Path,
    enclosing: u8,
) -> String {
    let starts = line_starts(source);
    let begin = starts[section.lines.start];
    let end = starts
        .get(section.lines.end)
        .copied()
        .unwrap_or(source.len());

    let mut edits: Vec<(usize, usize, String)> = vec![];
    for link in links::links(source) {
        if link.span.start < begin || link.span.end > end {
            continue;
        }
        // An image inside a link's text goes with the link.
        if edits
            .last()
            .is_some_and(|(_, last_end, _)| link.span.start < *last_end)
        {
            continue;
        }

        let url = rebase(&link.url, from, to);
        if url == link.url && link.inline {
            continue;
        }
        let label = link.text.clone().map_or("", |range| &source[range]);
        edits.push((link.span.start, link.span.end, inline(&link, label, &url)));
    }

    let mut text = source[begin..end].to_owned();
    for (start, finish, replacement) in edits.into_iter().rev() {
        text.replace_range(start - begin..finish - begin, &replacement);
    }

    shift_headings(&text, source, section, enclosing)
}

/// `text`, the section of `source` with its links rebased, with its headings
/// moved so the first sits one level below `enclosing`.
fn shift_headings(text: &str, source: &str, section: &Section, enclosing: u8) -> String {
    let shift = i16::from(enclosing) + 1 - i16::from(section.heading.level);
    let mut lines: Vec<String> = text.lines().map(ToOwned::to_owned).collect();

    for heading in headings(source) {
        if !section.lines.contains(&heading.line) {
            continue;
        }
        let Some(line) = lines.get_mut(heading.line - section.lines.start) else {
            continue;
        };
        let indent = line.len() - line.trim_start_matches(' ').len();
        let hashes = longest_prefix(&line[indent..], '#');
        if indent > 3 || hashes == 0 {
            // An underlined heading keeps its level: it has no `#` to change.
            continue;
        }

        let level = (i16::from(heading.level) + shift).clamp(1, 6);
        let level = usize::try_from(level).unwrap_or(1);
        line.replace_range(indent..indent + hashes, &"#".repeat(level));
    }

    lines.join("\n").trim().to_owned()
}

/// `url`, a destination in the document at `from`, as it reads from a document
/// in the directory `to`.
///
/// A destination naming a scheme, or the site's root, reads the same anywhere.
#[must_use]
pub(crate) fn rebase(url: &str, from: &Utf8Path, to: &Utf8Path) -> String {
    if has_scheme(url) || url.starts_with('/') {
        return url.to_owned();
    }

    let (path, fragment) = split_fragment(url);
    let target = if path.is_empty() {
        from.to_path_buf()
    } else {
        resolve(from.parent().unwrap_or(from), path)
    };

    let mut rebased = relative(to, &target);
    if let Some(fragment) = fragment {
        rebased.push('#');
        rebased.push_str(fragment);
    }

    rebased
}

/// The path from the directory `from` to `to`, as a link writes it.
#[must_use]
pub(crate) fn relative(from: &Utf8Path, to: &Utf8Path) -> String {
    let dir = to.parent().unwrap_or(to);
    let name = to.file_name().unwrap_or_default();

    doc::references::link(from, dir, name)
}

/// `link`, with the text `label`, written inline with the destination `url`.
fn inline(link: &Link, label: &str, url: &str) -> String {
    let bang = if link.image { "!" } else { "" };
    let url = if url.contains(|c: char| c.is_whitespace() || c == '(' || c == ')') {
        format!("<{url}>")
    } else {
        url.to_owned()
    };
    let title = if link.title.is_empty() {
        String::new()
    } else {
        format!(" \"{}\"", link.title.replace('"', "\\\""))
    };

    format!("{bang}[{label}]({url}{title})")
}

/// How many `c` open `text`.
fn longest_prefix(text: &str, c: char) -> usize {
    text.chars().take_while(|found| *found == c).count()
}

/// The longest run of `c` anywhere in `text`.
fn longest_run(text: &str, c: char) -> usize {
    let mut longest = 0;
    let mut current = 0;
    for found in text.chars() {
        current = if found == c { current + 1 } else { 0 };
        longest = longest.max(current);
    }

    longest
}

#[cfg(test)]
#[path = "embed_tests.rs"]
mod tests;
