//! `--mount`: external paths linked into the workspace, and the access grants
//! that let tools reach them.
//!
//! A mount takes effect in two stages.
//! [`apply_mounts`] writes the access grants into the partial config while the
//! configuration is resolved.
//! Like any other change the query's flags make, the grants are recorded in the
//! conversation's config, so later turns keep them without repeating `--mount`.
//! [`create_mount_effects`] creates the symlink in the workspace and records
//! its approval in user-local storage once the conversation is locked.
//! Both stay after the query ends.

use std::{env, fs};

use camino::{Utf8Path, Utf8PathBuf};
use chrono::{DateTime, Utc};
use jp_config::{
    PartialAppConfig,
    conversation::tool::{
        PartialEnableConfig, PartialToolConfig, PartialToolsConfig, ToolSource,
        access::{AccessConfig, PartialAccessConfig, PartialFsRuleConfig},
    },
    fs::expand_tilde,
};
use jp_storage::backend::FsStorageBackend;
use jp_workspace::Workspace;
use tracing::warn;

use super::config::{BoxedResult, effective_enable};
use crate::{
    access::{
        approvals::{APPROVALS_FILE, ApprovalLookup, ApprovalStore},
        compile::{ApprovalDecision, compile_policy},
        mount::{MountMode, MountSpec, absolute_target},
    },
    error::{Error, Result},
};

/// A resolved mount and the tools it grants access to (stage 1 planning).
struct MountPlan {
    rule_path: String,
    write: bool,
    /// (tool name, grants to seed its own `access` block with)
    targets: Vec<(String, PartialAccessConfig)>,
}

/// Inject `--mount` access grants into the partial config (stage 1).
///
/// Pure config mutation: one `access.fs` rule per in-scope tool.
/// The symlink is not required to exist yet; it is created later by
/// [`create_mount_effects`].
/// A tool that had no grants of its own is seeded first, so the mount doesn't
/// change what else the tool can reach (see [`mount_access_seed`]).
pub(super) fn apply_mounts(
    partial: &mut PartialAppConfig,
    mounts: &[String],
    workspace: Option<&Workspace>,
    merged_config: Option<&PartialAppConfig>,
) -> BoxedResult<()> {
    if mounts.is_empty() {
        return Ok(());
    }

    let workspace = workspace.ok_or("`--mount` requires a workspace")?;
    let cwd = current_dir_utf8()?;

    inject_mounts(
        partial,
        mounts,
        workspace.root(),
        &cwd,
        existing_mount_target(&cwd),
        merged_config,
    )
}

/// The pure half of [`apply_mounts`]: add the grants `mounts` name to
/// `partial`, resolving each `NAME` against `cwd` inside `root`.
///
/// `resolve` answers whether a bare `PATH` exists (see [`MountSpec::parse`]).
fn inject_mounts(
    partial: &mut PartialAppConfig,
    mounts: &[String],
    root: &Utf8Path,
    cwd: &Utf8Path,
    resolve: impl Fn(&str) -> Option<Utf8PathBuf>,
    merged_config: Option<&PartialAppConfig>,
) -> BoxedResult<()> {
    // Resolve the tool set and the global enable default from the merged
    // config (the fully-layered view) so a bare mount expands over the tools
    // actually enabled in the resolved config, honoring `*` defaults.
    let tools_config = merged_config.map_or(&partial.conversation.tools, |v| &v.conversation.tools);
    let default_enable = tools_config.defaults.enable.clone().unwrap_or_default();

    let mut plans = Vec::new();
    for spec in mounts {
        let spec = MountSpec::parse(spec, &resolve)?;
        let rule_path = spec.resolve_name(cwd, root)?.as_str().to_owned();

        let targets = match &spec.tool {
            Some(tool) => vec![(tool.clone(), mount_access_seed(tools_config, tool))],
            None => tools_config
                .tools
                .iter()
                .filter(|(_, cfg)| is_enabled_subprocess(cfg, &default_enable))
                .map(|(name, _)| (name.clone(), mount_access_seed(tools_config, name)))
                .collect(),
        };

        plans.push(MountPlan {
            rule_path,
            write: spec.mode == MountMode::Rw,
            targets,
        });
    }

    for plan in plans {
        for (tool, seed) in plan.targets {
            let cfg = partial.conversation.tools.tools.entry(tool).or_default();
            let access = cfg.access.get_or_insert_with(PartialAccessConfig::default);

            let already_present = access
                .fs
                .iter()
                .any(|rule| rule.path.as_deref() == Some(plan.rule_path.as_str()));
            if already_present {
                continue;
            }

            // Seed once per tool: a second mount for the same tool finds the
            // block already carrying rules and just appends to it.
            if access.fs.is_empty() && access.env.is_empty() {
                access.fs.extend(seed.fs);
                access.env.extend(seed.env);
            }

            access
                .fs
                .push(mount_partial_rule(&plan.rule_path, plan.write));
        }
    }

    Ok(())
}

