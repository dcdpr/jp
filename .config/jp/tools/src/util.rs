pub mod diff;
pub mod root;
pub mod runner;
pub mod xml;

use jp_md::format::Formatter;
use jp_tool::Outcome;
use serde::{Deserialize, Deserializer, Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::Tool;

pub type ToolResult = std::result::Result<Outcome, Box<dyn std::error::Error + Send + Sync>>;

/// Map a file path to a syntax-highlight language tag.
///
/// Looks at the path's extension and returns a known language identifier (the
/// kind a markdown code fence accepts).
/// If the extension isn't recognized, returns the extension itself — better
/// than nothing for languages we haven't enumerated.
#[must_use]
pub fn lang_from_path(path: &str) -> &str {
    match path.rsplit('.').next().unwrap_or_default() {
        "rs" => "rust",
        "js" | "mjs" | "cjs" => "javascript",
        "ts" | "tsx" => "typescript",
        "jsx" => "jsx",
        "py" => "python",
        "rb" => "ruby",
        "go" => "go",
        "c" | "h" => "c",
        "cpp" | "cc" | "hpp" | "hh" => "cpp",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "swift" => "swift",
        "sh" | "bash" => "bash",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "json" => "json",
        "md" => "markdown",
        other => other,
    }
}

/// Cap `s` at `max` bytes, appending a note naming the byte count that was kept
/// and the original size.
///
/// The cut lands on a UTF-8 character boundary, so the result is always valid
/// UTF-8 and may be slightly shorter than `max`.
/// Strings that already fit are returned unchanged, without a note.
///
/// Use this on any subprocess output that ends up in a tool result: a single
/// unbounded `stdout` or `stderr` can fill the assistant's whole context
/// window.
#[must_use]
pub fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }

    let end = s.floor_char_boundary(max);
    format!(
        "{}\n\n[Truncated: showing {end} of {} bytes]",
        &s[..end],
        s.len()
    )
}

/// Prefix every line of `document` with a markdown blockquote marker.
///
/// Empty lines get a bare `>` so the rail stays unbroken without trailing
/// whitespace.
/// Quoting an already-quoted document nests it one level deeper.
#[must_use]
pub fn quote(document: &str) -> String {
    let mut quoted = String::with_capacity(document.len() * 2);
    for line in document.lines() {
        quoted.push('>');
        if !line.is_empty() {
            quoted.push(' ');
            quoted.push_str(line);
        }
        quoted.push('\n');
    }

    quoted
}

/// Style a document for the terminal as a tool-call preview.
///
/// The document is quoted first, so the transcript carries a marker down the
/// whole preview and the reader can see where the tool output ends and the
/// conversation resumes.
/// Falls back to the unstyled source if the markdown can't be formatted.
#[must_use]
pub fn preview(document: &str) -> String {
    let quoted = quote(document);

    Formatter::new().format_terminal(&quoted).unwrap_or(quoted)
}

#[expect(clippy::unnecessary_wraps)]
pub fn error(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> ToolResult {
    Ok(Outcome::error(error.into().as_ref()))
}

#[expect(clippy::unnecessary_wraps)]
pub fn fail(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> ToolResult {
    Ok(Outcome::fail(error.into().as_ref()))
}

#[expect(clippy::needless_pass_by_value)]
pub fn unknown_tool(t: Tool) -> ToolResult {
    Err(format!("Unknown tool '{}'", t.name).into())
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum OneOrMany<T> {
    One(T),
    Many(Vec<T>),
}

impl<'de, T> Deserialize<'de> for OneOrMany<T>
where
    T: DeserializeOwned,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw<T> {
            One(T),
            Many(Vec<T>),
        }

        let value = Value::deserialize(deserializer)?;

        // LLMs sometimes encode an array argument as a JSON string holding the
        // array itself (e.g. `"[\"a\", \"b\"]"`). Treat that as the list it
        // represents rather than a single element containing raw JSON text.
        if let Value::String(s) = &value
            && let Ok(items) = serde_json::from_str::<Vec<T>>(s)
        {
            return Ok(Self::Many(items));
        }

        match serde_json::from_value(value).map_err(serde::de::Error::custom)? {
            Raw::One(v) => Ok(Self::One(v)),
            Raw::Many(v) => Ok(Self::Many(v)),
        }
    }
}

impl<T> OneOrMany<T> {
    /// Returns the inner value as a `Vec`, consuming the `OneOrMany`.
    #[must_use]
    pub fn into_vec(self) -> Vec<T> {
        match self {
            OneOrMany::One(v) => vec![v],
            OneOrMany::Many(v) => v,
        }
    }

    /// Returns the inner value as a slice.
    #[must_use]
    pub fn as_slice(&self) -> &[T] {
        match self {
            OneOrMany::One(v) => std::slice::from_ref(v),
            OneOrMany::Many(v) => v,
        }
    }
}

impl<T: PartialEq> PartialEq for OneOrMany<T> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::One(v1), Self::One(v2)) => v1 == v2,
            (Self::Many(v1), Self::Many(v2)) => v1 == v2,
            _ => false,
        }
    }
}

impl<T: Clone> Clone for OneOrMany<T> {
    fn clone(&self) -> Self {
        match self {
            Self::One(v) => Self::One(v.clone()),
            Self::Many(v) => Self::Many(v.clone()),
        }
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for OneOrMany<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::One(v) => std::fmt::Debug::fmt(v, f),
            Self::Many(v) => std::fmt::Debug::fmt(v, f),
        }
    }
}

impl<T> Default for OneOrMany<T> {
    fn default() -> Self {
        Self::Many(vec![])
    }
}

impl<T> std::ops::Deref for OneOrMany<T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        match self {
            OneOrMany::One(v) => std::slice::from_ref(v),
            OneOrMany::Many(v) => v,
        }
    }
}

impl<T> std::ops::DerefMut for OneOrMany<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            OneOrMany::One(v) => std::slice::from_mut(v),
            OneOrMany::Many(v) => v,
        }
    }
}

impl<T> FromIterator<T> for OneOrMany<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let mut items = iter.into_iter().collect::<Vec<_>>();

        if items.len() == 1 {
            Self::One(items.remove(0))
        } else {
            Self::Many(items)
        }
    }
}

impl<T> IntoIterator for OneOrMany<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        match self {
            OneOrMany::One(v) => vec![v].into_iter(),
            OneOrMany::Many(v) => v.into_iter(),
        }
    }
}

impl<T> From<T> for OneOrMany<T> {
    fn from(v: T) -> Self {
        Self::One(v)
    }
}

impl<T> From<Vec<T>> for OneOrMany<T> {
    fn from(mut v: Vec<T>) -> Self {
        if v.len() == 1 {
            Self::One(v.remove(0))
        } else {
            Self::Many(v)
        }
    }
}

impl<T> From<OneOrMany<T>> for Vec<T> {
    fn from(v: OneOrMany<T>) -> Self {
        match v {
            OneOrMany::One(v) => vec![v],
            OneOrMany::Many(v) => v,
        }
    }
}

#[cfg(test)]
#[path = "util_tests.rs"]
mod tests;
