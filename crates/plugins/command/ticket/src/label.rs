//! What the label changes do to the labels a ticket carries.
//!
//! Each change takes the labels a ticket carries and the labels a command
//! named, and gives the whole set the ticket should end up with, as tokens for
//! [`ticket::store::relabel`] to hold to the board's vocabulary.
//!
//! A named label matches a carried one without case, the way the board reads
//! labels: `key=value` matches that label, and a bare `key` every label under
//! the key.

use ticket::Labels;

/// `current`, with `named` added.
pub(crate) fn add(current: &Labels, named: &[String]) -> Vec<String> {
    let mut tokens = current.to_tokens();
    tokens.extend(named.iter().cloned());

    tokens
}

/// `current`, with what each key in `named` holds replaced by the values named
/// for it; every other key keeps its labels.
pub(crate) fn set(current: &Labels, named: &[String]) -> Vec<String> {
    let mut tokens: Vec<String> = current
        .to_tokens()
        .into_iter()
        .filter(|token| {
            !named
                .iter()
                .any(|name| key(name).eq_ignore_ascii_case(key(token)))
        })
        .collect();
    tokens.extend(named.iter().cloned());

    tokens
}

/// `current`, without the labels `named` matches.
pub(crate) fn remove(current: &Labels, named: &[String]) -> Vec<String> {
    current
        .to_tokens()
        .into_iter()
        .filter(|token| !named.iter().any(|name| matches(name, token)))
        .collect()
}

/// What a relabel did to one ticket, in words: what it removed and added, the
/// labels of `adding` the ticket already carried, and those of `removing` it
/// didn't.
pub(crate) fn describe(
    before: &Labels,
    after: &Labels,
    adding: &[String],
    removing: &[String],
) -> String {
    let (removed, added) = difference(before, after);
    let already = carried(before, adding);
    let present = carried(before, removing);
    let missing: Vec<&str> = removing
        .iter()
        .map(String::as_str)
        .filter(|name| !present.contains(name))
        .collect();

    let mut parts = vec![];
    if !removed.is_empty() {
        parts.push(format!("removed {}", removed.join(", ")));
    }
    if !added.is_empty() {
        parts.push(format!("added {}", added.join(", ")));
    }
    if !already.is_empty() {
        parts.push(format!("already carries {}", already.join(", ")));
    }
    if !missing.is_empty() {
        parts.push(format!("doesn't carry {}", missing.join(", ")));
    }

    if parts.is_empty() {
        "unchanged".to_owned()
    } else {
        parts.join("; ")
    }
}

/// The labels of `named` that `current` carries.
fn carried<'a>(current: &Labels, named: &'a [String]) -> Vec<&'a str> {
    let tokens = current.to_tokens();

    named
        .iter()
        .filter(|name| tokens.iter().any(|token| matches(name, token)))
        .map(String::as_str)
        .collect()
}

/// What a relabel took off a ticket and put on it, as tokens: `(removed,
/// added)`.
pub(crate) fn difference(before: &Labels, after: &Labels) -> (Vec<String>, Vec<String>) {
    let before = before.to_tokens();
    let after = after.to_tokens();

    let removed = before
        .iter()
        .filter(|token| !after.contains(token))
        .cloned()
        .collect();
    let added = after
        .iter()
        .filter(|token| !before.contains(token))
        .cloned()
        .collect();

    (removed, added)
}

/// Whether `name`, a label as a command names it, matches `token`, one a ticket
/// carries.
fn matches(name: &str, token: &str) -> bool {
    key(name).eq_ignore_ascii_case(key(token))
        && value(name)
            .is_none_or(|named| value(token).is_some_and(|v| v.eq_ignore_ascii_case(named)))
}

/// The key of a `key=value` token, or the whole of a bare one.
fn key(token: &str) -> &str {
    token.split_once('=').map_or(token, |(key, _)| key).trim()
}

/// The value of a `key=value` token; `None` for a bare one.
fn value(token: &str) -> Option<&str> {
    token.split_once('=').map(|(_, value)| value.trim())
}

#[cfg(test)]
#[path = "label_tests.rs"]
mod tests;