/// Create the symlinks and seed the approval store for `--mount` flags (stage
/// 2).
pub(super) fn create_mount_effects(
    mounts: &[String],
    workspace: &Workspace,
    fs_backend: Option<&FsStorageBackend>,
    now: DateTime<Utc>,
) -> Result<()> {
    if mounts.is_empty() {
        return Ok(());
    }

    let root = workspace.root().to_owned();
    let root_canonical = root.canonicalize_utf8().unwrap_or_else(|_| root.clone());
    let cwd = current_dir_utf8().map_err(|e| Error::CliConfig(e.to_string()))?;

    let approvals_path = approval_store_path(fs_backend);
    let mut store = approvals_path
        .as_deref()
        .map_or_default(ApprovalStore::load);

    let resolve = existing_mount_target(&cwd);
    let mut rules = Vec::new();
    for spec in mounts {
        let spec = MountSpec::parse(spec, &resolve).map_err(|e| Error::CliConfig(e.to_string()))?;
        let rule_path = spec
            .resolve_name(&cwd, &root)
            .map_err(|e| Error::CliConfig(e.to_string()))?;
        let link = root.join(&rule_path);

        let target = expand_tilde(&spec.path, env::var("HOME").ok())
            .unwrap_or_else(|| Utf8PathBuf::from(&spec.path));

        // Resolve the target before creating the link so a missing target
        // fails cleanly instead of leaving a broken symlink behind.
        let canonical = target.canonicalize_utf8().map_err(|e| {
            Error::CliConfig(format!("mount target '{target}' cannot be resolved: {e}"))
        })?;

        // An external mount must point outside the workspace. Reject an
        // in-workspace target before any side effect, so a rejected mount
        // leaves no symlink or approval entry behind.
        if canonical.starts_with(&root_canonical) {
            return Err(Error::CliConfig(format!(
                "mount target '{target}' is inside the workspace; mounts are for external paths"
            )));
        }

        // Link to the canonical absolute target so the symlink resolves the
        // same regardless of where it sits and matches the recorded approval
        // (a relative target would resolve against the link's parent instead).
        create_workspace_symlink(&link, &canonical)?;

        // A mount of an ancestor is allowed: tools re-entering the workspace
        // through it are checked against the workspace's own rules. It does
        // put a symlink cycle in the tree. The equal case was rejected above.
        if root_canonical.starts_with(&canonical) {
            warn!(
                mount = %rule_path,
                target = %canonical,
                workspace = %root_canonical,
                tip = "Filesystem tools that follow symlinks may find workspace files twice.",
                "Mount target contains the workspace."
            );
        }

        store.record(rule_path.as_str(), canonical, now);
        rules.push(spec.rule(rule_path.as_str()));
    }

    if let Some(path) = approvals_path {
        store.save(&path)?;
    }

    // Compile the just-created mounts against the seeded approvals to confirm
    // they resolve to a usable policy, surfacing broken or unapproved targets.
    let access = AccessConfig {
        fs: rules,
        env: vec![],
    };
    let (_, warnings) = compile_policy(&access, &root, |rule_path, candidate| {
        match store.lookup(rule_path, candidate) {
            ApprovalLookup::Approved => ApprovalDecision::Approved,
            ApprovalLookup::Retargeted { .. } | ApprovalLookup::Unknown => {
                ApprovalDecision::Rejected
            }
        }
    })
    .map_err(|e| Error::CliConfig(e.to_string()))?;

    for warning in warnings {
        warn!("{warning}");
    }

    Ok(())
}

/// Create a workspace symlink at `link` pointing to `target`.
///
/// A symlink that already resolves to the same target is a no-op; one resolving
/// elsewhere, or a non-symlink at `link`, is an error.
fn create_workspace_symlink(link: &Utf8Path, target: &Utf8Path) -> Result<()> {
    if link.is_symlink() {
        // Compare resolved targets rather than the raw link text, so a relative
        // and an absolute link to the same place are treated as identical.
        let same = link
            .canonicalize_utf8()
            .ok()
            .zip(target.canonicalize_utf8().ok())
            .is_some_and(|(existing, wanted)| existing == wanted);
        if same {
            return Ok(());
        }
        return Err(Error::CliConfig(format!(
            "mount '{link}' already exists as a symlink pointing elsewhere; choose another name \
             with NAME=PATH"
        )));
    }

    if link.exists() {
        return Err(Error::CliConfig(format!(
            "cannot create mount: '{link}' already exists and is not a symlink; choose another \
             name with NAME=PATH"
        )));
    }

    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent)?;
    }

    #[cfg(unix)]
    std::os::unix::fs::symlink(target.as_std_path(), link.as_std_path())?;

    #[cfg(windows)]
    if target.is_dir() {
        std::os::windows::fs::symlink_dir(target.as_std_path(), link.as_std_path())?;
    } else {
        std::os::windows::fs::symlink_file(target.as_std_path(), link.as_std_path())?;
    }

    Ok(())
}

