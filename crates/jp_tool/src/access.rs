//! Access policy types and cooperative filesystem checks.
//!
//! [`AccessPolicy`] is the finalized, host-compiled grant set that travels to a
//! tool inside its [`Context`].
//! Tools call [`Context::check_read`] and friends before touching the
//! filesystem; each check canonicalizes the requested path and evaluates it
//! against the policy's [`FsRule`]s.
//!
//! Two boundaries are enforced, in order:
//!
//! 1. **Pre-canonical** — the path the tool was asked to operate on must be
//!    workspace-relative.
//!    Absolute paths and `..`-escapes are rejected before any filesystem I/O.
//! 2. **Post-canonical** — after resolving symlinks, a target that lands
//!    outside the workspace is rejected unless the matching rule is `external`
//!    and the resolved target stays under the rule's approved target.
//!
//! The host (JP) builds [`FsRule`]s with [`FsRule::new`] and the `with_*`
//! builders; tools only read them through the accessors.
//!
//! [`Context::check_read`]: crate::Context::check_read
//! [`Context`]: crate::Context

use camino::{Utf8Component, Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

/// The finalized access grant set for a single tool invocation.
///
/// This is the wire form: the host merges configuration layers, compiles rule
/// paths, and bakes approved external targets into [`FsRule`]s before
/// serializing the policy into the tool's [`Context`].
/// Tools never see the configuration-layer types.
///
/// An empty `fs` list means the tool has unrestricted (but still
/// workspace-confined) filesystem access.
/// A non-empty list switches filesystem access to default-deny: only paths
/// matched by a rule that grants the requested capability are allowed.
///
/// [`Context`]: crate::Context
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccessPolicy {
    /// Filesystem grants.
    #[serde(default)]
    pub fs: Vec<FsRule>,

    /// Network grants.
    #[serde(default)]
    pub net: Vec<NetRule>,

    /// Environment-variable grants.
    #[serde(default)]
    pub env: Vec<EnvRule>,
}

/// A filesystem capability a tool may request on a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    /// Read file contents or list directory entries.
    Read,
    /// Create a new file or directory.
    Create,
    /// Modify an existing file.
    Update,
    /// Remove a file or directory.
    Delete,
    /// Execute a file as a program.
    Execute,
}

impl Capability {
    /// A human-readable name for error messages.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Create => "create",
            Self::Update => "update",
            Self::Delete => "delete",
            Self::Execute => "execute",
        }
    }
}

impl AccessPolicy {
    /// Whether filesystem access is restricted (default-deny).
    ///
    /// An empty `fs` list is unrestricted; any rule switches to default-deny.
    #[must_use]
    pub fn is_restricted(&self) -> bool {
        !self.fs.is_empty()
    }

    /// The most specific fs rule matching a workspace-relative lexical path,
    /// breaking ties toward the rule declared last.
    #[must_use]
    pub fn matching_fs_rule(&self, lexical: &Utf8Path) -> Option<&FsRule> {
        find_matching_rule(&self.fs, lexical)
    }

    /// Whether the policy permits `capability` on a workspace-relative lexical
    /// path.
    ///
    /// Unrestricted policies permit everything; restricted policies permit only
    /// what a matching rule grants (default-deny).
    #[must_use]
    pub fn permits(&self, capability: Capability, lexical: &Utf8Path) -> bool {
        if !self.is_restricted() {
            return true;
        }
        match self.matching_fs_rule(lexical) {
            Some(rule) => match capability {
                Capability::Read => rule.read(),
                Capability::Create => rule.create(),
                Capability::Update => rule.update(),
                Capability::Delete => rule.delete(),
                Capability::Execute => rule.execute(),
            },
            None => false,
        }
    }

    /// The workspace-relative grant paths, for building helpful error messages.
    pub fn grant_paths(&self) -> impl Iterator<Item = &Utf8Path> {
        self.fs.iter().map(FsRule::lexical_path)
    }
}

