//! Shared time-parsing types for CLI arguments.

use std::{ops::Deref, str::FromStr};

use chrono::{DateTime, Utc};

/// A point in time parsed from either a relative duration (`3w`, `30d`) or an
/// absolute date/datetime (`2026-01-01`, RFC 3339).
///
/// Stored as an absolute `DateTime<Utc>`. Relative durations are subtracted
/// from `Utc::now()` at parse time.
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
        // Try as relative duration first (e.g. "3w", "30d", "6h").
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
            "invalid time threshold '{s}': expected a duration (3w, 30d) or date (2026-01-01)"
        ))
    }
}
