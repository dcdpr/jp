//! Which rules gate, and which rustqual pass each rule is read from.
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

/// The policy file, inside the rustqual config directory.
pub const FILE: &str = "qual.toml";

/// A rule ID, or a prefix of one followed by `*`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern(String);

impl Pattern {
    /// Whether `rule` is the ID this names, or starts with its prefix.
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

/// One rustqual run over the workspace, and the rules it is read for.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pass {
    /// What reports call this pass.
    pub name: String,

    /// The rustqual config the pass runs with, relative to the policy file.
    pub config: String,

    /// The rules this pass is read for.
    ///
    /// A rule is read from the first pass listing a pattern that matches it,
    /// and dropped from every other pass.
    /// That way a rule two passes both report, such as IOSP, which can't be
    /// disabled, is counted once.
    pub rules: Vec<Pattern>,
}

/// The rules that gate, and the passes that find them.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    /// The passes, in the order rule ownership is decided.
    #[serde(rename = "pass")]
    pub passes: Vec<Pass>,

    /// Rules that never gate and are left out of every report.
    #[serde(default)]
    pub exempt: Vec<Pattern>,

    /// Rules with no findings left, where any finding fails, new or not.
    ///
    /// A rule matching both lists is enforced.
    #[serde(default)]
    pub enforced: Vec<Pattern>,
}

/// A policy file that can't be used.
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
    /// Returns an error for invalid TOML, an unknown key, a malformed rule
    /// pattern, no pass, a pass name used twice, or a pass with no rules.
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

    /// The pass `rule` is read from, if any pass is read for it.
    #[must_use]
    pub fn owner(&self, rule: &str) -> Option<&Pass> {
        self.passes
            .iter()
            .find(|pass| pass.rules.iter().any(|pattern| pattern.matches(rule)))
    }

    /// Whether findings of `rule` gate.
    #[must_use]
    pub fn gates(&self, rule: &str) -> bool {
        self.is_enforced(rule) || !self.exempt.iter().any(|pattern| pattern.matches(rule))
    }

    /// Whether any finding of `rule` fails, whether or not it is new.
    #[must_use]
    pub fn is_enforced(&self, rule: &str) -> bool {
        self.enforced.iter().any(|pattern| pattern.matches(rule))
    }
}

#[cfg(test)]
#[path = "policy_tests.rs"]
mod tests;
