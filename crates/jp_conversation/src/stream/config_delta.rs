//! Config deltas: the stream entries that change a conversation's
//! configuration.
//!
//! A conversation's config is not a snapshot but a base plus an ordered series
//! of changes, each recorded as a stream entry.
//! [`ConfigDelta`] is one such change, and [`fold`] is what applies it:
//! resolving a conversation's config means folding every delta in the stream
//! onto the base, in order.
//!
//! The on-disk shape is older than the type, so both serialization directions
//! are hand-rolled.
//! [`deserialize`] reads every shape ever written, including entries that
//! predate the [`Reset`] variant and entries whose config fields sat directly
//! in the envelope.
//!
//! [`Reset`]: ConfigDelta::Reset

use chrono::{DateTime, Utc};
use jp_config::{ConfigError, PartialAppConfig, PartialConfig as _};
use serde::{Serialize, Serializer};
use serde_json::Value;
use tracing::warn;

use crate::compat::deserialize_partial_config;

/// A configuration delta.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigDelta {
    /// Merge a partial configuration on top of the accumulated config state.
    Apply(ApplyDelta),

    /// Discard the accumulated config state.
    ///
    /// Config resolution restarts from program defaults; subsequent [`Apply`]
    /// events layer on top.
    ///
    /// [`Apply`]: Self::Apply
    Reset(ResetDelta),
}

impl ConfigDelta {
    /// The timestamp of the event, regardless of variant.
    #[must_use]
    pub const fn timestamp(&self) -> DateTime<Utc> {
        match self {
            Self::Apply(delta) => delta.timestamp,
            Self::Reset(delta) => delta.timestamp,
        }
    }
}

// Hand-rolled so `Apply` keeps the legacy flat shape (no `op` field) and
// `Reset` carries `"op": "reset"`. The variant discriminator must live inside
// the event body: the outer stream entry envelope already claims the top-level
// `type` key.
impl Serialize for ConfigDelta {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Apply(delta) => delta.serialize(serializer),
            Self::Reset(delta) => {
                #[derive(Serialize)]
                struct Tagged<'a> {
                    op: &'static str,
                    #[serde(flatten)]
                    inner: &'a ResetDelta,
                }

                Tagged {
                    op: "reset",
                    inner: delta,
                }
                .serialize(serializer)
            }
        }
    }
}

/// A configuration delta that merges on top of the accumulated config state.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ApplyDelta {
    /// The timestamp of the event.
    #[serde(serialize_with = "crate::serialize_dt")]
    pub timestamp: DateTime<Utc>,

    /// The configuration delta.
    pub delta: Box<PartialAppConfig>,

    /// Dotted paths of fields cleared before [`delta`] is merged.
    ///
    /// Merging is per field, so a field that merges by appending cannot reach a
    /// value that drops one of its elements: whatever the delta carries is
    /// added to what is already there.
    /// Clearing the field first leaves the merge nothing to combine with, and
    /// the delta's value lands whole.
    ///
    /// A path that names no field is ignored, so a delta written by a newer
    /// version, or naming a field since removed, still replays.
    ///
    /// [`delta`]: Self::delta
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unsets: Vec<String>,
}

impl ApplyDelta {
    /// An apply that merges `delta` and clears nothing.
    #[must_use]
    pub fn new(timestamp: DateTime<Utc>, delta: impl Into<Box<PartialAppConfig>>) -> Self {
        Self {
            timestamp,
            delta: delta.into(),
            unsets: Vec::new(),
        }
    }

    /// An apply that clears `unsets` before merging `delta`.
    #[must_use]
    pub fn with_unsets(
        timestamp: DateTime<Utc>,
        delta: impl Into<Box<PartialAppConfig>>,
        unsets: Vec<String>,
    ) -> Self {
        Self {
            timestamp,
            delta: delta.into(),
            unsets,
        }
    }
}

/// A configuration delta that discards the accumulated config state, resetting
/// it to program defaults.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ResetDelta {
    /// The timestamp of the event.
    #[serde(serialize_with = "crate::serialize_dt")]
    pub timestamp: DateTime<Utc>,
}

