//! Naming the root of an absolute path instead of printing it.
//!
//! Reports made by these tools get pasted into issues, and they are full of
//! paths from elsewhere: DWARF holds the source of every symbolicated frame as
//! an absolute path on the machine that did the build, dhat does the same, and
//! anything quoted from a subprocess's stderr names whatever that subprocess
//! was looking at.
//! None of it helps a reader, and all of it is somebody's filesystem layout.
//!
//! Replacing the prefix with the variable that names it keeps the path
//! actionable rather than merely censored: `$CARGO_HOME/registry/…` says
//! exactly where to look without saying whose machine it is.
//! A path inside the repository needs no variable at all — relative is both
//! shorter and what these reports have always printed.
//!
//! Two entry points, for the two shapes this comes in.
//! [`shorten`] takes a value already known to be a path.
//! [`shorten_within`] takes prose with paths somewhere inside it, which is what
//! a quoted stderr or a rendered stack frame is.

use camino::{Utf8Path, Utf8PathBuf};

/// A prefix worth hiding, and what to show in its place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shortening {
    /// Absolute prefix, with any trailing separator removed.
    prefix: Utf8PathBuf,

    /// What replaces it.
    ///
    /// Empty shows the remainder on its own, which is what a path inside the
    /// repository wants.
    label: String,
}

impl Shortening {
    /// A shortening for `prefix`, or `None` when there is nothing usable to
    /// match on.
    ///
    /// A prefix that trims to nothing would match every absolute path and
    /// rewrite the lot, so an unset or root-valued variable is dropped rather
    /// than applied.
    fn new(prefix: &str, label: &str) -> Option<Self> {
        let trimmed = prefix.trim_end_matches('/');
        if trimmed.is_empty() {
            return None;
        }

        Some(Self {
            prefix: Utf8PathBuf::from(trimmed),
            label: label.to_owned(),
        })
    }
}

/// The prefixes worth hiding, read from the environment.
pub fn shortenings(root: &Utf8Path) -> Vec<Shortening> {
    let home = std::env::var("HOME").ok();
    let cargo = std::env::var("CARGO_HOME").ok();
    let rustup = std::env::var("RUSTUP_HOME").ok();

    shortenings_from(root, home.as_deref(), cargo.as_deref(), rustup.as_deref())
}

/// The prefixes worth hiding, given where things live.
///
/// `CARGO_HOME` and `RUSTUP_HOME` fall back to their documented defaults under
/// the home directory, and are labelled by variable name either way: the name
/// is what tells a reader where to look, whether or not the variable happens to
/// be set on the machine that produced the report.
pub fn shortenings_from(
    root: &Utf8Path,
    home: Option<&str>,
    cargo: Option<&str>,
    rustup: Option<&str>,
) -> Vec<Shortening> {
    let under_home = |explicit: Option<&str>, default: &str| -> Option<String> {
        explicit
            .map(str::to_owned)
            .or_else(|| home.map(|home| format!("{}/{default}", home.trim_end_matches('/'))))
    };

    let mut out: Vec<Shortening> = Vec::new();
    out.extend(Shortening::new(root.as_str(), ""));

    if let Some(path) = under_home(cargo, ".cargo") {
        out.extend(Shortening::new(&path, "$CARGO_HOME"));
    }
    if let Some(path) = under_home(rustup, ".rustup") {
        out.extend(Shortening::new(&path, "$RUSTUP_HOME"));
    }
    if let Some(home) = home {
        out.extend(Shortening::new(home, "$HOME"));
    }

    // Longest first, so a registry path under the home directory is reported as
    // living under `$CARGO_HOME` rather than under `$HOME`.
    out.sort_by_key(|shortening| std::cmp::Reverse(shortening.prefix.as_str().len()));
    out
}

/// `path` with the first matching prefix replaced by the name for it.
///
/// A path under none of them is returned unchanged.
/// That covers what rustc already remapped (`/rustc/<hash>/…`), which names no
/// machine, and the SDK paths under `/Applications`, which name nobody.
pub fn shorten(path: &str, shortenings: &[Shortening]) -> String {
    for shortening in shortenings {
        let Some(rest) = strip(path, shortening.prefix.as_str()) else {
            continue;
        };

        if shortening.label.is_empty() {
            return if rest.is_empty() {
                ".".to_owned()
            } else {
                rest.to_owned()
            };
        }

        return if rest.is_empty() {
            shortening.label.clone()
        } else {
            format!("{}/{rest}", shortening.label)
        };
    }

    path.to_owned()
}

/// Every path inside `text`, with its prefix replaced by the name for it.
///
/// For a report rather than a path: a quoted stderr line, a dhat frame carrying
/// a source location, a trace event's fields.
/// Applied to a whole rendered report it catches every path in it at once,
/// which is the point — there is no enumerating the places a subprocess might
/// name a file.
pub fn shorten_within(text: &str, shortenings: &[Shortening]) -> String {
    let mut out = text.to_owned();

    for shortening in shortenings {
        let label = if shortening.label.is_empty() {
            // A repository path becomes relative, and mid-prose that means the
            // separator after it has to go too.
            String::new()
        } else {
            shortening.label.clone()
        };

        out = replace_at_boundaries(&out, shortening.prefix.as_str(), &label);
    }

    out
}

/// Whether `c` can continue a path component, and so cannot sit against the
/// edge of a match.
///
/// `/` is not one: a match starting right after a separator starts at a
/// component of its own, which is what makes `file:///Users/jean` shorten while
/// `/Volumes/Backup/Users/jean` does not.
fn continues_component(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '-' | '_' | '.')
}

/// Replace every occurrence of `prefix` in `text` that stands on component
/// boundaries at both ends.
///
/// Both ends are checked, and for the same reason.
/// Without the trailing test `/Users/jean` rewrites the front of
/// `/Users/jeanne`; without the leading one it rewrites the middle of
/// `/Volumes/Backup/Users/jean`, which is a different file on a mounted disk.
fn replace_at_boundaries(text: &str, prefix: &str, label: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(at) = rest.find(prefix) {
        let after = &rest[at + prefix.len()..];

        // What precedes the match is the tail of this window, or of what has
        // already been written when the match sits at the front of it.
        let before = if at > 0 {
            rest[..at].chars().last()
        } else {
            out.chars().last()
        };

        let boundary = before.is_none_or(|c| !continues_component(c))
            && after.chars().next().is_none_or(|c| !continues_component(c));

        out.push_str(&rest[..at]);
        if !boundary {
            out.push_str(prefix);
            rest = after;
            continue;
        }

        // A repository path has no label, so the `/` that followed the prefix
        // has to go with it or the remainder still reads as absolute. The root
        // on its own becomes `.`, because dropping it entirely would leave a
        // field with no value at all.
        if label.is_empty() {
            if let Some(tail) = after.strip_prefix('/') {
                rest = tail;
            } else {
                out.push('.');
                rest = after;
            }
            continue;
        }

        out.push_str(label);
        rest = after;
    }

    out.push_str(rest);
    out
}

/// What follows `prefix` in `path`, when `prefix` covers a whole leading run of
/// components.
///
/// The boundary check is the point: a plain string prefix would rewrite
/// `/Users/jeanne/src` against a home of `/Users/jean`.
fn strip<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = path.strip_prefix(prefix)?;
    if rest.is_empty() {
        return Some(rest);
    }

    rest.strip_prefix('/')
}

#[cfg(test)]
#[path = "paths_tests.rs"]
mod tests;
