//! The page a location names, and what the review page shows, kept in the URL
//! so a reload or a shared link lands where it was.
//!
//! ```text
//! /                                             the review list
//! /reviews/R-1h4k2m9                            the first file, latest version
//! /reviews/R-1h4k2m9?file=docs/a.md&version=3   a file in a version
//! /reviews/R-1h4k2m9?file=docs/a.md&pass=2      what pass 2 changed
//! /reviews/R-1h4k2m9?from=1&to=3&diff=unified   versions 1 to 3, as a diff
//! ```

use std::fmt::Write as _;

/// The page a location names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// `/`: the review list.
    List,

    /// `/reviews/{id}`: one review.
    Review(ReviewView),

    /// Any other path.
    Unknown(String),
}

/// What the review page shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReviewView {
    /// The review: `R-1h4k2m9`.
    pub id: String,

    /// The file shown; the review's first when absent.
    pub file: Option<String>,

    /// The version the file and its comments are shown in; the latest when
    /// absent.
    pub version: Option<u32>,

    /// What changed, shown in place of the file as one version holds it.
    pub changes: Option<Changes>,

    /// Whether resolved comments are highlighted in the file.
    pub resolved: bool,
}

/// The changes view: what changed between two versions of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Changes {
    pub span: Span,

    /// Whether the raw unified diff is shown, in place of the rendered file.
    pub unified: bool,
}

/// The two versions a changes view compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Span {
    /// The version a pass was sent against, and the one recorded when it ended.
    Pass(u32),

    /// Two versions; the server picks the latest for an absent `to`, and the
    /// one before `to` for an absent `from`.
    Versions { from: Option<u32>, to: Option<u32> },
}

impl Route {
    /// The page `path`, with its `query` (without the `?`), names.
    #[must_use]
    pub fn parse(path: &str, query: &str) -> Self {
        let path = path.trim_end_matches('/');
        if path.is_empty() {
            return Self::List;
        }
        let Some(id) = path
            .strip_prefix("/reviews/")
            .filter(|id| !id.is_empty() && !id.contains('/'))
        else {
            return Self::Unknown(path.to_owned());
        };

        let params = params(query);
        let get = |key: &str| {
            params
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.as_str())
        };
        let number = |key: &str| get(key).and_then(|value| value.parse().ok());

        let span = match (number("pass"), number("from"), number("to")) {
            (Some(pass), ..) => Some(Span::Pass(pass)),
            (None, None, None) if get("changes").is_none() => None,
            (None, from, to) => Some(Span::Versions { from, to }),
        };

        Self::Review(ReviewView {
            id: decode(id),
            file: get("file")
                .filter(|file| !file.is_empty())
                .map(ToOwned::to_owned),
            version: number("version"),
            changes: span.map(|span| Changes {
                span,
                unified: get("diff") == Some("unified"),
            }),
            resolved: get("resolved") == Some("1"),
        })
    }

    /// The location of the page: its path, and query when it has one.
    #[must_use]
    pub fn href(&self) -> String {
        let view = match self {
            Self::List => return "/".to_owned(),
            Self::Unknown(path) => return path.clone(),
            Self::Review(view) => view,
        };

        let mut params: Vec<(&str, String)> = vec![];
        if let Some(file) = &view.file {
            params.push(("file", encode(file)));
        }
        if let Some(version) = view.version {
            params.push(("version", version.to_string()));
        }
        match view.changes.map(|changes| changes.span) {
            None => {}
            Some(Span::Pass(pass)) => params.push(("pass", pass.to_string())),
            Some(Span::Versions {
                from: None,
                to: None,
            }) => params.push(("changes", "1".to_owned())),
            Some(Span::Versions { from, to }) => {
                params.extend(from.map(|from| ("from", from.to_string())));
                params.extend(to.map(|to| ("to", to.to_string())));
            }
        }
        if view.changes.is_some_and(|changes| changes.unified) {
            params.push(("diff", "unified".to_owned()));
        }
        if view.resolved {
            params.push(("resolved", "1".to_owned()));
        }

        let mut href = format!("/reviews/{}", encode(&view.id));
        for (index, (key, value)) in params.iter().enumerate() {
            let separator = if index == 0 { '?' } else { '&' };
            // Writing to a `String` cannot fail.
            let _ = write!(href, "{separator}{key}={value}");
        }

        href
    }
}

impl ReviewView {
    /// The review `id`, as it opens: its first file, in its latest version.
    #[must_use]
    pub fn of(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            ..Self::default()
        }
    }
}

/// The decoded `key=value` pairs of a query.
fn params(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (decode(key), decode(value))
        })
        .collect()
}

/// How many bytes a percent escape takes: `%` and two hex digits.
const ESCAPE_LENGTH: usize = "%XX".len();

/// The radix of the digits in a percent escape.
const HEX: u32 = 16;

/// `text` with `%XX` escapes and `+` decoded; an escape that isn't one is kept
/// as it is.
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let escaped = bytes
            .get(index + 1..index + ESCAPE_LENGTH)
            .and_then(|hex| str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, HEX).ok());
        match (bytes[index], escaped) {
            (b'%', Some(byte)) => {
                decoded.push(byte);
                index += ESCAPE_LENGTH;
            }
            (b'+', _) => {
                decoded.push(b' ');
                index += 1;
            }
            (byte, _) => {
                decoded.push(byte);
                index += 1;
            }
        }
    }

    String::from_utf8_lossy(&decoded).into_owned()
}

/// `text` with every byte other than letters, digits, `-._~`, and `/`
/// percent-encoded.
pub(crate) fn encode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') {
            encoded.push(char::from(byte));
        } else {
            // Writing to a `String` cannot fail.
            let _ = write!(encoded, "%{byte:02X}");
        }
    }

    encoded
}

#[cfg(test)]
#[path = "route_tests.rs"]
mod tests;
