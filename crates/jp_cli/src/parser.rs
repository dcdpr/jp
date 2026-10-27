use std::{convert::Infallible, fmt, str::FromStr};

use camino::{FromPathBufError, Utf8Path, Utf8PathBuf, absolute_utf8};
use clap::{ArgMatches, error::ErrorKind};
use clean_path::Clean as _;
use jp_id::Parts;
use relative_path::RelativePathBuf;
use tracing::trace;
use url::Url;

use crate::error::{Error, Result};

/// The clap id of `jp query`'s positional query words.
pub(crate) const QUERY_ARG: &str = "query";

/// The clap indices of the query words, or none when the command takes no query
/// text.
pub(crate) fn query_indices(matches: &ArgMatches) -> Vec<usize> {
    // `indices_of` panics on an id the command does not define, and most
    // commands share their flags with `jp query` without its query words.
    if matches.try_get_raw(QUERY_ARG).is_err() {
        return vec![];
    }

    matches
        .indices_of(QUERY_ARG)
        .map(Iterator::collect)
        .unwrap_or_default()
}

/// A flag with an optional value that, written directly before the query, may
/// instead be the query's first word.
///
/// `jp q --fork 2 hello` forks keeping two turns and asks "hello", while `jp q
/// --fork hello there` forks keeping every turn and asks "hello there".
/// The flag and the query words each read [`given_back`] for the same type, so
/// they agree on which word went where.
pub(crate) trait GivesBack {
    /// The flag's clap argument id.
    const ARG: &'static str;

    /// Whether `value` is meant for the flag.
    ///
    /// Only asked about a non-empty value written directly before the query;
    /// any other value belongs to the flag regardless.
    fn claims(value: &str) -> bool;
}

/// Each value of `F` that `F` gives back to the query, with its clap index.
///
/// A value is given back when it is written directly before a query word and
/// `F` does not claim it.
/// The flag then behaves as if it were written without a value.
/// A command without query text never gives anything back.
pub(crate) fn given_back<F: GivesBack>(matches: &ArgMatches) -> Vec<(usize, String)> {
    let query = query_indices(matches);
    if query.is_empty() {
        return vec![];
    }

    let Ok(Some(values)) = matches.try_get_raw(F::ARG) else {
        return vec![];
    };
    let Some(indices) = matches.indices_of(F::ARG) else {
        return vec![];
    };

    values
        .zip(indices)
        .filter(|(_, index)| query.contains(&(index + 1)))
        .filter_map(|(value, index)| Some((index, value.to_str()?.to_owned())))
        .filter(|(_, value)| !value.is_empty() && !F::claims(value))
        .collect()
}

/// Split a comma-separated flag value into its items.
///
/// Space around an item is ignored, so `"read, write"` and `"read,write"` name
/// the same two items.
/// `item` names what is being parsed ("tool name", "label") and appears in the
/// error message.
///
/// Only for values whose items cannot themselves contain a comma.
/// Prefer clap's `value_delimiter(',')` unless the flag also gives the empty
/// value a meaning of its own, which the delimiter would make unreachable.
///
/// # Errors
///
/// Returns an error if an item is empty or fails to parse.
pub(crate) fn split_list<T>(value: &str, item: &str) -> std::result::Result<Vec<T>, clap::Error>
where
    T: FromStr,
    T::Err: fmt::Display,
{
    value
        .split(',')
        .map(|part| match part.trim() {
            "" => Err(clap::Error::raw(
                ErrorKind::InvalidValue,
                format!("empty {item} in '{value}'\n"),
            )),
            part => part.parse().map_err(|error| {
                clap::Error::raw(
                    ErrorKind::InvalidValue,
                    format!("invalid {item} '{part}': {error}\n"),
                )
            }),
        })
        .collect()
}

#[derive(Debug, Clone)]
pub(crate) enum AttachmentUrlOrPath {
    Url(Url),
    Path(RelativePathBuf),
}

