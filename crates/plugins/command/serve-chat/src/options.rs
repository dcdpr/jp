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
//! `compact` lists the compactions "@JP compact" asks `jp` for, in the shape of
//! the plugin protocol's `compact` request.
//! It defaults to stripping reasoning and tool calls from every turn but the
//! first and the last:
//!
//! ```toml
//! [[plugins.command.serve-chat.options.compact]]
//! from = 2
//! to = -2
//! reasoning = {}
//! tool_calls = { mode = "strip" }
//! ```
//!
//! Only messages from senders in `people` are answered; all other messages are
//! dropped.
//! Every option except `people` and `signal.account` is optional.
//! Unknown keys are ignored.

use std::{collections::BTreeMap, num::NonZeroI64, time::Duration};

use camino::Utf8PathBuf;
use chrono::{Datelike as _, Days, NaiveDate, Weekday};
use jp_plugin::message::{Compaction, ReasoningCompaction, ToolCallsCompaction, ToolCallsMode};
use serde::Deserialize;
use serde_json::{Map, Value};

const KEY: &str = "plugins.command.serve-chat.options";

const DEFAULT_DAILY_TTL: &str = "7d";

const DEFAULT_WEEKLY_TTL: &str = "14d";

const DEFAULT_PULL_INTERVAL: Duration = Duration::from_mins(5);

const DEFAULT_WRITABLE: [&str; 3] = ["docs/ticket", "docs/rfd", "docs/wiki"];

const DAY: Duration = Duration::from_hours(24);

const DAYS_PER_WEEK: u32 = 7;

const DEFAULT_DECIDE_URL: &str = "https://api.typesafe.ai/v1/systemone";

const DEFAULT_DECIDE_MODEL: &str = "jev-latest";

const DEFAULT_DECIDE_KEY_ENV: &str = "JEV_API_KEY";

const DEFAULT_DECIDE_THRESHOLD: f64 = 0.5;

const SECOND_TURN: NonZeroI64 = NonZeroI64::new(2).unwrap();

const SECOND_TO_LAST_TURN: NonZeroI64 = NonZeroI64::new(-2).unwrap();

/// Default prompt sent to the decide API when `decide.instructions` is unset.
pub(crate) const DEFAULT_DECIDE_INSTRUCTIONS: &str =
    "What should the assistant do with the last message in this chat?";

#[derive(Debug, Clone, PartialEq)]
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

    /// Optional pre-turn message classifier configuration.
    pub decide: Option<DecideOptions>,

    /// Compaction rules sent to the host when a user requests compaction.
    pub compact: Vec<Compaction>,
}

/// Configuration for the System One decide API.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DecideOptions {
    pub url: String,
    pub model: String,

    /// Environment variable holding the bearer token, or `None` for
    /// unauthenticated endpoints.
    pub api_key_env: Option<String>,

    /// Minimum probability required to act on the model's choice; below this
    /// threshold, the bridge defaults to answering.
    pub threshold: f64,

    /// Question prompt sent to the decide model.
    pub instructions: String,
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

    /// Where `signal-cli` saves received attachments; `None` for its default.
    pub attachments_dir: Option<Utf8PathBuf>,
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
            Self::Weekly(day) => today - Days::new(u64::from(today.weekday().days_since(day))),
        }
    }

    /// Duration of one rollover period.
    fn length(self) -> Duration {
        match self {
            Self::Daily => DAY,
            Self::Weekly(_) => DAY * DAYS_PER_WEEK,
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
    decide: Option<RawDecide>,
    compact: Option<Vec<Compaction>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawDecide {
    url: Option<String>,
    model: Option<String>,
    api_key_env: Option<String>,
    threshold: Option<f64>,
    instructions: Option<String>,
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
    attachments_dir: Option<Utf8PathBuf>,
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
        let conversation_ttl = conversation_ttl(raw.conversation_ttl, rollover)?;

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
            signal: signal(raw.signal)?,
            decide: raw.decide.map(decide).transpose()?,
            compact: compact(raw.compact)?,
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

/// Validate `compact`, defaulting to stripping reasoning and tool calls from
/// turns `2..-2` (matching the built-in `conversation.compaction.rules`
/// default).
fn compact(raw: Option<Vec<Compaction>>) -> Result<Vec<Compaction>, String> {
    let Some(compactions) = raw else {
        return Ok(vec![Compaction {
            from: SECOND_TURN,
            to: SECOND_TO_LAST_TURN,
            reasoning: Some(ReasoningCompaction::default()),
            tool_calls: Some(ToolCallsCompaction {
                mode: ToolCallsMode::Strip,
                over_bytes: None,
            }),
            summary: None,
        }]);
    };
    if compactions.is_empty() {
        return Err(format!(
            "`{KEY}.compact` names no compactions; leave it out for the default."
        ));
    }

    Ok(compactions)
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

fn decide(raw: RawDecide) -> Result<DecideOptions, String> {
    let threshold = raw.threshold.unwrap_or(DEFAULT_DECIDE_THRESHOLD);
    if !(0.0..=1.0).contains(&threshold) {
        return Err(format!(
            "`{KEY}.decide.threshold` is `{threshold}`; it is a probability, from 0 to 1."
        ));
    }
    // Empty names no variable: the server, such as a local model, asks for no
    // token.
    let api_key_env = raw.api_key_env.map_or_else(
        || DEFAULT_DECIDE_KEY_ENV.to_owned(),
        |name| name.trim().to_owned(),
    );
    let api_key_env = (!api_key_env.is_empty()).then_some(api_key_env);

    Ok(DecideOptions {
        url: raw.url.unwrap_or_else(|| DEFAULT_DECIDE_URL.to_owned()),
        model: raw.model.unwrap_or_else(|| DEFAULT_DECIDE_MODEL.to_owned()),
        api_key_env,
        threshold,
        instructions: raw
            .instructions
            .filter(|instructions| !instructions.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_DECIDE_INSTRUCTIONS.to_owned()),
    })
}

/// Validate `conversation_ttl`, defaulting based on `rollover` and rejecting
/// durations shorter than the rollover period.
fn conversation_ttl(raw: Option<String>, rollover: Rollover) -> Result<String, String> {
    let conversation_ttl = raw.unwrap_or_else(|| {
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
            "`{KEY}.conversation_ttl` is `{conversation_ttl}`, shorter than a conversation lasts: \
             it would be removed while the chat still uses it. Make it at least `{}`.",
            humantime::format_duration(rollover.length())
        ));
    }

    Ok(conversation_ttl)
}

fn signal(raw: RawSignal) -> Result<SignalOptions, String> {
    let account = raw
        .account
        .map(|account| account.trim().to_owned())
        .filter(|account| !account.is_empty())
        .ok_or_else(|| format!("`{KEY}.signal.account` is required: the account to run as."))?;

    Ok(SignalOptions {
        account,
        account_id: raw
            .account_id
            .map(|id| id.trim().to_owned())
            .filter(|id| !id.is_empty()),
        command: raw.command.unwrap_or_else(|| "signal-cli".to_owned()),
        attachments_dir: raw.attachments_dir,
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