impl From<ApplyDelta> for ConfigDelta {
    fn from(delta: ApplyDelta) -> Self {
        Self::Apply(delta)
    }
}

impl From<ResetDelta> for ConfigDelta {
    fn from(delta: ResetDelta) -> Self {
        Self::Reset(delta)
    }
}

impl From<PartialAppConfig> for ConfigDelta {
    fn from(config: PartialAppConfig) -> Self {
        Self::Apply(ApplyDelta::new(Utc::now(), config))
    }
}

/// Extract the stored config subtree from a `config_delta` event.
///
/// Two on-disk shapes exist.
/// Newer streams nest the config under a `delta` key; older ones carry the
/// config fields as siblings of the envelope keys (`type`, `timestamp`, `op`),
/// which are removed here so only config fields remain.
///
/// The value is the entry as stored, so `event_id` may still be among those
/// siblings and is removed with them.
/// The stream-entry deserializer lifts that key out before a payload is read,
/// but this is also reached with a raw entry it never saw: the first element of
/// a legacy file, which [`ConversationStream::from_legacy_events`] reads as the
/// base config rather than as an entry.
///
/// [`ConversationStream::from_legacy_events`]: crate::ConversationStream::from_legacy_events
pub(super) fn subtree(value: &Value) -> Value {
    if let Some(delta) = value.get("delta") {
        return delta.clone();
    }

    let mut obj = value.as_object().cloned().unwrap_or_default();
    obj.remove("type");
    obj.remove("timestamp");
    obj.remove("op");
    obj.remove("unsets");
    obj.remove("event_id");
    Value::Object(obj)
}

/// Deserialize a [`ConfigDelta`] from a raw JSON value, tolerating schema
/// changes within the stored config.
///
/// The `op` field selects the variant: absent (which covers every event written
/// before the reset variant existed) or `"apply"` decodes as
/// [`ConfigDelta::Apply`]; `"reset"` decodes as [`ConfigDelta::Reset`].
/// Delegates to [`deserialize_partial_config`] for the config subtree and
/// extracts the timestamp separately.
///
/// # Errors
///
/// Returns an error for any other `op` value: an op added by a newer version
/// must fail loudly here instead of being misread as an apply and corrupting
/// config resolution.
pub(crate) fn deserialize(value: &Value) -> Result<ConfigDelta, String> {
    let timestamp = value
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|s| crate::parse_dt(s).ok())
        .unwrap_or_else(Utc::now);

    if let Some(op) = value.get("op") {
        if op == "reset" {
            return Ok(ConfigDelta::Reset(ResetDelta { timestamp }));
        }

        if op != "apply" {
            return Err(format!("unknown config delta `op`: {op}"));
        }
    }

    // The hand-rolled deserializer bypasses the derived one, so the field's
    // `#[serde(default)]` never runs and the key has to be read here.
    let unsets = value
        .get("unsets")
        .and_then(Value::as_array)
        .map(|paths| {
            paths
                .iter()
                .filter_map(|path| path.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();

    let delta = deserialize_partial_config(subtree(value));

    Ok(ConfigDelta::Apply(ApplyDelta {
        timestamp,
        delta: Box::new(delta),
        unsets,
    }))
}

/// Fold a single [`ConfigDelta`] into an accumulated partial config state.
///
/// [`Apply`] merges the delta on top of `state`.
/// [`Reset`] discards `state`, restarting from the empty partial
/// (`PartialAppConfig::default()`); program defaults are injected when the
/// partial is finalized into an [`AppConfig`].
///
/// [`AppConfig`]: jp_config::AppConfig
/// [`Apply`]: ConfigDelta::Apply
/// [`Reset`]: ConfigDelta::Reset
pub(super) fn fold(state: &mut PartialAppConfig, delta: ConfigDelta) -> Result<(), ConfigError> {
    match delta {
        ConfigDelta::Apply(apply) => {
            for path in &apply.unsets {
                if let Err(error) = state.unset(path) {
                    warn!(%path, %error, "Ignoring a config delta unset for an unknown field.");
                }
            }

            state.merge(&(), *apply.delta)
        }
        ConfigDelta::Reset(_) => {
            *state = PartialAppConfig::default();
            Ok(())
        }
    }
}