/// A compiled filesystem grant.
///
/// The fields are private to keep the `write` alias and the approved-target
/// boundary from being read inconsistently.
/// The host constructs rules with [`FsRule::new`] plus the `with_*` builders;
/// consumers read capabilities through the accessor methods, which expand the
/// `write` alias.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FsRule {
    /// Workspace-relative path the rule applies to, normalized lexically (`..`
    /// collapsed) with the rule's own symlink left unresolved.
    ///
    /// Tool-call targets are matched against this with a component-aware
    /// longest-prefix match.
    lexical_path: Utf8PathBuf,

    /// Whether the rule's path is permitted to resolve outside the workspace.
    #[serde(default)]
    external: bool,

    /// Canonical absolute target that external resolution is permitted to land
    /// in.
    ///
    /// `Some` only for approved `external` rules; the resolved target must stay
    /// under this path.
    /// `None` for ordinary workspace-anchored rules.
    #[serde(default)]
    approved_target: Option<Utf8PathBuf>,

    #[serde(default)]
    read: Option<bool>,
    #[serde(default)]
    write: Option<bool>,
    #[serde(default)]
    create: Option<bool>,
    #[serde(default)]
    update: Option<bool>,
    #[serde(default)]
    delete: Option<bool>,
    #[serde(default)]
    execute: Option<bool>,
}

impl FsRule {
    /// Create a rule at the given workspace-relative lexical path with all
    /// capabilities denied and no external resolution.
    #[must_use]
    pub fn new(lexical_path: impl Into<Utf8PathBuf>) -> Self {
        Self {
            lexical_path: lexical_path.into(),
            external: false,
            approved_target: None,
            read: None,
            write: None,
            create: None,
            update: None,
            delete: None,
            execute: None,
        }
    }

    /// Mark the rule as permitted to resolve outside the workspace.
    #[must_use]
    pub fn with_external(mut self, external: bool) -> Self {
        self.external = external;
        self
    }

    /// Set the approved canonical target for an external rule.
    #[must_use]
    pub fn with_approved_target(mut self, target: Option<Utf8PathBuf>) -> Self {
        self.approved_target = target;
        self
    }

    /// Set the `read` capability.
    #[must_use]
    pub fn with_read(mut self, read: bool) -> Self {
        self.read = Some(read);
        self
    }

    /// Set the `write` alias (expands to `create`, `update`, `delete`).
    #[must_use]
    pub fn with_write(mut self, write: bool) -> Self {
        self.write = Some(write);
        self
    }

    /// Set the `create` capability explicitly, overriding the `write` alias.
    #[must_use]
    pub fn with_create(mut self, create: bool) -> Self {
        self.create = Some(create);
        self
    }

    /// Set the `update` capability explicitly, overriding the `write` alias.
    #[must_use]
    pub fn with_update(mut self, update: bool) -> Self {
        self.update = Some(update);
        self
    }

    /// Set the `delete` capability explicitly, overriding the `write` alias.
    #[must_use]
    pub fn with_delete(mut self, delete: bool) -> Self {
        self.delete = Some(delete);
        self
    }

    /// Set the `execute` capability.
    #[must_use]
    pub fn with_execute(mut self, execute: bool) -> Self {
        self.execute = Some(execute);
        self
    }

    /// The workspace-relative lexical path the rule matches against.
    #[must_use]
    pub fn lexical_path(&self) -> &Utf8Path {
        &self.lexical_path
    }

    /// Whether the rule is permitted to resolve outside the workspace.
    #[must_use]
    pub const fn external(&self) -> bool {
        self.external
    }

    /// The approved canonical target for an external rule, if any.
    #[must_use]
    pub fn approved_target(&self) -> Option<&Utf8Path> {
        self.approved_target.as_deref()
    }

