//! Shared time-parsing types for CLI arguments.

use std::{ffi::OsStr, ops::Deref, str::FromStr};

use chrono::{DateTime, Utc};
use clap::{Arg, ArgGroup, ArgMatches, Command, FromArgMatches, builder::TypedValueParser};
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
/// between subcommands that want range-based selection (`jp c rm`, `jp c
/// archive`, `jp c use`, …).
///
/// `--from` is inclusive, `--until` is exclusive.
/// Both accept the full [`TimeThreshold`] syntax (conversation ID, relative
/// duration, or absolute date).
///
/// # Type parameters
///
/// - `EXCLUSIVE`: whether the range is mutually exclusive with the positional
///   `id` argument provided by `PositionalIds`.
///   Commands like `c rm` and `c archive` use the range as a target replacement
///   (`EXCLUSIVE = true`, the default); commands like `c use` use it as a
///   candidate-set filter that composes with target keywords like `?p`
///   (`EXCLUSIVE = false`).
///
/// Because clap's derive macro doesn't evaluate const generics in `#[arg]`
/// attributes, this type implements [`clap::Args`] and [`clap::FromArgMatches`]
/// manually.
#[derive(Debug)]
pub(crate) struct CreationRange<const EXCLUSIVE: bool = true> {
    pub from: Option<TimeThreshold>,
    pub until: Option<TimeThreshold>,
}

impl<const EXCLUSIVE: bool> Default for CreationRange<EXCLUSIVE> {
    fn default() -> Self {
        Self {
            from: None,
            until: None,
        }
    }
}

impl<const EXCLUSIVE: bool> CreationRange<EXCLUSIVE> {
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

/// A clap [`TypedValueParser`] for [`TimeThreshold`].
#[derive(Clone)]
struct TimeThresholdParser;

impl TypedValueParser for TimeThresholdParser {
    type Value = TimeThreshold;

    fn parse_ref(
        &self,
        _cmd: &Command,
        _arg: Option<&Arg>,
        value: &OsStr,
    ) -> Result<Self::Value, clap::Error> {
        let s = value
            .to_str()
            .ok_or_else(|| clap::Error::new(clap::error::ErrorKind::InvalidUtf8))?;

        s.parse::<TimeThreshold>()
            .map_err(|e| clap::Error::raw(clap::error::ErrorKind::InvalidValue, format!("{e}\n")))
    }
}

impl<const EXCLUSIVE: bool> clap::Args for CreationRange<EXCLUSIVE> {
    fn augment_args(cmd: Command) -> Command {
        let from_help = "Match conversations created at or after the specified time.";
        let from_long_help = "Match conversations created at or after the specified \
                              time.\n\nAccepts a conversation ID (uses its creation timestamp), a \
                              relative duration (e.g. `3w`, `30d`, `6h`), or an absolute date \
                              (e.g. `2026-01-01`). Composable with `--until`.";
        let until_help = "Match conversations created before the specified time.";
        let until_long_help = "Match conversations created before the specified time.\n\nAccepts \
                               the same formats as `--from`. The range is half-open (`--until` is \
                               exclusive), so `--from X --until Y` matches everything in `[X, Y)`.";

        let mut group = ArgGroup::new("creation_range")
            .multiple(true)
            .args(["from", "until"]);
        if EXCLUSIVE {
            group = group.conflicts_with("id");
        }

        cmd.arg(
            Arg::new("from")
                .long("from")
                .value_parser(TimeThresholdParser)
                .help(from_help)
                .long_help(from_long_help),
        )
        .arg(
            Arg::new("until")
                .long("until")
                .value_parser(TimeThresholdParser)
                .help(until_help)
                .long_help(until_long_help),
        )
        .group(group)
    }

    fn augment_args_for_update(cmd: Command) -> Command {
        Self::augment_args(cmd)
    }
}

impl<const EXCLUSIVE: bool> FromArgMatches for CreationRange<EXCLUSIVE> {
    fn from_arg_matches(matches: &ArgMatches) -> Result<Self, clap::Error> {
        Ok(Self {
            from: matches.get_one::<TimeThreshold>("from").copied(),
            until: matches.get_one::<TimeThreshold>("until").copied(),
        })
    }

    fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), clap::Error> {
        if let Some(v) = matches.get_one::<TimeThreshold>("from").copied() {
            self.from = Some(v);
        }
        if let Some(v) = matches.get_one::<TimeThreshold>("until").copied() {
            self.until = Some(v);
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "time_tests.rs"]
mod tests;
