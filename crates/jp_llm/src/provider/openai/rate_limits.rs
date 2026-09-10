//! Reading the subscription usage state `OpenAI` reports on every response.
//!
//! The `x-codex-*` header family carries how much of each usage window is spent
//! and when it resets, on successful responses as well as rejections.
//! Two windows are reported per limit: `primary` and `secondary`.
//! Beyond the account-wide family there is one family per metered limit, named
//! by an infix (`x-codex-<limit>-primary-used-percent`), whose `-limit-name`
//! header says which model it covers.
//!
//! Parsing is pure.
//! [`apply`] is the one place that reads a rejection's headers and sharpens the
//! error they came with, so a cooldown lands on the window the account actually
//! spent rather than on a guess.

use chrono::{DateTime, TimeDelta, Utc};
use reqwest::header::HeaderMap;

use crate::error::{StreamError, StreamErrorKind};

/// How full a window has to be before it is worth telling the user.
const WARN_THRESHOLD: f64 = 80.0;

/// One usage window's state.
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    /// How much of the window is spent, as a percentage.
    pub used_percent: f64,

    /// How long the window covers, in minutes.
    pub window_minutes: Option<i64>,

    /// When the window resets.
    pub resets_at: Option<DateTime<Utc>>,
}

impl Window {
    /// Whether the window is spent.
    #[must_use]
    pub fn is_spent(&self) -> bool {
        self.used_percent >= 100.0
    }

    /// Whether the window is full enough to warn about.
    #[must_use]
    pub fn is_warning(&self) -> bool {
        self.used_percent >= WARN_THRESHOLD && !self.is_spent()
    }

    /// How the window is named in user-facing output.
    ///
    /// Derived from its length, which is what the user recognizes from their
    /// account page; an unreported length falls back to the window's position.
    #[must_use]
    pub fn name(&self, position: &str) -> String {
        match self.window_minutes {
            Some(minutes) if minutes % (60 * 24) == 0 => {
                let days = minutes / (60 * 24);
                format!("{days}-day limit")
            }
            Some(minutes) if minutes % 60 == 0 => {
                let hours = minutes / 60;
                format!("{hours}-hour limit")
            }
            Some(minutes) => format!("{minutes}-minute limit"),
            None => format!("{position} limit"),
        }
    }
}

/// One limit family's state.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    /// The family's id: `codex` for the account-wide limit, otherwise the infix
    /// from its header names.
    pub limit_id: String,

    /// Which model the limit covers, when the family names one.
    pub limit_name: Option<String>,

    pub primary: Option<Window>,
    pub secondary: Option<Window>,
}

impl Snapshot {
    /// The cooldown scope this family records against.
    ///
    /// A family naming a model covers only that model, so its scope is the
    /// model name; the account-wide family covers everything.
    #[must_use]
    pub fn scope(&self) -> String {
        self.limit_name
            .as_deref()
            .map_or_else(|| SCOPE_ACCOUNT.to_owned(), str::to_ascii_lowercase)
    }

    /// The spent window, if either is spent.
    #[must_use]
    pub fn spent(&self) -> Option<&Window> {
        [self.primary.as_ref(), self.secondary.as_ref()]
            .into_iter()
            .flatten()
            .find(|window| window.is_spent())
    }

    /// The fullest window worth warning about, if any.
    #[must_use]
    pub fn warning(&self) -> Option<&Window> {
        [self.primary.as_ref(), self.secondary.as_ref()]
            .into_iter()
            .flatten()
            .filter(|window| window.is_warning())
            .max_by(|a, b| a.used_percent.total_cmp(&b.used_percent))
    }
}

/// Sharpen a rejection with the usage state its headers report.
///
/// A subscription rejection says only that something is spent.
/// The headers say which window, and when it reopens, which is the difference
/// between a cooldown that expires on its own and one guessed from a default.
///
/// Headers that report no spent window leave the error alone: a request refused
/// for another reason should not retire the credential that sent it.
pub fn apply(error: &mut StreamError, headers: &HeaderMap) {
    let snapshots = parse_all(headers);
    let Some((snapshot, window)) = snapshots
        .iter()
        .find_map(|snapshot| snapshot.spent().map(|window| (snapshot, window)))
    else {
        return;
    };

    // The account is out on a window the request had to draw from, whatever the
    // body called it.
    error.kind = StreamErrorKind::SubscriptionExhausted;
    error.quota_scope = Some(snapshot.scope());
    error.quota_reset = window.resets_at;
}