    /// Whether the rule grants read access.
    #[must_use]
    pub fn read(&self) -> bool {
        self.read.unwrap_or(false)
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

    /// Whether the rule grants execute access.
    #[must_use]
    pub fn execute(&self) -> bool {
        self.execute.unwrap_or(false)
    }
}

/// A network grant, matched against parsed URIs.
///
/// Carried in [`AccessPolicy`] for completeness; cooperative evaluation of net
/// rules is not part of the filesystem check surface.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetRule {
    /// The host the rule applies to.
    pub host: String,
    #[serde(default)]
    pub scheme: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub path_prefix: Option<String>,
    #[serde(default)]
    pub allow: bool,
}

/// An environment-variable grant.
///
/// Carried in [`AccessPolicy`] for completeness; cooperative evaluation of env
/// rules is not part of the filesystem check surface.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvRule {
    /// The variable name (a trailing `*` marks a prefix match).
    pub name: String,
    #[serde(default)]
    pub read: bool,
}

/// Failure reasons for a filesystem access check.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum FsAccessError {
    /// The requested path is absolute; tool calls must be workspace-relative.
    #[error("absolute paths are not permitted: {0}")]
    Absolute(Utf8PathBuf),

    /// The requested path lexically escapes the workspace via `..`.
    #[error("path escapes the workspace: {0}")]
    InputEscape(Utf8PathBuf),

    /// The resolved (canonical) target lands outside the workspace and no
    /// `external` rule permits it.
    #[error("path resolves outside the workspace: {0}")]
    Escape(Utf8PathBuf),

    /// No rule grants the requested capability on the target.
    #[error("access denied: {capability} on {target}")]
    Denied {
        /// The capability that was requested.
        capability: &'static str,
        /// The workspace-relative target the capability was requested for.
        target: Utf8PathBuf,
        /// The configured grant paths, for a helpful error message.
        grants: Vec<Utf8PathBuf>,
    },

    /// Resolving the path failed (e.g. an unreadable ancestor directory).
    #[error("failed to resolve path {path}: {message}")]
    Io {
        /// The path that failed to resolve.
        path: Utf8PathBuf,
        /// The underlying I/O error message.
        message: String,
    },
}

/// Outcome of resolving and matching a target path against the policy.
struct Resolved<'a> {
    /// The resolved absolute path (symlinks followed on existing ancestors).
    canonical: Utf8PathBuf,
    /// Whether the canonical path is inside the workspace root.
    inside: bool,
    /// The matching rule, if any.
    rule: Option<&'a FsRule>,
}

impl crate::Context {
    /// Check read access to `input`, returning the resolved absolute path.
    ///
    /// `input` must be workspace-relative; the check joins it against the
    /// workspace root and canonicalizes it itself.
    /// Do not pre-resolve paths against the root before calling.
    pub fn check_read(&self, input: &Utf8Path) -> Result<Utf8PathBuf, FsAccessError> {
        self.check_capability(input, "read", FsRule::read)
    }

    /// Check create access to `input`, returning the resolved absolute path.
    pub fn check_create(&self, input: &Utf8Path) -> Result<Utf8PathBuf, FsAccessError> {
        self.check_capability(input, "create", FsRule::create)
    }

    /// Check update access to `input`, returning the resolved absolute path.
    pub fn check_update(&self, input: &Utf8Path) -> Result<Utf8PathBuf, FsAccessError> {
        self.check_capability(input, "update", FsRule::update)
    }

    /// Check delete access to `input`, returning the resolved absolute path.
    pub fn check_delete(&self, input: &Utf8Path) -> Result<Utf8PathBuf, FsAccessError> {
        self.check_capability(input, "delete", FsRule::delete)
    }

    /// Check execute access to `input`, returning the resolved absolute path.
    pub fn check_execute(&self, input: &Utf8Path) -> Result<Utf8PathBuf, FsAccessError> {
        self.check_capability(input, "execute", FsRule::execute)
    }

