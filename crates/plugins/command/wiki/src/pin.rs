//! Permalinks: links to a file as a commit on the default branch holds it.
//!
//! A relative link follows the work tree, which is what a Reference page wants
//! and what a Snapshot must not do.
//! [`pin`] rewrites a page's relative source links, the ones leaving the
//! documentation tree, into permalinks under the repository's web address:
//!
//! ```text
//! https://github.com/dcdpr/jp/blob/<commit>/crates/plugins/command/rfd/src/links.rs#L88-L125
//! ```
//!
//! Each is pinned to the newest default-branch commit whose copy of the file
//! matches the work tree; a link with no such commit, or to a file the
//! command's access doesn't let it read, is left alone and reported.
//! [`permalink`] reads one back, for `check`.

use std::{collections::BTreeMap, ops::Range};

use camino::Utf8Path;
use comrak::{Arena, nodes::NodeValue, parse_document};
use jp_tool::Capability;

use crate::{
    access::Access,
    git::{self, DEFAULT_BRANCH},
    links::{self, has_scheme, line_range, parse_options, percent_decode, resolve, split_fragment},
    options::Paths,
};

/// A link to a file at a commit, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Permalink {
    /// The full hash of the commit.
    pub commit: String,

    /// The file's path from the top of the repository.
    pub path: String,

    pub fragment: Option<String>,
}

/// `url` read as a permalink under `repository`, naming a full commit hash:
/// `<repository>/blob/<commit>/<path>`, with `tree` for a directory.
///
/// `None` for any other address, which is an ordinary external link.
#[must_use]
pub(crate) fn permalink(url: &str, repository: &str) -> Option<Permalink> {
    let rest = url.strip_prefix(repository)?.strip_prefix('/')?;
    let (kind, rest) = rest.split_once('/')?;
    if !matches!(kind, "blob" | "tree") {
        return None;
    }

    let (commit, rest) = rest.split_once('/')?;
    let full = matches!(commit.len(), 40 | 64) && commit.bytes().all(|b| b.is_ascii_hexdigit());
    if !full {
        return None;
    }

    let (path, fragment) = split_fragment(rest);
    Some(Permalink {
        commit: commit.to_ascii_lowercase(),
        path: percent_decode(path).trim_end_matches('/').to_owned(),
        fragment: fragment.map(ToOwned::to_owned),
    })
}

/// What [`pin`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Pinned {
    /// Each destination rewritten, and what it became.
    pub pinned: Vec<(String, String)>,

    /// Each source link left as it was, and why.
    pub left: Vec<(String, String)>,
}

/// `source`, the page at `page`, with its relative source links pinned to the
/// commits under `repository` that hold what the work tree holds.
///
/// Only files `access` lets the command read are pinned: telling which commit
/// holds a file means reading it.
#[must_use]
pub(crate) fn pin(
    source: &str,
    page: &Utf8Path,
    paths: &Paths,
    repository: Option<&str>,
    access: &Access,
) -> (String, Pinned) {
    let mut report = Pinned::default();
    let dir = page.parent().unwrap_or(&paths.root);
    let docs = paths.docs();

    let mut rewrites: BTreeMap<String, String> = BTreeMap::new();
    let mut context: Option<Result<(String, String, String), String>> = None;
    for link in links::links(source) {
        if link.image || has_scheme(&link.url) || link.url.starts_with(['/', '#']) {
            continue;
        }
        let (path, fragment) = split_fragment(&link.url);
        let target = resolve(dir, path);
        if !target.starts_with(&paths.root) || target.starts_with(&docs) {
            continue;
        }
        if rewrites.contains_key(&link.url) {
            continue;
        }
        if let Err(refusal) = access.check(Capability::Read, &target) {
            report.left.push((link.url.clone(), refusal));
            continue;
        }

        // Read once, and only for a page that cites source at all.
        let ready = context
            .get_or_insert_with(|| setting(&paths.root, repository))
            .clone();
        let pinned = ready.and_then(|(repository, prefix, head)| {
            let relative = paths.show(&target);
            let commit = commit_for(&paths.root, &target, &relative, &prefix, &head)?;
            let kind = if target.is_dir() { "tree" } else { "blob" };
            let anchor = fragment.map_or(String::new(), normalized_fragment);

            Ok(format!(
                "{repository}/{kind}/{commit}/{prefix}{relative}{anchor}"
            ))
        });

        match pinned {
            Ok(url) => {
                rewrites.insert(link.url.clone(), url);
            }
            Err(why) => report.left.push((link.url.clone(), why)),
        }
    }

    let mut text = source.to_owned();
    for (old, new) in rewrites {
        let (rewritten, count) = replace_destination(&text, &old, &new);
        if count == 0 {
            report.left.push((
                old,
                "its destination isn't written in a form this can rewrite".to_owned(),
            ));
            continue;
        }
        text = rewritten;
        report.pinned.push((old, new));
    }

    (text, report)
}

