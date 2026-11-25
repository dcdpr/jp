//! 1-based user-facing positions and ranges in an ordered sequence.
//!
//! JP numbers conversation turns and compaction events from `1`.
//! A [`Position`] is a 1-based index counted from the start (`3`) or from the
//! end (`-1` is the last item).
//! A [`PositionSpec`] is a single position or an inclusive `A..B` range where
//! either bound may be omitted.
//!
//! Both types store the 1-based numbers as written.
//! [`Position::index`], [`PositionSpec::resolve`], and [`PositionSpec::clamp`]
//! translate them to 0-based indices against a sequence length.
//!
//! See `docs/wiki/00dxyba-indexing-and-counting-conventions.md`.

use std::{fmt, ops::Range, str::FromStr};

/// A 1-based position in an ordered sequence, counted from either end.
///
/// Both variants hold the number as written.
/// Parsing never produces `0`, which names no item from either end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Position {
    /// Counted from the start; `1` is the first item.
    Absolute(usize),

    /// Counted from the end; `1` (written `-1`) is the last item.
    FromEnd(usize),
}

impl Position {
    /// The last item of a sequence, written `-1`.
    pub const LAST: Self = Self::FromEnd(1);

    /// The 0-based index this position names in a sequence of `count` items.
    ///
    /// `None` when the sequence has no such item.
    #[must_use]
    pub const fn index(self, count: usize) -> Option<usize> {
        match self {
            Self::Absolute(n) | Self::FromEnd(n) if n == 0 || n > count => None,
            Self::Absolute(n) => Some(n - 1),
            Self::FromEnd(n) => Some(count - n),
        }
    }
}

impl FromStr for Position {
    type Err = ParsePositionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (digits, from_end) = s
            .strip_prefix('-')
            .map_or((s, false), |digits| (digits, true));

        match (digits.parse::<usize>(), from_end) {
            (Ok(0), false) => Err(ParsePositionError::Zero),
            (Ok(0), true) => Err(ParsePositionError::ZeroFromEnd),
            (Ok(n), false) => Ok(Self::Absolute(n)),
            (Ok(n), true) => Ok(Self::FromEnd(n)),
            (Err(_), _) => Err(ParsePositionError::NotANumber(s.to_owned())),
        }
    }
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Absolute(n) => write!(f, "{n}"),
            Self::FromEnd(n) => write!(f, "-{n}"),
        }
    }
}

/// One position, or an inclusive range of them.
///
/// Written `N`, or `A..B` with both ends included.
/// Either end of a range may be omitted: `10..` runs through the last item,
/// `..10` starts at the first, and `..` is the whole sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PositionSpec {
    /// A single item.
    Single(Position),

    /// An inclusive range `from..to`.
    /// `None` on either side is the start or the end of the sequence.
    Range(Option<Position>, Option<Position>),
}

impl PositionSpec {
    /// The whole sequence, written `..`.
    pub const ALL: Self = Self::Range(None, None);
    /// The last item of a sequence, written `-1`.
    pub const LAST: Self = Self::Single(Position::LAST);

    /// The first and last position this spec names; `None` is an open end.
    const fn ends(self) -> (Option<Position>, Option<Position>) {
        match self {
            Self::Single(position) => (Some(position), Some(position)),
            Self::Range(from, to) => (from, to),
        }
    }

    /// Resolve this spec to a half-open 0-based index range in a sequence of
    /// `count` items.
    ///
    /// The returned range is empty only for `..` on an empty sequence.
    ///
    /// # Errors
    ///
    /// Returns an error if either bound is out of range for `count`, or if the
    /// resolved end precedes the start (`to_index < from_index`).
    /// Bounds are validated against `count` before ordering is checked, because
    /// a from-end position can only be converted to an index when it is within
    /// `count`.
    pub fn resolve(self, count: usize) -> Result<Range<usize>, ResolvePositionError> {
        let (from, to) = self.ends();
        let index = |position: Position| {
            position
                .index(count)
                .ok_or(ResolvePositionError::OutOfRange { position, count })
        };

        let from_index = from.map(index).transpose()?;
        let to_index = to.map(index).transpose()?;

        if let (Some(from), Some(to), Some(from_index), Some(to_index)) =
            (from, to, from_index, to_index)
            && to_index < from_index
        {
            return Err(ResolvePositionError::Inverted {
                from,
                to,
                from_index,
                to_index,
            });
        }

        Ok(from_index.unwrap_or(0)..to_index.map_or(count, |index| index + 1))
    }

