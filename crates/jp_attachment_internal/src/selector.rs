//! Parser for the conversation attachment selector DSL.
//!
//! See the crate README for the full grammar.
//!
//! The DSL uses `,` as the content separator (e.g. `u,a:-1`) so it survives
//! URL query-string parsing intact — form-urlencoded decoding turns `+`
//! into a space, but leaves `,` alone.

use std::{fmt, str::FromStr};

/// What content kinds to include from a selected turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) struct Content {
    pub assistant: bool,
    pub user: bool,
    pub reasoning: bool,
    pub tools: bool,
}

impl Content {
    /// Default content: assistant messages only.
    pub const fn assistant_only() -> Self {
        Self {
            assistant: true,
            user: false,
            reasoning: false,
            tools: false,
        }
    }

    /// All content kinds.
    pub const fn all() -> Self {
        Self {
            assistant: true,
            user: true,
            reasoning: true,
            tools: true,
        }
    }

    fn is_empty(self) -> bool {
        !(self.assistant || self.user || self.reasoning || self.tools)
    }
}

impl fmt::Display for Content {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if *self == Self::all() {
            return f.write_str("*");
        }

        let mut parts = Vec::with_capacity(4);
        if self.assistant {
            parts.push("a");
        }
        if self.user {
            parts.push("u");
        }
        if self.reasoning {
            parts.push("r");
        }
        if self.tools {
            parts.push("t");
        }

        f.write_str(&parts.join(","))
    }
}

/// Which turns to include. Both bounds are inclusive.
///
/// Negative values count from the end (`-1` = last turn).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct Range {
    /// Left bound, 1-based. `None` means "from the start".
    pub start: Option<i64>,
    /// Right bound, 1-based. `None` means "to the end".
    pub end: Option<i64>,
}

impl Range {
    /// Select just the last turn.
    pub const fn last() -> Self {
        Self {
            start: Some(-1),
            end: None,
        }
    }

    /// Select all turns.
    #[allow(dead_code)]
    pub const fn all() -> Self {
        Self {
            start: None,
            end: None,
        }
    }

    /// Resolve the 0-based half-open `[start, end)` interval over a stream of
    /// `total` turns. Returns `None` if the selection is empty.
    pub fn resolve(self, total: usize) -> Option<(usize, usize)> {
        if total == 0 {
            return None;
        }

        let total_i = i64::try_from(total).ok()?;

        let start = normalize(self.start.unwrap_or(1), total_i).max(0);
        let end_inclusive = normalize(self.end.unwrap_or(total_i), total_i).min(total_i - 1);

        if start > end_inclusive {
            return None;
        }

        let start_usize = usize::try_from(start).ok()?;
        let end_exclusive = usize::try_from(end_inclusive).ok()?.saturating_add(1);

        Some((start_usize, end_exclusive))
    }
}

/// Map a 1-based signed index to a 0-based unsigned index.
///
/// Positive `N` → `N - 1`. Negative `-N` → `total - N`. `0` is clamped to `0`.
fn normalize(idx: i64, total: i64) -> i64 {
    use std::cmp::Ordering;
    match idx.cmp(&0) {
        Ordering::Equal => 0,
        Ordering::Greater => idx - 1,
        Ordering::Less => total + idx,
    }
}

impl fmt::Display for Range {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.start, self.end) {
            (None, None) => f.write_str(".."),
            // Negative left-open ranges use the `-N` shorthand so the round-
            // trip matches what the user wrote (`a:-3` not `a:-3..`).
            (Some(s), None) if s < 0 => write!(f, "{s}"),
            (Some(s), None) => write!(f, "{s}.."),
            (None, Some(e)) => write!(f, "..{e}"),
            (Some(s), Some(e)) if s == e => write!(f, "{s}"),
            (Some(s), Some(e)) => write!(f, "{s}..{e}"),
        }
    }
}

/// A parsed selector spec: `CONTENT[:RANGE]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) struct Selector {
    pub content: Content,
    pub range: Range,
}

impl Default for Selector {
    fn default() -> Self {
        Self {
            content: Content::assistant_only(),
            range: Range::last(),
        }
    }
}

impl FromStr for Selector {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Ok(Self::default());
        }

        // The DSL has two components separated by `:`. With both parts
        // present (`a:-1`), the split is unambiguous. With only one part
        // present, the shape decides: input made up of digits, dashes, and
        // dots is a range (e.g. `-1`, `5..-3`); anything else is content
        // (e.g. `a`, `u,a`).
        let (content_str, range_str) = match s.split_once(':') {
            Some((c, r)) => (c, Some(r)),
            None if looks_like_range(s) => ("", Some(s)),
            None => (s, None),
        };

        let content = if content_str.is_empty() {
            Content::assistant_only()
        } else {
            parse_content(content_str)?
        };

        let range = match range_str {
            None => Range::last(),
            Some(r) => parse_range(r)?,
        };

        Ok(Self { content, range })
    }
}

/// Returns `true` if `s` is shaped like a range bound — only ASCII digits,
/// dashes, and dots. Used to disambiguate single-component selector input
/// without a colon: `-1` is a range, while `a` is content.
fn looks_like_range(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_digit() || c == '-' || c == '.')
}

impl fmt::Display for Selector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.content, self.range)
    }
}

fn parse_content(s: &str) -> Result<Content, String> {
    let mut content = Content {
        assistant: false,
        user: false,
        reasoning: false,
        tools: false,
    };

    for part in s.split(',') {
        match part.trim() {
            "a" | "assistant" => content.assistant = true,
            "u" | "user" => content.user = true,
            "r" | "reasoning" => content.reasoning = true,
            "t" | "tools" => content.tools = true,
            "*" | "all" => content = Content::all(),
            "" => return Err("empty content flag".into()),
            other => return Err(format!("unknown content flag '{other}'")),
        }
    }

    if content.is_empty() {
        return Err("at least one content flag is required".into());
    }

    Ok(content)
}

fn parse_range(s: &str) -> Result<Range, String> {
    if s.is_empty() {
        return Err("empty range".into());
    }

    if let Some((left, right)) = s.split_once("..") {
        let start = parse_signed_bound(left, "start")?;
        let end = parse_signed_bound(right, "end")?;
        return Ok(Range { start, end });
    }

    // Shorthand: single signed index.
    let n = parse_signed_index(s, "turn")?;
    if n < 0 {
        Ok(Range {
            start: Some(n),
            end: None,
        })
    } else {
        Ok(Range {
            start: Some(n),
            end: Some(n),
        })
    }
}

fn parse_signed_bound(s: &str, label: &str) -> Result<Option<i64>, String> {
    if s.is_empty() {
        return Ok(None);
    }
    Ok(Some(parse_signed_index(s, label)?))
}

fn parse_signed_index(s: &str, label: &str) -> Result<i64, String> {
    let n: i64 = s.parse().map_err(|_| format!("invalid {label} '{s}'"))?;
    if n == 0 {
        return Err(format!("{label} must be non-zero (indices are 1-based)"));
    }
    Ok(n)
}

#[cfg(test)]
#[path = "selector_tests.rs"]
mod tests;
