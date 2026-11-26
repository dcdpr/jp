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

/// What the review page shows of a file, as the header's picker offers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Showing {
    /// The file as the latest version holds it, moving on to newer ones.
    Latest,

    /// The file as one version holds it.
    Version(u32),

    /// What a pass changed.
    Pass(u32),

    /// What changed between two versions.
    Compare,
}

impl Showing {
    /// The value of the picker's option for this: `latest`, `v3`, `p2`, or
    /// `compare`.
    #[must_use]
    pub fn value(self) -> String {
        match self {
            Self::Latest => "latest".to_owned(),
            Self::Version(version) => format!("v{version}"),
            Self::Pass(pass) => format!("p{pass}"),
            Self::Compare => "compare".to_owned(),
        }
    }

    /// What the option valued `value` shows; `None` for a value no option has.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "latest" => return Some(Self::Latest),
            "compare" => return Some(Self::Compare),
            _ => {}
        }
        let (kind, number) = value.split_at_checked(1)?;
        let number = number.parse().ok()?;

        match kind {
            "v" => Some(Self::Version(number)),
            "p" => Some(Self::Pass(number)),
            _ => None,
        }
    }
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

    /// What the view shows of its file.
    #[must_use]
    pub fn showing(&self) -> Showing {
        match (self.changes.map(|changes| changes.span), self.version) {
            (Some(Span::Pass(pass)), _) => Showing::Pass(pass),
            (Some(Span::Versions { .. }), _) => Showing::Compare,
            (None, Some(version)) => Showing::Version(version),
            (None, None) => Showing::Latest,
        }
    }

    /// The view showing `showing` of the same file.
    ///
    /// A changes view keeps showing rendered or unified changes, as it did.
    /// Comparing starts from the version shown and the one before it.
    #[must_use]
    pub fn show(&self, showing: Showing) -> Self {
        let changes = |span| {
            Some(Changes {
                span,
                unified: self.changes.is_some_and(|changes| changes.unified),
            })
        };
        let (version, changes) = match showing {
            Showing::Latest => (None, None),
            Showing::Version(version) => (Some(version), None),
            Showing::Pass(pass) => (self.version, changes(Span::Pass(pass))),
            Showing::Compare => (
                self.version,
                changes(Span::Versions {
                    from: None,
                    to: self.version,
                }),
            ),
        };

        Self {
            version,
            changes,
            ..self.clone()
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

/// `text`, a part of a query, with `%XX` escapes and `+` decoded; an escape
/// that isn't one is kept as it is.
fn decode(text: &str) -> String {
    unescape(text, true)
}

/// `text` with `%XX` escapes decoded, and `+` read as a space when `plus` says
/// it stands for one, as it does in a query and not in a path; an escape that
/// isn't one is kept as it is.
pub(crate) fn unescape(text: &str, plus: bool) -> String {
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
            (b'+', _) if plus => {
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
