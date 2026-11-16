//! Rule policy configuration: pass ownership, exemptions, and enforced rules.
//!
//! The policy lives in `.config/rustqual/qual.toml`:
//!
//! ```toml
//! exempt = ["iosp/violation"]
//! enforced = ["DRY-004"]
//!
//! [[pass]]
//! name = "dry"
//! config = "dry.toml"
//! rules = ["DRY-*"]
//!
//! [[pass]]
//! name = "main"
//! config = "config.toml"
//! rules = ["*"]
//! ```
//!
//! A rule pattern is a rule ID, or a prefix followed by `*`.

use std::collections::BTreeSet;

use serde::{Deserialize, Deserializer};

/// Policy filename inside the `rustqual` config directory.
pub const FILE: &str = "qual.toml";

/// An exact rule ID or a prefix pattern ending with `*`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern(String);

impl Pattern {
    /// Return `true` if `rule` matches this exact rule ID or prefix pattern.
    #[must_use]
    pub fn matches(&self, rule: &str) -> bool {
        match self.0.strip_suffix('*') {
            Some(prefix) => rule.starts_with(prefix),
            None => rule == self.0,
        }
    }
}

impl<'de> Deserialize<'de> for Pattern {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let pattern = String::deserialize(deserializer)?;
        if pattern.is_empty() {
            return Err(serde::de::Error::custom("a rule pattern can't be empty"));
        }
        if pattern.trim_end_matches('*').contains('*') {
            return Err(serde::de::Error::custom(format!(
                "`*` is only allowed at the end of a rule pattern: {pattern:?}"
            )));
        }
        Ok(Self(pattern))
    }
}

/// A configured `rustqual` analysis pass and the rules collected from it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pass {
    pub name: String,

    /// Path to the `rustqual` config file, relative to the policy file.
    pub config: String,

    /// Rule patterns owned by this pass.
    ///
    /// Each rule is collected from the first pass whose patterns match it and
    /// ignored in later passes, so rules reported by multiple passes are
    /// counted once.
    pub rules: Vec<Pattern>,
}

/// Rule gating policy and analysis pass configuration.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    /// Analysis passes in priority order for rule ownership.
    #[serde(rename = "pass")]
    pub passes: Vec<Pass>,

    /// Rules that never fail the gate and are omitted from reports.
    #[serde(default)]
    pub exempt: Vec<Pattern>,

    /// Rules that fail the gate on any finding, even if present at the base
    /// revision.
    ///
    /// Takes precedence over `exempt` if a rule matches both lists.
    #[serde(default)]
    pub enforced: Vec<Pattern>,
}

/// Error parsing or validating a [`Policy`].
#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
    #[error("invalid policy: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("the policy names no pass")]
    NoPass,

    #[error("the policy names pass {0:?} more than once")]
    DuplicatePass(String),

    #[error("pass {0:?} is read for no rule")]
    NoRules(String),
}

impl Policy {
    /// Parse and validate a policy file.
    ///
    /// # Errors
    ///
    /// Returns an error if the TOML is invalid, contains unknown keys or
    /// malformed rule patterns, defines no passes, repeats a pass name, or
    /// leaves a pass with no rules.
    pub fn parse(input: &str) -> Result<Self, PolicyError> {
        let policy: Self = toml::from_str(input)?;
        if policy.passes.is_empty() {
            return Err(PolicyError::NoPass);
        }

        let mut names = BTreeSet::new();
        for pass in &policy.passes {
            if !names.insert(pass.name.as_str()) {
                return Err(PolicyError::DuplicatePass(pass.name.clone()));
            }
            if pass.rules.is_empty() {
                return Err(PolicyError::NoRules(pass.name.clone()));
            }
        }

        Ok(policy)
    }

    /// Return the first pass whose rule patterns match `rule`.
    #[must_use]
    pub fn owner(&self, rule: &str) -> Option<&Pass> {
        self.passes
            .iter()
            .find(|pass| pass.rules.iter().any(|pattern| pattern.matches(rule)))
    }

    /// Return `true` if findings for `rule` are included in reports and gating.
    #[must_use]
    pub fn gates(&self, rule: &str) -> bool {
        self.is_enforced(rule) || !self.exempt.iter().any(|pattern| pattern.matches(rule))
    }

    /// Return `true` if `rule` fails the gate regardless of the base report.
    #[must_use]
    pub fn is_enforced(&self, rule: &str) -> bool {
        self.enforced.iter().any(|pattern| pattern.matches(rule))
    }
}

#[cfg(test)]
#[path = "policy_tests.rs"]
mod tests;