impl AttachmentUrlOrPath {
    pub fn parse(&self, root: Option<&Utf8Path>) -> Result<Url> {
        let path = match &self {
            AttachmentUrlOrPath::Url(url) => return Ok(url.clone()),
            AttachmentUrlOrPath::Path(path) => path,
        };

        // Special case for file attachments
        trace!(
            path = path.as_str(),
            "URI is not a valid URL, treating as file path."
        );
        let (path, exclude) = match path.as_str().strip_prefix('!') {
            Some(path) => (path, "?exclude=true"),
            None => (path.as_str(), ""),
        };

        // Check if the path falls within the workspace.
        //
        // If `root` is `None`, then we allow absolute paths, otherwise we
        // assume the context is a workspace and we only allow relative paths.
        let mut path = Utf8PathBuf::from(path);
        if let Some(root) = root {
            if path.is_relative() {
                path = absolute_utf8(&path).map_err(|error| {
                    Error::Attachment(format!(
                        "Attachment path {path} is relative, but the current directory could not \
                         be determined: {error}",
                    ))
                })?;
            }

            if !path.exists() {
                return Err(Error::Attachment(format!(
                    "Attachment path does not exist: {path}",
                )));
            }

            let p: Utf8PathBuf = path
                .as_std_path()
                .clean()
                .try_into()
                .map_err(FromPathBufError::into_io_error)?;

            let Ok(p) = p.strip_prefix(root) else {
                return Err(Error::Attachment(format!(
                    "Attachment path must be relative to the workspace: {path}",
                )));
            };

            path = p.to_path_buf();
        } else if !path.exists() {
            return Err(Error::Attachment(format!(
                "Attachment path does not exist: {path}",
            )));
        }

        Url::parse(&format!("file:{path}{exclude}")).map_err(Into::into)
    }
}

impl std::str::FromStr for AttachmentUrlOrPath {
    type Err = Infallible;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        // 1. Honour anything that already looks like a URL.
        if let Ok(url) = Url::parse(s) {
            return Ok(Self::Url(url));
        }

        // 2. JP ID shorthand: `jp-c1234`, `jp-c1234?a:-1`, `jp-c1234?raw`,
        //    `jp-c1234?select=a:-1&raw=all`. The bare suffix after `?` is
        //    treated as the value of `select=` unless it already names a
        //    known parameter (`select` or `raw`).
        if let Some(url) = jp_id_shorthand_to_url(s) {
            return Ok(Self::Url(url));
        }

        // 3. Fall back to a relative file path.
        Ok(Self::Path(RelativePathBuf::from(s)))
    }
}

/// Rewrite a JP ID shorthand into a `jp://` URL.
///
/// Returns `None` if `s` doesn't parse as a valid JP ID.
/// A directory literally named `jp-c1234` would still match — prefix it with
/// `./` to force the path interpretation.
fn jp_id_shorthand_to_url(s: &str) -> Option<Url> {
    let (id_part, query) = s
        .find('?')
        .map_or((s, None), |idx| (&s[..idx], Some(&s[idx + 1..])));

    let Ok(parts) = id_part.parse::<Parts>() else {
        return None;
    };

    if !parts.target_id.is_valid() {
        return None;
    }

    let url_str = match query {
        Some(q) if !q.is_empty() => {
            format!("jp://{id_part}?{}", canonicalize_shorthand_query(q))
        }
        _ => format!("jp://{id_part}"),
    };

    Url::parse(&url_str).ok()
}

fn canonicalize_shorthand_query(q: &str) -> String {
    if starts_with_known_param(q) {
        return q.to_owned();
    }

    format!("select={q}")
}

fn starts_with_known_param(q: &str) -> bool {
    ["select", "raw"].iter().any(|name| {
        q == *name || q.starts_with(&format!("{name}=")) || q.starts_with(&format!("{name}&"))
    })
}

#[cfg(test)]
#[path = "parser_tests.rs"]
mod tests;