    /// Resolve this spec to a half-open 0-based index range in a sequence of
    /// `count` items, clamping out-of-range bounds to the sequence boundaries.
    ///
    /// Returns `None` if the clamped range is empty: the sequence is empty, the
    /// start lies past the last item, the end lies before the first item, or
    /// the resolved end precedes the start.
    #[must_use]
    pub fn clamp(self, count: usize) -> Option<Range<usize>> {
        let last = count.checked_sub(1)?;
        let (from, to) = self.ends();

        let from_index = match from {
            None => 0,
            Some(Position::Absolute(n)) => n.saturating_sub(1),
            Some(Position::FromEnd(n)) => count.saturating_sub(n),
        };
        let to_index = match to {
            None => last,
            Some(Position::Absolute(n)) => n.saturating_sub(1).min(last),
            Some(Position::FromEnd(n)) => count.checked_sub(n)?.min(last),
        };

        (from_index <= to_index).then_some(from_index..to_index + 1)
    }
}

impl FromStr for PositionSpec {
    type Err = ParsePositionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let Some((from, to)) = s.split_once("..") else {
            return s.parse().map(Self::Single);
        };

        let end = |text: &str| (!text.is_empty()).then(|| text.parse()).transpose();
        Ok(Self::Range(end(from)?, end(to)?))
    }
}

impl fmt::Display for PositionSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Single(position) => position.fmt(f),
            Self::Range(from, to) => {
                if let Some(from) = from {
                    from.fmt(f)?;
                }
                f.write_str("..")?;
                if let Some(to) = to {
                    to.fmt(f)?;
                }
                Ok(())
            }
        }
    }
}

/// Error returned when parsing a [`Position`] or [`PositionSpec`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsePositionError {
    /// The input is not an integer.
    NotANumber(String),

    /// The input is `0`, which is invalid for 1-based positions.
    Zero,

    /// The input is `-0`, which is invalid for 1-based from-end offsets.
    ZeroFromEnd,
}

impl ParsePositionError {
    /// Format this error for a sequence whose items are called `noun` (e.g.
    /// `"turn"` or `"compaction"`).
    #[must_use]
    pub fn describe(&self, noun: &str) -> String {
        match self {
            Self::NotANumber(text) => format!("invalid {noun} number `{text}`"),
            Self::Zero => format!("{noun} numbers are 1-based; `0` is not a valid {noun}"),
            Self::ZeroFromEnd => {
                format!("from-end offsets are 1-based; use `-1` for the last {noun}")
            }
        }
    }
}

/// Error returned when resolving a [`PositionSpec`] against a sequence length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvePositionError {
    /// A bound falls outside `1..=count`.
    OutOfRange {
        /// The out-of-range bound as written.
        position: Position,
        /// The number of items in the sequence.
        count: usize,
    },

    /// The resolved end index precedes the start index (`to_index <
    /// from_index`).
    Inverted {
        /// The start bound as written.
        from: Position,
        /// The end bound as written.
        to: Position,
        /// The resolved 0-based start index.
        from_index: usize,
        /// The resolved 0-based end index.
        to_index: usize,
    },
}

impl ResolvePositionError {
    /// Format this error for a conversation's sequence of `noun` items (e.g.
    /// `"turn"` or `"compaction"`).
    ///
    /// An inverted range includes both the bounds as written and the 1-based
    /// positions they resolved to, so inverted from-end ranges (`-1..-3`) are
    /// clear.
    #[must_use]
    pub fn describe(&self, noun: &str) -> String {
        match self {
            Self::OutOfRange { position, count } => {
                format!("{noun} {position} out of range (conversation has {count} {noun}s)")
            }
            Self::Inverted {
                from,
                to,
                from_index,
                to_index,
            } => format!(
                "range {from}..{to} is not valid: {noun} {} comes before {noun} {}",
                to_index + 1,
                from_index + 1
            ),
        }
    }
}

#[cfg(test)]
#[path = "position_tests.rs"]
mod tests;
