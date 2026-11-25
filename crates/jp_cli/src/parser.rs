use std::{convert::Infallible, fmt, str::FromStr};

use camino::{FromPathBufError, Utf8Path, Utf8PathBuf, absolute_utf8};
use clean_path::Clean as _;
use jp_id::Parts;
use relative_path::RelativePathBuf;
use tracing::trace;
use url::Url;

use crate::error::{Error, Result};

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
/// Returns a message naming the item when one is empty or fails to parse.
pub(crate) fn split_list<T>(value: &str, item: &str) -> std::result::Result<Vec<T>, String>
where
    T: FromStr,
    T::Err: fmt::Display,
{
    value
        .split(',')
        .map(|part| match part.trim() {
            "" => Err(format!("empty {item} in '{value}'")),
            part => part
                .parse()
                .map_err(|error| format!("invalid {item} '{part}': {error}")),
        })
        .collect()
}

#[derive(Debug, Clone)]
pub(crate) enum AttachmentUrlOrPath {
    Url(Url),
    Path(RelativePathBuf),
}

impl AttachmentUrlOrPath {
    /// Convert this argument into an attachment URL, requiring any file path to
    /// exist on disk.
    ///
    /// When `root` is provided, file paths must reside within the workspace and
    /// are made relative to `root`.
    pub fn parse(&self, root: Option<&Utf8Path>) -> Result<Url> {
        self.to_url(root, true)
    }

    /// Convert this argument into an attachment URL without checking whether a
    /// file path exists on disk.
    ///
    /// Path resolution matches [`Self::parse`], producing the same URL that was
    /// stored when the file was attached.
    pub fn parse_unchecked(&self, root: Option<&Utf8Path>) -> Result<Url> {
        self.to_url(root, false)
    }

    fn to_url(&self, root: Option<&Utf8Path>, require_exists: bool) -> Result<Url> {
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
            Some(path) => (path, true),
            None => (path.as_str(), false),
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

            if require_exists && !path.exists() {
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
        } else if require_exists && !path.exists() {
            return Err(Error::Attachment(format!(
                "Attachment path does not exist: {path}",
            )));
        }

        file_url(&path, exclude)
    }
}

/// The `file` URL naming the workspace-relative `path`, as an exclusion when
/// `exclude` is set.
///
/// The `file` handler builds it as it builds a `file` table with this `path`,
/// so the two forms name the same attachment, and a `#`, `?` or `%` in the file
/// name stays part of it.
fn file_url(path: &Utf8Path, exclude: bool) -> Result<Url> {
    let handler = jp_attachment::find_handler_by_scheme("file")
        .ok_or_else(|| Error::NotFound("Attachment handler", "file".to_owned()))?;
    let mut url = handler
        .table_url(path.as_str())
        .map_err(|error| Error::Attachment(format!("Invalid attachment path {path}: {error}")))?;

    if exclude {
        url.query_pairs_mut().append_pair("exclude", "true");
    }

    Ok(url)
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
