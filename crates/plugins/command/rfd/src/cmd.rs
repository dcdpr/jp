//! The commands that change RFDs.
//!
//! Each one checks everything it can before it writes, so a refusal leaves the
//! repository as it was, and reports what it changed as an [`Output`]: one line
//! per change, and warnings for what is left to check by hand.
//!
//! [`Output`]: crate::Output

pub(crate) mod abandon;
pub(crate) mod draft;
pub(crate) mod init;
pub(crate) mod promote;
pub(crate) mod rank;
pub(crate) mod relate;
pub(crate) mod rename;
pub(crate) mod renumber;

use camino::{Utf8Path, Utf8PathBuf};

use crate::{
    id::{self, RfdId},
    paths::Paths,
    references,
};

/// Whether `id` names a draft.
fn is_draft(id: &str) -> bool {
    id.starts_with('D')
}

/// A file name's stem: `042-slug` for `042-slug.md`.
fn stem(name: &str) -> &str {
    name.strip_suffix(".md").unwrap_or(name)
}

/// The id a citation names, when it names one.
fn parse(id: &str) -> Option<RfdId> {
    id.parse().ok()
}

/// Check that `title` can head an RFD and give its file a name.
fn check_title(title: &str) -> Result<(), String> {
    if title.is_empty() {
        return Err("Give the RFD a title.".to_owned());
    }
    if title.contains(['\n', '\r']) {
        return Err("A title is one line.".to_owned());
    }
    if id::slug(title).is_empty() {
        return Err(format!("Title '{title}' produces an empty filename slug."));
    }

    Ok(())
}

/// `design` as the metadata writes it: `Design`.
fn capitalized(category: &str) -> String {
    let mut chars = category.chars();
    match chars.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

/// A warning naming files, one per line under `heading`.
fn listed(heading: &str, paths: &Paths, files: &[Utf8PathBuf]) -> String {
    let mut text = heading.to_owned();
    for file in files {
        text.push_str(&format!("\n  {}", paths.show(file)));
    }

    text
}

/// What a rewrite left behind in the rest of the workspace.
struct Leftovers<'a> {
    /// The directories the rewrite covered, which aren't searched.
    covered: &'a [&'a Utf8Path],

    /// Where the search looked, for the warning: `outside docs/`.
    scope: String,

    /// What a file found still does: `still name 042-x.md`.
    finding: String,
}

impl Leftovers<'_> {
    /// A warning naming the files outside the covered directories whose text
    /// `matches` accepts, if any.
    ///
    /// When `scan` is false the workspace is not read, and the warning says
    /// nothing outside the covered directories was checked.
    fn report(&self, paths: &Paths, scan: bool, matches: impl Fn(&str) -> bool) -> Option<String> {
        if !scan {
            return Some(format!(
                "References {} were not checked: the access policy doesn't allow reading the \
                 workspace.",
                self.scope
            ));
        }

        let found = references::mentions(&paths.root, self.covered, matches);
        (!found.is_empty()).then(|| {
            listed(
                &format!("Warning: references {} {}:", self.scope, self.finding),
                paths,
                &found,
            )
        })
    }
}

/// `'a', 'b', or 'c'`, for a message listing what a value can be.
fn alternatives(values: &[String]) -> String {
    let quoted: Vec<String> = values.iter().map(|value| format!("'{value}'")).collect();

    match quoted.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [first, second] => format!("{first} or {second}"),
        [rest @ .., last] => format!("{}, or {last}", rest.join(", ")),
    }
}

#[cfg(test)]
#[path = "cmd_tests.rs"]
mod tests;