    fn check_capability(
        &self,
        input: &Utf8Path,
        capability: &'static str,
        granted: fn(&FsRule) -> bool,
    ) -> Result<Utf8PathBuf, FsAccessError> {
        // Pre-canonical invariant: the tool may only express workspace-relative
        // paths. Reject absolute paths and `..`-escapes before any I/O so the
        // LLM's expressible reach is decoupled from the on-disk layout.
        if input.is_absolute() {
            return Err(FsAccessError::Absolute(input.to_owned()));
        }
        let lexical =
            normalize_lexical(input).ok_or_else(|| FsAccessError::InputEscape(input.to_owned()))?;

        let rules = self.access.as_ref().map_or(&[][..], |a| a.fs.as_slice());
        let restricted = !rules.is_empty();

        let resolved = self.resolve(&lexical, rules)?;
        let Resolved {
            canonical,
            inside,
            rule,
        } = resolved;

        // Post-canonical boundary: a target that escapes the workspace is only
        // allowed through an approved external rule whose approved target
        // contains the resolved path (nested-escape boundary).
        if !inside {
            let permitted = rule.is_some_and(|r| {
                r.external()
                    && r.approved_target()
                        .is_some_and(|target| canonical.starts_with(target))
            });
            if !permitted {
                return Err(FsAccessError::Escape(canonical));
            }
        }

        if !restricted {
            return Ok(canonical);
        }

        match rule {
            Some(rule) if granted(rule) => Ok(canonical),
            _ => Err(FsAccessError::Denied {
                capability,
                target: lexical,
                grants: rules.iter().map(|r| r.lexical_path.clone()).collect(),
            }),
        }
    }

    fn resolve<'a>(
        &self,
        lexical: &Utf8Path,
        rules: &'a [FsRule],
    ) -> Result<Resolved<'a>, FsAccessError> {
        let root_canonical = self
            .root
            .canonicalize_utf8()
            .map_err(|e| FsAccessError::Io {
                path: self.root.clone(),
                message: e.to_string(),
            })?;

        let canonical =
            canonicalize_target(&root_canonical, lexical).map_err(|e| FsAccessError::Io {
                path: self.root.join(lexical),
                message: e.to_string(),
            })?;

        let inside = canonical.starts_with(&root_canonical);
        let rule = find_matching_rule(rules, lexical);

        Ok(Resolved {
            canonical,
            inside,
            rule,
        })
    }
}

/// Normalize a relative path lexically, collapsing `.` and `..` without
/// touching the filesystem.
///
/// Returns `None` if the path is absolute or if `..` segments escape above the
/// workspace root.
/// An empty result represents the workspace root itself.
///
/// The host uses [`lexical_workspace_relative`] to compile rule paths into the
/// same lexical form tool calls are matched against.
#[must_use]
pub fn lexical_workspace_relative(input: &Utf8Path) -> Option<Utf8PathBuf> {
    normalize_lexical(input)
}

fn normalize_lexical(input: &Utf8Path) -> Option<Utf8PathBuf> {
    let mut out: Vec<&str> = Vec::new();
    for component in input.components() {
        match component {
            Utf8Component::CurDir => {}
            Utf8Component::Normal(segment) => out.push(segment),
            Utf8Component::ParentDir => {
                out.pop()?;
            }
            Utf8Component::RootDir | Utf8Component::Prefix(_) => return None,
        }
    }

    let mut path = Utf8PathBuf::new();
    for segment in out {
        path.push(segment);
    }
    Some(path)
}

/// Resolve `lexical` against the canonical workspace root, following symlinks
/// on the deepest existing ancestor and re-appending any not-yet-created
/// suffix.
fn canonicalize_target(
    root_canonical: &Utf8Path,
    lexical: &Utf8Path,
) -> std::io::Result<Utf8PathBuf> {
    let full = root_canonical.join(lexical);

    let mut existing = full.clone();
    let mut suffix: Vec<String> = Vec::new();
    while !existing.exists() {
        match existing.file_name() {
            Some(name) => {
                suffix.push(name.to_owned());
                // The root always exists, so a missing path always has a
                // parent we can step back to.
                existing = existing
                    .parent()
                    .map_or_else(|| root_canonical.to_owned(), Utf8Path::to_owned);
            }
            None => break,
        }
    }

    let mut canonical = existing.canonicalize_utf8()?;
    for segment in suffix.into_iter().rev() {
        canonical.push(segment);
    }
    Ok(canonical)
}