/// What every pin in a workspace needs: the repository's address, the path from
/// the top of the repository to the workspace root, and the commit the default
/// branch is at.
fn setting(root: &Utf8Path, repository: Option<&str>) -> Result<(String, String, String), String> {
    let repository = repository.ok_or(
        "there is no repository address to pin to: set `plugins.command.vite.options.repository`, \
         or give git an `origin` remote",
    )?;
    let prefix = git::prefix(root).ok_or("the workspace isn't in a git repository")?;
    let head = git::resolve(root, DEFAULT_BRANCH).ok_or(
        "git knows no default branch (`origin/HEAD`); `git remote set-head origin --auto` sets it",
    )?;

    Ok((repository.to_owned(), prefix, head))
}

/// The newest commit on the default branch holding `target` as the work tree
/// does.
///
/// `relative` is its path from the workspace root and `prefix` the path from
/// the top of the repository to that root.
fn commit_for(
    root: &Utf8Path,
    target: &Utf8Path,
    relative: &str,
    prefix: &str,
    head: &str,
) -> Result<String, String> {
    if target.is_dir() {
        return if git::unchanged_since(root, head, relative) {
            Ok(head.to_owned())
        } else {
            Err(
                "the directory differs from the default branch; commit and merge it first"
                    .to_owned(),
            )
        };
    }
    if !target.is_file() {
        return Err("there is no such file".to_owned());
    }

    let held = git::hash_object(root, relative).ok_or("git can't read the file")?;
    let blob_at = |commit: &str| git::resolve(root, &format!("{commit}:{prefix}{relative}"));
    if blob_at(head).as_deref() == Some(held.as_str()) {
        return Ok(head.to_owned());
    }

    let changes = git::changes(root, DEFAULT_BRANCH, relative);
    for (index, commit) in changes.iter().enumerate() {
        if blob_at(commit).as_deref() != Some(held.as_str()) {
            continue;
        }

        // The copy this commit wrote lasted until the next change; the commit
        // just before that one is the newest still holding it.
        if let Some(newer) = index.checked_sub(1).map(|newer| &changes[newer])
            && let Some(before) = git::resolve(root, &format!("{newer}^"))
            && blob_at(&before).as_deref() == Some(held.as_str())
        {
            return Ok(before);
        }
        return Ok(commit.clone());
    }

    Err(
        "no commit on the default branch holds the file as the work tree does; land the change \
         first"
            .to_owned(),
    )
}

/// A fragment as a permalink writes it: `#L12-L24` for any spelling of a line
/// range, `#L12` for one line, and any other fragment as it was.
fn normalized_fragment(fragment: &str) -> String {
    match line_range(fragment) {
        Some((start, end)) if start == end => format!("#L{start}"),
        Some((start, end)) => format!("#L{start}-L{end}"),
        None => format!("#{fragment}"),
    }
}

/// `source` with the link destination `old` replaced by `new` wherever a link
/// or a definition writes it outside a code block, and how many places that
/// was.
fn replace_destination(source: &str, old: &str, new: &str) -> (String, usize) {
    let fenced = code_lines(source);
    let mut count = 0;

    let lines: Vec<String> = source
        .split_inclusive('\n')
        .enumerate()
        .map(|(index, line)| {
            if fenced.iter().any(|range| range.contains(&index)) {
                return line.to_owned();
            }

            let mut line = line.to_owned();
            for (from, to) in [
                (format!("](<{old}>)"), format!("](<{new}>)")),
                (format!("]({old})"), format!("]({new})")),
                (format!("]({old} "), format!("]({new} ")),
            ] {
                count += line.matches(&from).count();
                line = line.replace(&from, &to);
            }

            if let Some(rewritten) = replace_definition(&line, old, new) {
                count += 1;
                line = rewritten;
            }

            line
        })
        .collect();

    (lines.concat(), count)
}

/// `line` with the destination of the reference definition it holds replaced,
/// when it is a definition of `old`.
fn replace_definition(line: &str, old: &str, new: &str) -> Option<String> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 || !line[indent..].starts_with('[') {
        return None;
    }

    let colon = line.find("]:")? + 2;
    let after = &line[colon..];
    let space = after.len() - after.trim_start().len();
    let rest = after.trim_start().strip_prefix(old)?;
    if !(rest.is_empty() || rest.starts_with(char::is_whitespace)) {
        return None;
    }

    Some(format!("{}{new}{rest}", &line[..colon + space]))
}

/// The 0-based lines of `source` inside code blocks.
fn code_lines(source: &str) -> Vec<Range<usize>> {
    let arena = Arena::new();
    let root = parse_document(&arena, source, &parse_options());

    root.descendants()
        .filter(|node| matches!(node.data().value, NodeValue::CodeBlock(_)))
        .map(|node| {
            let position = node.data().sourcepos;
            position.start.line.saturating_sub(1)..position.end.line
        })
        .collect()
}

#[cfg(test)]
#[path = "pin_tests.rs"]
mod tests;
