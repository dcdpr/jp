use std::convert::Infallible;

use camino::{FromPathBufError, Utf8Path, Utf8PathBuf, absolute_utf8};
use clean_path::Clean as _;
use relative_path::RelativePathBuf;
use tracing::trace;
use url::Url;

use crate::error::{Error, Result};

/// Prefix used by every JP ID.
///
/// This must stay in sync with `jp_id::ID_PREFIX`. Repeating it here avoids
/// pulling `jp_id` into the parser hot path just for a constant.
const JP_ID_PREFIX: &str = "jp-";

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
/// Returns `None` if `s` doesn't look like a JP ID. The check is intentionally
/// shallow: the shape `jp-<variant><target>` with `[a-z0-9]` characters is
/// enough to disambiguate from regular file paths. A directory literally
/// named `jp-c1234` would still match — prefix it with `./` to force the
/// path interpretation.
fn jp_id_shorthand_to_url(s: &str) -> Option<Url> {
    let (id_part, query) = split_jp_shorthand(s);
    if !looks_like_jp_id(id_part) {
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

fn split_jp_shorthand(s: &str) -> (&str, Option<&str>) {
    s.find('?')
        .map_or((s, None), |idx| (&s[..idx], Some(&s[idx + 1..])))
}

/// Names of query parameters the `jp://` scheme understands. A shorthand
/// suffix that starts with one of these (with `=` or `&` or end-of-string)
/// is passed through verbatim; anything else is treated as the value of an
/// implicit `select=`.
const KNOWN_JP_PARAMS: &[&str] = &["select", "raw"];

fn canonicalize_shorthand_query(q: &str) -> String {
    if starts_with_known_param(q) {
        return q.to_owned();
    }
    format!("select={q}")
}

fn starts_with_known_param(q: &str) -> bool {
    KNOWN_JP_PARAMS.iter().any(|name| {
        q == *name || q.starts_with(&format!("{name}=")) || q.starts_with(&format!("{name}&"))
    })
}

fn looks_like_jp_id(s: &str) -> bool {
    let Some(rest) = s.strip_prefix(JP_ID_PREFIX) else {
        return false;
    };
    let mut chars = rest.chars();
    // Variant: a single lowercase ASCII letter.
    let Some(variant) = chars.next() else {
        return false;
    };
    if !variant.is_ascii_lowercase() {
        return false;
    }
    // Target: at least one alphanumeric, all lowercase ASCII or digits.
    let target: &str = &rest[variant.len_utf8()..];
    !target.is_empty()
        && target
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

#[cfg(test)]
#[path = "parser_tests.rs"]
mod tests;
