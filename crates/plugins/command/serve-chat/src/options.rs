//! Plugin configuration parsed from `plugins.command.serve-chat.options`.
//!
//! ```toml
//! [plugins.command.serve-chat.options]
//! transport = "signal"
//! persona = "personas/planner"
//! admin = "maintainer"
//!
//! [plugins.command.serve-chat.options.signal]
//! account = "+31600000000"
//!
//! [plugins.command.serve-chat.options.people.maintainer]
//! name = "Max Maintainer"
//! signal = "+31611111111"
//! ```
//!
//! Only messages from senders in `people` are answered; all other messages are
//! dropped.
//! Every option except `people` and `signal.account` is optional.
//! Unknown keys are ignored.

use std::{collections::BTreeMap, time::Duration};

use chrono::{Datelike as _, Days, NaiveDate, Weekday};
use serde::Deserialize;
use serde_json::{Map, Value};

const KEY: &str = "plugins.command.serve-chat.options";

const DEFAULT_DAILY_TTL: &str = "7d";

const DEFAULT_WEEKLY_TTL: &str = "14d";

const DEFAULT_PULL_INTERVAL: Duration = Duration::from_mins(5);

const DEFAULT_WRITABLE: [&str; 3] = ["docs/ticket", "docs/rfd", "docs/wiki"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Options {
    /// Configuration value passed via `--cfg` on every turn.
    pub persona: Option<String>,

    /// Key in `people` of the admin notified when the bridge pauses.
    pub admin: Option<String>,

    /// How often a chat starts a new conversation.
    pub rollover: Rollover,

    /// Expiration duration for the bridge's conversations (in `jp query --tmp`
    /// format).
    pub conversation_ttl: String,

    /// Allowed senders, keyed by their configuration identifier.
    pub people: BTreeMap<String, Person>,

    pub checkout: CheckoutOptions,

    pub signal: SignalOptions,

    /// Additional `--cfg` values applied after `persona` on every turn.
    pub cfg: Vec<String>,
}

/// An allowed sender.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Person {
    /// Display name used for `user.name` and tool author attribution.
    pub name: String,

    /// Signal phone number or account UUID (ACI).
    pub signal: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CheckoutOptions {
    /// Whether to lock, pull, and check the checkout around turns.
    pub enabled: bool,

    /// Workspace-relative directories that turns are allowed to modify.
    pub writable: Vec<String>,

    pub pull_interval: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SignalOptions {
    /// Signal account identifier passed to `signal-cli -a`.
    pub account: String,

    /// Account UUID (ACI) for the bridge's own Signal account, used to detect
    /// mentions and replies that reference the account by UUID rather than
    /// phone number.
    pub account_id: Option<String>,

    pub command: String,
}

impl SignalOptions {
    /// Return all configured identifiers (`account` and `account_id`) for the
    /// bridge's Signal account.
    pub(crate) fn own_ids(&self) -> Vec<String> {
        let mut ids = vec![self.account.clone()];
        ids.extend(self.account_id.clone());
        ids
    }
}

/// How often a chat starts a new conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rollover {
    Daily,
    Weekly(Weekday),
}

impl Rollover {
    /// Return the start date of the rollover period containing `today`.
    pub(crate) fn start(self, today: NaiveDate) -> NaiveDate {
        match self {
            Self::Daily => today,
            Self::Weekly(day) => {
                let back =
                    (7 + today.weekday().num_days_from_monday() - day.num_days_from_monday()) % 7;
                today - Days::new(u64::from(back))
            }
        }
    }

    /// Duration of one rollover period.
    fn length(self) -> Duration {
        match self {
            Self::Daily => Duration::from_hours(24),
            Self::Weekly(_) => Duration::from_hours(7 * 24),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Raw {
    transport: Option<String>,
    persona: Option<String>,
    cfg: Vec<String>,
    admin: Option<String>,
    conversation_ttl: Option<String>,
    rollover: Option<String>,
    people: BTreeMap<String, RawPerson>,
    checkout: RawCheckout,
    signal: RawSignal,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawPerson {
    name: String,
    signal: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawCheckout {
    enabled: Option<bool>,
    writable: Option<Vec<String>>,
    pull_interval: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawSignal {
    account: Option<String>,
    account_id: Option<String>,
    command: Option<String>,
}

impl Options {
    /// Parse and validate `plugins.command.serve-chat.options` from `init`.
    pub(crate) fn read(options: &Map<String, Value>) -> Result<Self, String> {
        let raw: Raw = serde_json::from_value(Value::Object(options.clone()))
            .map_err(|error| format!("`{KEY}` is invalid: {error}."))?;

        match raw.transport.as_deref().map(str::trim) {
            None | Some("signal") => {}
            Some(other) => {
                return Err(format!(
                    "`{KEY}.transport` is `{other}`; the only transport is `signal`."
                ));
            }
        }

        let people = people(raw.people)?;
        if let Some(admin) = &raw.admin
            && !people.contains_key(admin)
        {
            return Err(format!(
                "`{KEY}.admin` is `{admin}`, which isn't one of `{KEY}.people`."
            ));
        }

        let rollover = rollover(raw.rollover.as_deref())?;
        let conversation_ttl = raw.conversation_ttl.unwrap_or_else(|| {
            match rollover {
                Rollover::Daily => DEFAULT_DAILY_TTL,
                Rollover::Weekly(_) => DEFAULT_WEEKLY_TTL,
            }
            .to_owned()
        });
        let ttl = humantime::parse_duration(&conversation_ttl)
            .map_err(|error| format!("`{KEY}.conversation_ttl` is invalid: {error}."))?;
        if ttl < rollover.length() {
            return Err(format!(
                "`{KEY}.conversation_ttl` is `{conversation_ttl}`, shorter than a conversation \
                 lasts: it would be removed while the chat still uses it. Make it at least `{}`.",
                humantime::format_duration(rollover.length())
            ));
        }

        let account = raw
            .signal
            .account
            .map(|account| account.trim().to_owned())
            .filter(|account| !account.is_empty())
            .ok_or_else(|| format!("`{KEY}.signal.account` is required: the account to run as."))?;

        Ok(Self {
            persona: raw.persona.filter(|persona| !persona.trim().is_empty()),
            cfg: raw
                .cfg
                .into_iter()
                .filter(|cfg| !cfg.trim().is_empty())
                .collect(),
            admin: raw.admin,
            rollover,
            conversation_ttl,
            people,
            checkout: checkout(raw.checkout)?,
            signal: SignalOptions {
                account,
                account_id: raw
                    .signal
                    .account_id
                    .map(|id| id.trim().to_owned())
                    .filter(|id| !id.is_empty()),
                command: raw
                    .signal
                    .command
                    .unwrap_or_else(|| "signal-cli".to_owned()),
            },
        })
    }

    /// Look up a configured person matching any of the Signal sender `ids`.
    pub(crate) fn signal_person(&self, ids: &[String]) -> Option<(&str, &Person)> {
        self.people.iter().find_map(|(key, person)| {
            person
                .signal
                .as_ref()
                .is_some_and(|account| ids.iter().any(|id| id == account))
                .then_some((key.as_str(), person))
        })
    }
}

/// Parse `rollover` from `"daily"` (default) or a weekday name such as
/// `"sunday"`.
fn rollover(raw: Option<&str>) -> Result<Rollover, String> {
    let Some(raw) = raw.map(str::trim) else {
        return Ok(Rollover::Daily);
    };
    if raw.eq_ignore_ascii_case("daily") {
        return Ok(Rollover::Daily);
    }

    raw.parse::<Weekday>().map(Rollover::Weekly).map_err(|_| {
        format!(
            "`{KEY}.rollover` is `{raw}`; use `daily`, or the day of the week a new conversation \
             starts on, such as `sunday`."
        )
    })
}

fn people(raw: BTreeMap<String, RawPerson>) -> Result<BTreeMap<String, Person>, String> {
    if raw.is_empty() {
        return Err(format!(
            "`{KEY}.people` is empty, so the bridge would answer nobody. List who it answers."
        ));
    }

    raw.into_iter()
        .map(|(key, person)| {
            let name = person.name.trim();
            if name.is_empty() || name.chars().any(char::is_control) {
                return Err(format!(
                    "`{KEY}.people.{key}.name` must be a name on one line."
                ));
            }

            Ok((key, Person {
                name: name.to_owned(),
                signal: person
                    .signal
                    .map(|account| account.trim().to_owned())
                    .filter(|account| !account.is_empty()),
            }))
        })
        .collect()
}

fn checkout(raw: RawCheckout) -> Result<CheckoutOptions, String> {
    let pull_interval = match raw.pull_interval {
        None => DEFAULT_PULL_INTERVAL,
        Some(interval) => humantime::parse_duration(&interval)
            .map_err(|error| format!("`{KEY}.checkout.pull_interval` is invalid: {error}."))?,
    };

    let writable: Vec<String> = raw
        .writable
        .unwrap_or_else(|| DEFAULT_WRITABLE.map(ToOwned::to_owned).to_vec())
        .into_iter()
        .map(|dir| dir.trim().trim_end_matches('/').to_owned())
        .collect();
    if let Some(bad) = writable
        .iter()
        .find(|dir| dir.is_empty() || dir.starts_with('/') || dir.split('/').any(|c| c == ".."))
    {
        return Err(format!(
            "`{KEY}.checkout.writable` holds `{bad}`; name directories relative to the workspace \
             root."
        ));
    }

    Ok(CheckoutOptions {
        enabled: raw.enabled.unwrap_or(true),
        writable,
        pull_interval,
    })
}

#[cfg(test)]
#[path = "options_tests.rs"]
mod tests;
