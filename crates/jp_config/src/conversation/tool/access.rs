//! Filesystem access grants for a tool.
//!
//! `access.fs` declares which paths a tool may touch and what it may do there.
//! When the section is absent the tool keeps unrestricted (workspace-confined)
//! access; declaring at least one rule switches the tool to default-deny.
//!
//! ```toml
//! [[conversation.tools.fs_modify_file.access.fs]]
//! path = "."
//! read = true
//!
//! [[conversation.tools.fs_modify_file.access.fs]]
//! path = "fork"
//! external = true
//! read = true
//! write = true
//! ```
//!
//! A rule with `external = true` acknowledges that its `path` is a symlink that
//! resolves outside the workspace; the canonical target is approved host-side
//! on first use.

use std::str::FromStr;

use schematic::Config;
use serde::{Deserialize, Serialize};

use crate::{
    BoxedError,
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::PartialConfigDelta,
    internal::merge::vec_with_strategy,
    partial::{ToPartial, partial_opt, partial_opts},
    types::vec::{MergeableVec, vec_to_mergeable_partial},
};

/// Resource access grants for a tool.
///
/// Only filesystem grants (`fs`) are modelled here; network and environment
/// grants are a separate concern.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct AccessConfig {
    /// Filesystem access rules.
    ///
    /// Each rule grants capabilities at a workspace-relative path prefix.
    /// Rules from later config layers append by default; the most specific
    /// (longest-prefix) rule wins for a given target.
    #[setting(
        nested,
        partial_via = MergeableVec::<FsRuleConfig>,
        merge = vec_with_strategy,
    )]
    pub fs: Vec<FsRuleConfig>,
}

impl AssignKeyValue for PartialAccessConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            _ if kv.p("fs") => kv.try_vec_of_nested(self.fs.as_mut())?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialAccessConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            fs: next
                .fs
                .into_iter()
                .filter(|v| !self.fs.contains(v))
                .collect(),
        }
    }
}

impl ToPartial for AccessConfig {
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            fs: vec_to_mergeable_partial(&self.fs),
        }
    }
}

/// A single filesystem access rule.
///
/// Capabilities default to denied.
/// `write` is an alias that expands to `create`, `update`, and `delete`;
/// explicit atomic values override it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Config)]
#[config(rename_all = "snake_case")]
pub struct FsRuleConfig {
    /// Workspace-relative path the rule applies to.
    ///
    /// `"."` matches the entire workspace.
    /// Paths are literal and component-aware: `path = "src"` matches
    /// `src/lib.rs` but not `src_generated/foo.rs`.
    #[setting(required)]
    pub path: String,

    /// Acknowledge that this rule's `path` is permitted to resolve outside the
    /// workspace via a symlink.
    ///
    /// Defaults to `false`.
    /// This is not a capability grant — it only permits the path to resolve
    /// externally.
    /// The canonical target is approved on first use and remembered.
    pub external: Option<bool>,

    /// Grant reading file contents and listing directory entries.
    pub read: Option<bool>,

    /// Alias for `create` + `update` + `delete`.
    pub write: Option<bool>,

    /// Grant creating new files and directories.
    pub create: Option<bool>,

    /// Grant modifying existing files.
    pub update: Option<bool>,

    /// Grant removing files and directories.
    pub delete: Option<bool>,

    /// Grant executing files as programs.
    pub execute: Option<bool>,
}

impl FsRuleConfig {
    /// Whether this rule is a workspace symlink mount (`external = true`).
    #[must_use]
    pub fn is_external(&self) -> bool {
        self.external.unwrap_or(false)
    }

    /// Whether the rule grants create access (falls back to the `write` alias).
    #[must_use]
    pub fn create(&self) -> bool {
        self.create.unwrap_or_else(|| self.write.unwrap_or(false))
    }

    /// Whether the rule grants update access (falls back to the `write` alias).
    #[must_use]
    pub fn update(&self) -> bool {
        self.update.unwrap_or_else(|| self.write.unwrap_or(false))
    }

    /// Whether the rule grants delete access (falls back to the `write` alias).
    #[must_use]
    pub fn delete(&self) -> bool {
        self.delete.unwrap_or_else(|| self.write.unwrap_or(false))
    }
}

impl AssignKeyValue for PartialFsRuleConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "path" => self.path = kv.try_some_string()?,
            "external" => self.external = kv.try_some_bool()?,
            "read" => self.read = kv.try_some_bool()?,
            "write" => self.write = kv.try_some_bool()?,
            "create" => self.create = kv.try_some_bool()?,
            "update" => self.update = kv.try_some_bool()?,
            "delete" => self.delete = kv.try_some_bool()?,
            "execute" => self.execute = kv.try_some_bool()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl FromStr for PartialFsRuleConfig {
    type Err = BoxedError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self {
            path: Some(s.to_owned()),
            ..Default::default()
        })
    }
}

impl ToPartial for FsRuleConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            path: partial_opt(&self.path, defaults.path),
            external: partial_opts(self.external.as_ref(), defaults.external),
            read: partial_opts(self.read.as_ref(), defaults.read),
            write: partial_opts(self.write.as_ref(), defaults.write),
            create: partial_opts(self.create.as_ref(), defaults.create),
            update: partial_opts(self.update.as_ref(), defaults.update),
            delete: partial_opts(self.delete.as_ref(), defaults.delete),
            execute: partial_opts(self.execute.as_ref(), defaults.execute),
        }
    }
}

#[cfg(test)]
#[path = "access_tests.rs"]
mod tests;
