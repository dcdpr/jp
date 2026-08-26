//! Event patch overlays.
//!
//! A provider can discover that events already in the stream carry metadata it
//! will not accept back — most often a stale reasoning signature.
//! Rather than rewrite those events, the repair is recorded as an overlay
//! appended to the stream, and applied when the stream is projected for a
//! request.
//!
//! The stored history stays byte-identical, so a repair made for one provider
//! or model does not destroy metadata another one could still use.
//!
//! These types mirror the provider-facing vocabulary in `jp_llm`.
//! The duplication is deliberate: the conversation crate owns what is
//! persisted, and must not depend on the LLM crate to say what a stored patch
//! means.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// An appended instruction to rewrite how earlier events are projected.
///
/// Overlays never modify or remove the events they target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventOverlay {
    /// When the overlay was recorded.
    pub timestamp: DateTime<Utc>,

    /// Patches to apply, in order.
    pub patches: Vec<OverlayPatch>,
}

/// A single matcher/action pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverlayPatch {
    /// Which events to target.
    pub matcher: OverlayMatcher,

    /// What to change about them.
    pub action: OverlayAction,
}

/// Identifies which events a patch applies to.
///
/// Matching is by content rather than position, so an overlay is independent of
/// where it sits in the stream and stays correct when earlier events are
/// trimmed or compacted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "matcher", rename_all = "snake_case")]
#[non_exhaustive]
pub enum OverlayMatcher {
    /// Match events whose `metadata[key]` is the string `value`.
    MetadataValue {
        /// Metadata key to compare.
        key: String,
        /// Value the key must hold.
        value: String,
    },
}

/// The change a patch makes to a matched event's projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
#[non_exhaustive]
pub enum OverlayAction {
    /// Drop a metadata key from the projected event.
    RemoveMetadata {
        /// Metadata key to drop.
        key: String,
    },
}

impl OverlayPatch {
    /// Whether this patch targets `metadata`, and would change it.
    ///
    /// A matcher that hits an event which does not carry the key the action
    /// removes is not a change: matching alone is not progress.
    #[must_use]
    pub fn would_change(&self, metadata: &serde_json::Map<String, serde_json::Value>) -> bool {
        let matched = match &self.matcher {
            OverlayMatcher::MetadataValue { key, value } => metadata
                .get(key)
                .and_then(serde_json::Value::as_str)
                .is_some_and(|v| v == value),
        };

        if !matched {
            return false;
        }

        match &self.action {
            OverlayAction::RemoveMetadata { key } => metadata.contains_key(key),
        }
    }

    /// Apply this patch to `metadata`, returning whether it changed anything.
    pub fn apply(&self, metadata: &mut serde_json::Map<String, serde_json::Value>) -> bool {
        if !self.would_change(metadata) {
            return false;
        }

        match &self.action {
            OverlayAction::RemoveMetadata { key } => metadata.remove(key).is_some(),
        }
    }
}
