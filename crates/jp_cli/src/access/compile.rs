//! Host-side compilation of `access.fs` config rules into a
//! [`jp_tool::AccessPolicy`].
//!
//! Compilation canonicalizes each rule path, runs the approval lifecycle for
//! `external` rules, and bakes the approved canonical target into the compiled
//! [`FsRule`].
//! The cooperative checker and the OS sandbox both consume the resulting
//! policy.

use camino::Utf8Path;
use jp_config::conversation::tool::access::{AccessConfig, FsRuleConfig};
use jp_tool::{AccessPolicy, FsRule, lexical_workspace_relative};
use tracing::warn;

use crate::access::approvals::{ApprovalLookup, ApprovalStore};

/// The decision an approver returns for an external rule's candidate target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    /// The target binding is approved; bake it into the rule.
    Approved,
    /// The target binding is rejected; drop the rule from the policy.
    Rejected,
}

/// Failure reasons for compiling an `access.fs` rule.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CompileError {
    /// A rule path is absolute or escapes the workspace.
    #[error("access rule path is not workspace-relative: {0}")]
    NotWorkspaceRelative(String),

    /// `external = true` on a rule whose path canonicalizes inside the
    /// workspace.
    #[error(
        "access rule '{0}' sets external = true but resolves inside the workspace; declare a rule \
         pointing at the external symlink instead"
    )]
    ExternalInsideWorkspace(String),
}

/// The outcome of compiling a set of `access.fs` rules.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct CompiledFs {
    /// The compiled, approval-baked filesystem rules.
    pub rules: Vec<FsRule>,
    /// Non-fatal compilation warnings (broken symlinks, rejected approvals).
    pub warnings: Vec<String>,
}

/// Compile `access.fs` rules into [`FsRule`]s.
///
/// `root` is the workspace root.
/// `approve` is consulted for each `external` rule whose path resolves outside
/// the workspace, with the rule path and the resolved canonical target; it
/// returns whether the binding is approved.
///
/// # Errors
///
/// Returns [`CompileError`] for a rule path that is not workspace-relative, or
/// `external = true` on a rule that resolves inside the workspace.
pub fn compile_fs(
    config: &AccessConfig,
    root: &Utf8Path,
    mut approve: impl FnMut(&str, &Utf8Path) -> ApprovalDecision,
) -> Result<CompiledFs, CompileError> {
    let root_canonical = root.canonicalize_utf8().unwrap_or_else(|_| root.to_owned());

    let mut compiled = CompiledFs::default();

    for rule in &config.fs {
        let lexical = lexical_workspace_relative(Utf8Path::new(&rule.path))
            .ok_or_else(|| CompileError::NotWorkspaceRelative(rule.path.clone()))?;
        let lexical_str = lexical.as_str().to_owned();

        if !rule.is_external() {
            compiled.rules.push(build_rule(rule, lexical, None));
            continue;
        }

        let joined = root.join(&rule.path);
        let canonical = match joined.canonicalize_utf8() {
            Ok(canonical) => canonical,
            Err(error) => {
                compiled.warnings.push(format!(
                    "dropping external rule '{lexical_str}': cannot resolve target ({error})"
                ));
                continue;
            }
        };

        if canonical.starts_with(&root_canonical) {
            return Err(CompileError::ExternalInsideWorkspace(rule.path.clone()));
        }

        match approve(&lexical_str, &canonical) {
            ApprovalDecision::Approved => {
                compiled
                    .rules
                    .push(build_rule(rule, lexical, Some(canonical)));
            }
            ApprovalDecision::Rejected => {
                compiled.warnings.push(format!(
                    "dropping external rule '{lexical_str}': target binding not approved"
                ));
            }
        }
    }

    Ok(compiled)
}

/// Compile a tool's `access` config into a runtime [`AccessPolicy`], consulting
/// the approval store for external targets (trust-on-first-use).
///
/// Returns `None` when the tool declares no `access` — the tool keeps
/// unrestricted, workspace-confined access.
/// A config that fails to compile (malformed rule path, or `external` resolving
/// inside the workspace) degrades to an empty policy: workspace-confined with
/// no external access, rather than silently granting more.
pub(crate) fn compile_tool_policy(
    access: Option<&AccessConfig>,
    root: &Utf8Path,
    approvals: &ApprovalStore,
) -> Option<AccessPolicy> {
    let config = access?;

    let result = compile_policy(config, root, |rule_path, candidate| {
        match approvals.lookup(rule_path, candidate) {
            ApprovalLookup::Approved => ApprovalDecision::Approved,
            ApprovalLookup::Retargeted { .. } | ApprovalLookup::Unknown => {
                ApprovalDecision::Rejected
            }
        }
    });

    match result {
        Ok((policy, warnings)) => {
            for warning in warnings {
                warn!("{warning}");
            }
            Some(policy)
        }
        Err(error) => {
            warn!(%error, "Failed to compile tool access policy; denying external access.");
            Some(AccessPolicy::default())
        }
    }
}

/// Compile a full [`AccessConfig`] into an [`AccessPolicy`].
///
/// # Errors
///
/// Propagates [`CompileError`] from [`compile_fs`].
pub fn compile_policy(
    config: &AccessConfig,
    root: &Utf8Path,
    approve: impl FnMut(&str, &Utf8Path) -> ApprovalDecision,
) -> Result<(AccessPolicy, Vec<String>), CompileError> {
    let compiled = compile_fs(config, root, approve)?;
    let policy = AccessPolicy {
        fs: compiled.rules,
        ..AccessPolicy::default()
    };
    Ok((policy, compiled.warnings))
}

fn build_rule(
    config: &FsRuleConfig,
    lexical: camino::Utf8PathBuf,
    approved_target: Option<camino::Utf8PathBuf>,
) -> FsRule {
    let mut rule = FsRule::new(lexical)
        .with_external(config.is_external())
        .with_approved_target(approved_target);

    if let Some(read) = config.read {
        rule = rule.with_read(read);
    }
    if let Some(write) = config.write {
        rule = rule.with_write(write);
    }
    if let Some(create) = config.create {
        rule = rule.with_create(create);
    }
    if let Some(update) = config.update {
        rule = rule.with_update(update);
    }
    if let Some(delete) = config.delete {
        rule = rule.with_delete(delete);
    }
    if let Some(execute) = config.execute {
        rule = rule.with_execute(execute);
    }

    rule
}

#[cfg(test)]
#[path = "compile_tests.rs"]
mod tests;
