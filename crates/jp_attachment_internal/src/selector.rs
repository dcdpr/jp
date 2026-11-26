//! Parser for the conversation attachment selector DSL.
//!
//! See the crate README for the full grammar.
//!
//! The DSL uses `,` as the content separator (e.g. `u,a:-1`) so it survives URL
//! query-string parsing intact — form-urlencoded decoding turns `+` into a
//! space, but leaves `,` alone.

use std::{fmt, str::FromStr};

use jp_conversation::position::PositionSpec;

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

/// A parsed selector spec: `CONTENT[:RANGE]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) struct Selector {
    pub content: Content,

    /// Which turns to include: one 1-based turn or an inclusive range, where a
    /// negative number counts from the end (`-1` is the last turn).
    pub range: PositionSpec,
}

impl Default for Selector {
    fn default() -> Self {
        Self {
            content: Content::assistant_only(),
            range: PositionSpec::LAST,
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
            None => PositionSpec::LAST,
            Some("") => return Err("empty range".into()),
            Some(r) => match r.parse::<PositionSpec>() {
                // A range whose ends are one turn selects that turn, so it is
                // stored as one: `-1..-1` is the default selector, and
                // displays as `-1`.
                Ok(PositionSpec::Range(Some(from), Some(to))) if from == to => {
                    PositionSpec::Single(from)
                }
                Ok(spec) => spec,
                Err(error) => return Err(error.describe("turn")),
            },
        };

        Ok(Self { content, range })
    }
}

/// Returns `true` if `s` is shaped like a range bound — only ASCII digits,
/// dashes, and dots.
/// Used to disambiguate single-component selector input without a colon: `-1`
/// is a range, while `a` is content.
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

#[cfg(test)]
#[path = "selector_tests.rs"]
mod tests;
