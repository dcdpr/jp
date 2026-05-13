//! Shared time-parsing types for CLI arguments.

use std::{ops::Deref, str::FromStr};

use chrono::{DateTime, Utc};
use jp_conversation::ConversationId;

/// A point in time parsed from a conversation ID, a relative duration
/// (`3w`, `30d`), or an absolute date/datetime (`2026-01-01`, RFC 3339).
///
/// Stored as an absolute `DateTime<Utc>`. Relative durations are subtracted
/// from `Utc::now()` at parse time. Conversation IDs resolve to their
/// embedded creation timestamp, which makes `--from jp-c…` a convenient
/// shorthand for `--from <when-that-conversation-was-created>`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TimeThreshold(pub DateTime<Utc>);

impl Deref for TimeThreshold {
    type Target = DateTime<Utc>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<TimeThreshold> for DateTime<Utc> {
    fn from(t: TimeThreshold) -> Self {
        t.0
    }
}

impl From<DateTime<Utc>> for TimeThreshold {
    fn from(dt: DateTime<Utc>) -> Self {
        Self(dt)
    }
}

impl FromStr for TimeThreshold {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Try as a conversation ID — its embedded timestamp is the threshold.
        if let Ok(id) = s.parse::<ConversationId>() {
            return Ok(Self(id.timestamp()));
        }

        // Try as relative duration (e.g. "3w", "30d", "6h").
        if let Ok(dur) = humantime::parse_duration(s) {
            let cutoff = Utc::now() - dur;
            return Ok(Self(cutoff));
        }

        // Try as RFC 3339 / ISO 8601 datetime.
        if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
            return Ok(Self(dt.with_timezone(&Utc)));
        }

        // Try as date-only (YYYY-MM-DD), interpreted as midnight UTC.
        if let Ok(date) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
            let dt = date
                .and_hms_opt(0, 0, 0)
                .expect("midnight is valid")
                .and_utc();
            return Ok(Self(dt));
        }

        Err(format!(
            "invalid time threshold '{s}': expected a conversation ID, a duration (3w, 30d), or a \
             date (2026-01-01)"
        ))
    }
}

/// A half-open `[from, until)` filter on conversation creation date, shared
/// between subcommands that want range-based selection (`jp c rm`,
/// `jp c archive`, …).
///
/// `--from` is inclusive, `--until` is exclusive. Both accept the full
/// [`TimeThreshold`] syntax (conversation ID, relative duration, or absolute
/// date). The pair is declared as a clap [`ArgGroup`] so the whole range is
/// mutually exclusive with the positional `id` argument provided by
/// `PositionalIds` — setting any bound and an `id` together is a parse error.
///
/// [`ArgGroup`]: clap::ArgGroup
#[derive(Debug, Default, clap::Args)]
#[group(id = "creation_range", multiple = true, conflicts_with = "id")]
pub(crate) struct CreationRange {
    /// Match conversations created at or after the specified time.
    ///
    /// Accepts a conversation ID (uses its creation timestamp), a relative
    /// duration (e.g. `3w`, `30d`, `6h`), or an absolute date
    /// (e.g. `2026-01-01`). Composable with `--until`.
    #[arg(long)]
    pub from: Option<TimeThreshold>,

    /// Match conversations created before the specified time.
    ///
    /// Accepts the same formats as `--from`. The range is half-open
    /// (`--until` is exclusive), so `--from X --until Y` matches everything
    /// in `[X, Y)`.
    #[arg(long)]
    pub until: Option<TimeThreshold>,
}

impl CreationRange {
    /// Whether either bound is set.
    pub fn is_set(&self) -> bool {
        self.from.is_some() || self.until.is_some()
    }

    /// Half-open range test on the conversation's creation timestamp.
    pub fn matches(&self, id: ConversationId) -> bool {
        self.from.is_none_or(|t| id.timestamp() >= *t)
            && self.until.is_none_or(|t| id.timestamp() < *t)
    }
}

#[cfg(test)]
#[path = "time_tests.rs"]
mod tests;