/// The account-wide cooldown scope.
///
/// Matches `jp_credentials::SCOPE_ACCOUNT`; restated here so the parser stays
/// free of store types.
const SCOPE_ACCOUNT: &str = "account";

/// The account-wide limit family's id.
const DEFAULT_LIMIT_ID: &str = "codex";

/// Read every limit family the headers report.
///
/// Families with no window data are dropped: a header set that names a family
/// but reports nothing about it says only that the family exists.
#[must_use]
pub fn parse_all(headers: &HeaderMap) -> Vec<Snapshot> {
    let mut ids: Vec<String> = headers
        .keys()
        .filter_map(|name| limit_id(name.as_str()))
        .collect();

    ids.push(DEFAULT_LIMIT_ID.to_owned());
    ids.sort_unstable();
    ids.dedup();

    ids.into_iter()
        .filter_map(|id| parse_family(headers, &id))
        .collect()
}

/// Read one limit family, or `None` when it reports no window data.
fn parse_family(headers: &HeaderMap, limit_id: &str) -> Option<Snapshot> {
    let prefix = if limit_id == DEFAULT_LIMIT_ID {
        "x-codex".to_owned()
    } else {
        format!("x-codex-{}", limit_id.replace('_', "-"))
    };

    let primary = parse_window(headers, &prefix, "primary");
    let secondary = parse_window(headers, &prefix, "secondary");

    if primary.is_none() && secondary.is_none() {
        return None;
    }

    Some(Snapshot {
        limit_id: limit_id.to_owned(),
        limit_name: string(headers, &format!("{prefix}-limit-name")),
        primary,
        secondary,
    })
}

/// Read one window of one family.
///
/// A window is reported only when something about it is non-zero: the server
/// sends the full header set for every family, so an all-zero window with no
/// reset means "no such window" rather than "an empty one".
fn parse_window(headers: &HeaderMap, prefix: &str, position: &str) -> Option<Window> {
    let used_percent = number::<f64>(headers, &format!("{prefix}-{position}-used-percent"))?;
    let window_minutes = number::<i64>(headers, &format!("{prefix}-{position}-window-minutes"));
    let resets_at = reset_at(headers, prefix, position);

    let reported = used_percent != 0.0
        || window_minutes.is_some_and(|minutes| minutes != 0)
        || resets_at.is_some();

    reported.then_some(Window {
        used_percent,
        window_minutes,
        resets_at,
    })
}

/// When a window resets.
///
/// The relative form is preferred: an absolute timestamp is only as good as the
/// client's clock, and the server sends both.
fn reset_at(headers: &HeaderMap, prefix: &str, position: &str) -> Option<DateTime<Utc>> {
    let relative = number::<i64>(headers, &format!("{prefix}-{position}-reset-after-seconds"))
        .filter(|seconds| *seconds > 0)
        .map(|seconds| Utc::now() + TimeDelta::seconds(seconds));

    relative.or_else(|| {
        number::<i64>(headers, &format!("{prefix}-{position}-reset-at"))
            .filter(|seconds| *seconds > 0)
            .and_then(|seconds| DateTime::from_timestamp(seconds, 0))
    })
}

/// The limit-family id a header name belongs to, if it names one.
///
/// Keyed off `-primary-used-percent`, which every family sends.
fn limit_id(header_name: &str) -> Option<String> {
    let lower = header_name.to_ascii_lowercase();
    let infix = lower
        .strip_suffix("-primary-used-percent")?
        .strip_prefix("x-codex")?;

    match infix.strip_prefix('-') {
        Some(id) if !id.is_empty() => Some(id.replace('-', "_")),
        _ => Some(DEFAULT_LIMIT_ID.to_owned()),
    }
}

/// A header's value as a number, or `None` when absent, blank, or unparseable.
///
/// Blank matters: the server sends `x-codex-secondary-reset-at:` with no value
/// when there is no secondary window.
fn number<T: std::str::FromStr>(headers: &HeaderMap, name: &str) -> Option<T> {
    string(headers, name)?.parse().ok()
}

/// A header's value as a non-empty string.
fn string(headers: &HeaderMap, name: &str) -> Option<String> {
    let value = headers.get(name)?.to_str().ok()?.trim();

    (!value.is_empty()).then(|| value.to_owned())
}

#[cfg(test)]
#[path = "rate_limits_tests.rs"]
mod tests;