/// Find the most specific rule matching `target`, breaking ties toward the rule
/// declared last (matching the append-merge precedence of config layers).
fn find_matching_rule<'a>(rules: &'a [FsRule], target: &Utf8Path) -> Option<&'a FsRule> {
    let mut best: Option<(usize, usize)> = None;
    for (index, rule) in rules.iter().enumerate() {
        let Some(specificity) = prefix_specificity(rule.lexical_path(), target) else {
            continue;
        };
        match best {
            Some((best_spec, _)) if specificity < best_spec => {}
            _ => best = Some((specificity, index)),
        }
    }
    best.map(|(_, index)| &rules[index])
}

/// If `rule_path` is a component-wise prefix of `target`, return its component
/// count (its specificity).
/// The empty path (workspace root) matches everything.
fn prefix_specificity(rule_path: &Utf8Path, target: &Utf8Path) -> Option<usize> {
    let rule_components: Vec<&str> = path_segments(rule_path);
    let target_components: Vec<&str> = path_segments(target);
    if rule_components.len() > target_components.len() {
        return None;
    }
    rule_components
        .iter()
        .zip(&target_components)
        .all(|(a, b)| a == b)
        .then_some(rule_components.len())
}

/// Return the `Normal` segments of a path, ignoring `.` and treating `"."` or
/// `""` as the workspace root (no segments).
fn path_segments(path: &Utf8Path) -> Vec<&str> {
    path.components()
        .filter_map(|component| match component {
            Utf8Component::Normal(segment) => Some(segment),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_collapses_dot_and_parent() {
        assert_eq!(
            normalize_lexical(Utf8Path::new("a/./b/../c")),
            Some(Utf8PathBuf::from("a/c"))
        );
        assert_eq!(
            normalize_lexical(Utf8Path::new(".")),
            Some(Utf8PathBuf::new())
        );
        assert_eq!(
            normalize_lexical(Utf8Path::new("a/..")),
            Some(Utf8PathBuf::new())
        );
    }

    #[test]
    fn normalize_rejects_escape_and_absolute() {
        assert_eq!(normalize_lexical(Utf8Path::new("../a")), None);
        assert_eq!(normalize_lexical(Utf8Path::new("a/../../b")), None);
        assert_eq!(normalize_lexical(Utf8Path::new("/etc/passwd")), None);
    }

    #[test]
    fn prefix_match_is_component_aware() {
        assert_eq!(
            prefix_specificity(Utf8Path::new("src"), Utf8Path::new("src/lib.rs")),
            Some(1)
        );
        // `src` must not prefix-match `src_generated`.
        assert_eq!(
            prefix_specificity(Utf8Path::new("src"), Utf8Path::new("src_generated/x")),
            None
        );
        // The root matches everything.
        assert_eq!(
            prefix_specificity(Utf8Path::new(""), Utf8Path::new("any/where")),
            Some(0)
        );
    }

    #[test]
    fn longest_prefix_wins_then_last() {
        let rules = vec![
            FsRule::new("").with_read(true),
            FsRule::new("src").with_read(false),
            FsRule::new("src").with_read(true),
        ];
        // Two `src` rules tie on specificity; the last declared wins.
        let rule = find_matching_rule(&rules, Utf8Path::new("src/lib.rs")).unwrap();
        assert!(rule.read());

        // The root rule wins for an unmatched subtree.
        let rule = find_matching_rule(&rules, Utf8Path::new("tests/x.rs")).unwrap();
        assert!(rule.read());
    }

    #[test]
    fn write_alias_expands() {
        let rule = FsRule::new("x").with_write(true);
        assert!(rule.create() && rule.update() && rule.delete());
        assert!(!rule.read() && !rule.execute());

        let rule = FsRule::new("x").with_write(true).with_delete(false);
        assert!(rule.create() && rule.update() && !rule.delete());
    }
}