/// Resolve the path to the user-local approval store, if user storage exists.
fn approval_store_path(fs_backend: Option<&FsStorageBackend>) -> Option<Utf8PathBuf> {
    fs_backend
        .and_then(|fs| fs.user_storage_with_path(relative_path::RelativePath::new(APPROVALS_FILE)))
}

/// Load the approval store, treating missing/in-memory storage as empty.
pub(super) fn load_approval_store(fs_backend: Option<&FsStorageBackend>) -> ApprovalStore {
    approval_store_path(fs_backend)
        .as_deref()
        .map_or_default(ApprovalStore::load)
}

/// Resolve a mount `PATH` as typed against `cwd`, if something exists there.
///
/// Existence follows symlinks, so a dangling symlink counts as missing.
fn existing_mount_target(cwd: &Utf8Path) -> impl Fn(&str) -> Option<Utf8PathBuf> + '_ {
    let home = env::var("HOME").ok();
    move |path| absolute_target(path, cwd, home.as_deref()).filter(|target| target.exists())
}

fn current_dir_utf8() -> BoxedResult<Utf8PathBuf> {
    let cwd = env::current_dir()?;
    Utf8PathBuf::from_path_buf(cwd)
        .map_err(|path| format!("current directory is not valid UTF-8: {}", path.display()).into())
}

/// The grants a tool's own `access` block is seeded with before a mount rule is
/// appended to it.
///
/// Writing a rule into a tool's own scope detaches that tool from
/// `[conversation.tools.'*'.access]`, which applies whole or not at all (RFD
/// 076), and a rule in one resource list switches only that list to
/// default-deny.
/// The seed carries the tool's prior reach across both of those edges:
///
/// - A tool that declared no grants of its own gets a copy of the `'*'` block,
///   `env` rules included.
///   Copying only `fs` would hand back unrestricted environment access to a
///   tool whose `env` rules came from `'*'`.
/// - When the scope that applied granted no `fs` rules at all, the tool had
///   unrestricted filesystem access, and the mount rule on its own would end
///   it.
///   A workspace-wide rule stands in for what the tool already had.
///
/// A tool that declared `fs` rules of its own needs no seed and gets none:
/// those rules are already in the config, and the mount rule appends to them.
fn mount_access_seed(tools: &PartialToolsConfig, name: &str) -> PartialAccessConfig {
    let own = tools
        .tools
        .get(name)
        .and_then(|cfg| cfg.access.as_ref())
        .filter(|access| declares_rules(access));

    let effective = own.or_else(|| {
        tools
            .defaults
            .access
            .as_ref()
            .filter(|access| declares_rules(access))
    });

    // Only the `'*'` block needs carrying: a tool's own rules are already in
    // the config, and copying them would duplicate every one of them.
    let mut seed = match effective {
        Some(block) if own.is_none() => block.clone(),
        _ => PartialAccessConfig::default(),
    };

    if effective.is_none_or(|block| block.fs.is_empty()) {
        seed.fs.push(workspace_default_partial_rule());
    }

    seed
}

/// Whether an `access` block holds a rule of any resource type.
///
/// Mirrors [`AccessConfig::is_empty`] on the resolved side: a block with no
/// rules at all is not a policy, so it neither counts as the tool's own choice
/// nor is worth seeding.
fn declares_rules(access: &PartialAccessConfig) -> bool {
    !access.fs.is_empty() || !access.env.is_empty()
}

/// Whether a partial tool config is an enabled tool that runs as a subprocess
/// JP starts: a local command or a command plugin.
///
/// The tool's own `enable` takes precedence over the global `*` default; a tool
/// whose effective `state` resolves to `false` is not part of a bare mount's
/// scope.
fn is_enabled_subprocess(cfg: &PartialToolConfig, default_enable: &PartialEnableConfig) -> bool {
    cfg.source.as_ref().is_some_and(ToolSource::is_subprocess)
        && effective_enable(cfg.enable.as_ref(), default_enable).state
}

/// The workspace-default rule injected to preserve a tool's prior implicit
/// workspace access.
fn workspace_default_partial_rule() -> PartialFsRuleConfig {
    PartialFsRuleConfig {
        path: Some(".".to_owned()),
        read: Some(true),
        write: Some(true),
        ..PartialFsRuleConfig::default()
    }
}

/// The `access.fs` rule a mount injects.
fn mount_partial_rule(rule_path: &str, write: bool) -> PartialFsRuleConfig {
    PartialFsRuleConfig {
        path: Some(rule_path.to_owned()),
        external: Some(true),
        read: Some(true),
        write: Some(write),
        ..PartialFsRuleConfig::default()
    }
}

#[cfg(test)]
#[path = "mounts_tests.rs"]
mod tests;
