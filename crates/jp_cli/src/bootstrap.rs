//! Pre-workspace bootstrap (RFD 087).
//!
//! Selecting a workspace by ID or path happens *before* a [`Workspace`] exists,
//! so a dedicated bootstrap step owns the pre-workspace resolution: it resolves
//! the launch cwd, selects a concrete checkout root, and derives the working
//! directory for spawned children — once, up front.
//!
//! Consumers (workspace construction, config loading, MCP and plugin spawns,
//! local tools) receive the resolved [`ExecutionContext`] explicitly instead of
//! re-deriving values from the process cwd at each call site, which is what
//! keeps the launch-cwd / root / child-cwd distinction from collapsing.

use std::io;

use camino::{Utf8Path, Utf8PathBuf};
use chrono::Utc;
use crossterm::style::Stylize as _;
use inquire::Select;
use jp_workspace::{
    Id, Workspace,
    roots::{self, RootEntry},
    session::Session,
    session_store::WorkspaceSelection,
};
use tracing::{debug, warn};

use crate::{
    DEFAULT_STORAGE_DIR,
    cmd::{
        self,
        workspace::target::{self as workspace_target, ResolvedTarget, TargetEnv, WorkspaceTarget},
    },
    error::Result,
};

/// What a command needs from the workspace bootstrap.
///
/// The workspace-level analog of a command's conversation load request
/// (`conversation_load_request`): the bootstrap step reads this declaration and
/// only runs workspace resolution when the command asks for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkspaceRequirement {
    /// No workspace is bootstrapped.
    ///
    /// The downstream consumers that assume a root — config loading, MCP and
    /// plugin child cwd, path parsing — simply do not run.
    None,

    /// Resolve and validate a target root, without loading the conversation
    /// index.
    Resolve,

    /// Resolve, construct the [`Workspace`], and load the conversation index.
    Load,
}

/// How the workspace root was selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RootSource {
    /// Derived from the launch cwd by walking up the directory tree.
    Cwd,

    /// An explicit `--workspace <path>` target.
    CliPath,

    /// An explicit `--workspace <id>` target, expanded through the workspace's
    /// roots registry.
    CliId,

    /// An explicit `--workspace` keyword target (`s`, `l`, `?`, free text).
    CliSelector,

    /// The session's active workspace, from the user-global session store (`jp
    /// w use`).
    SessionActive,

    /// The interactive picker fallback, recorded as the session's new active
    /// workspace.
    Picker,
}

/// The bootstrap-resolved execution context.
///
/// A from-anywhere run keeps three directories distinct, which coincide only
/// when JP runs from inside the workspace it operates on:
///
/// - **launch cwd** — where the user invoked `jp`.
/// - **workspace root** — the selected checkout root.
/// - **child cwd** — the working directory spawned children inherit.
#[derive(Debug)]
pub(crate) struct ExecutionContext {
    /// Where the user invoked `jp`.
    ///
    /// The shell completed any relative path argument against this directory,
    /// so user-typed relative paths resolve against it — never against the
    /// workspace root.
    pub(crate) launch_cwd: Utf8PathBuf,

    /// The selected checkout root.
    pub(crate) root: Utf8PathBuf,

    /// The working directory for spawned children, when it differs from the
    /// process cwd.
    ///
    /// See [`Self::child_cwd`].
    child_cwd: Option<Utf8PathBuf>,

    /// How [`Self::root`] was selected.
    pub(crate) source: RootSource,
}

impl ExecutionContext {
    /// The working directory spawned MCP servers, plugins, and local tools
    /// inherit.
    ///
    /// `Some` — holding the workspace root — when JP operates on a workspace
    /// whose root is not the launch cwd's own workspace: children then run as
    /// if launched from the selected root.
    /// `None` when JP runs from inside the selected workspace: children inherit
    /// the process cwd unchanged.
    pub(crate) fn child_cwd(&self) -> Option<&Utf8Path> {
        self.child_cwd.as_deref()
    }

    /// The directory config loading treats as the invocation directory.
    ///
    /// The `$CWD/.jp.{toml,json,yaml}` chain loads from here: the selected
    /// workspace root for a from-anywhere run (config loads as if launched from
    /// there), the launch cwd otherwise (so a subdirectory's `.jp.toml` chain
    /// keeps loading as it does today).
    pub(crate) fn config_cwd(&self) -> &Utf8Path {
        self.child_cwd().unwrap_or(&self.launch_cwd)
    }

    /// An execution context for a test-constructed workspace: as if `jp` was
    /// launched from the workspace root itself.
    #[cfg(test)]
    pub(crate) fn for_workspace(workspace: &Workspace) -> Self {
        Self {
            launch_cwd: workspace.root().to_owned(),
            root: workspace.root().to_owned(),
            child_cwd: None,
            source: RootSource::Cwd,
        }
    }
}

/// Resolve the execution context for this invocation.
///
/// The interactive precedence ladder (RFD 087):
///
/// 1. An explicit `--workspace` target wins.
/// 2. Else a session **sticky** to its active workspace keeps using it, even
///    when the cwd resolves elsewhere.
/// 3. Else, when the cwd and the session-active workspace disagree, a prompt
///    decides — and can record the cwd as the new selection (`C`) or pin the
///    session sticky (`A`).
/// 4. Else the launch cwd's own workspace.
/// 5. Else the session-active workspace, while the workspace still has a live
///    checkout — recovering through surviving checkouts when the recorded one
///    is gone.
/// 6. Else the picker, recorded as the new session-active workspace.
///
/// Non-interactive runs ignore the session layer entirely (steps 2–3 and
/// 5–6), so scripts never depend on hidden per-session state.
pub(crate) fn resolve(
    target: Option<&WorkspaceTarget>,
    session: Option<&Session>,
) -> Result<ExecutionContext> {
    let env = TargetEnv::new(session)?;
    resolve_from(&env, target)
}

/// [`resolve`], with the environment passed explicitly.
fn resolve_from(env: &TargetEnv<'_>, target: Option<&WorkspaceTarget>) -> Result<ExecutionContext> {
    let (root, source) = match target {
        // An explicit target wins.
        Some(target) => match workspace_target::resolve(target, env)? {
            ResolvedTarget::Help => {
                return Err(cmd::Error::from(workspace_target::help()).into());
            }
            // `-w cwd`: resolve from the launch directory — explicitly, so
            // the session layer does not apply.
            ResolvedTarget::Cwd => (cwd_root(env)?, RootSource::Cwd),
            ResolvedTarget::Root(selected) => (selected.root, source_for(target)),
        },

        // No explicit target: the session layer applies only to interactive
        // runs with a session identity — scripts and identity-less sessions
        // resolve from the cwd or error with guidance.
        None => match (env.session, env.interactive) {
            (Some(session), true) => ladder(env, session)?,
            _ => (cwd_root(env)?, RootSource::Cwd),
        },
    };

    // The root-as-working-directory invariant: children run as if launched
    // from the selected root whenever that root is not the launch cwd's own
    // workspace. Runs from inside the selected workspace leave the child cwd
    // untouched.
    let child_cwd = if matches!(source, RootSource::Cwd) {
        None
    } else {
        let launch_root = Workspace::find_root(env.launch_cwd.clone(), DEFAULT_STORAGE_DIR);

        (!launch_root.is_some_and(|launch_root| same_dir(&launch_root, &root)))
            .then(|| root.clone())
    };

    Ok(ExecutionContext {
        launch_cwd: env.launch_cwd.clone(),
        root,
        child_cwd,
        source,
    })
}

/// The `RootSource` an explicit, root-producing target maps to.
fn source_for(target: &WorkspaceTarget) -> RootSource {
    match target {
        WorkspaceTarget::Path(_) => RootSource::CliPath,
        WorkspaceTarget::Id(_) | WorkspaceTarget::Stdin => RootSource::CliId,
        WorkspaceTarget::Session
        | WorkspaceTarget::SessionPicker
        | WorkspaceTarget::Picker
        | WorkspaceTarget::Latest
        | WorkspaceTarget::Fuzzy(_) => RootSource::CliSelector,
        WorkspaceTarget::Cwd | WorkspaceTarget::Help => {
            unreachable!("resolved before source mapping")
        }
    }
}

/// The launch cwd's own workspace, or the no-workspace error.
fn cwd_root(env: &TargetEnv<'_>) -> Result<Utf8PathBuf> {
    Workspace::find_root(env.launch_cwd.clone(), DEFAULT_STORAGE_DIR)
        .ok_or_else(|| no_workspace_error(env))
}

/// The no-target precedence ladder (RFD 087 steps 2–6): sticky pin, conflict
/// prompt, cwd, session-active workspace, picker.
///
/// Only reached interactively with a session identity: a choice that cannot be
/// prompted for or recorded is not made at all.
fn ladder(env: &TargetEnv<'_>, session: &Session) -> Result<(Utf8PathBuf, RootSource)> {
    let cwd = Workspace::find_root(env.launch_cwd.clone(), DEFAULT_STORAGE_DIR);

    let mapping = env.store.load(session);
    let sticky = mapping.as_ref().is_some_and(|mapping| mapping.sticky);
    let active = mapping
        .and_then(|mapping| mapping.history.into_iter().next())
        .and_then(|entry| active_workspace(env, entry));

    match (active, cwd) {
        // Step 2: a sticky session keeps its active workspace, even when the
        // cwd resolves elsewhere.
        (Some(active), _) if sticky => {
            active_root(env, session, active).map(|root| (root, RootSource::SessionActive))
        }

        // Step 3: the cwd and the active workspace disagree — prompt.
        (Some(active), Some(cwd)) if !active.covers(&cwd) => {
            let choice = conflict_choice(&active, &cwd)?;
            apply_conflict_choice(env, session, choice, active, cwd)
        }

        // Step 4: cwd wins when present.
        (_, Some(cwd)) => Ok((cwd, RootSource::Cwd)),

        // Step 5: the session-active workspace, while it has a live checkout.
        (Some(active), None) => {
            active_root(env, session, active).map(|root| (root, RootSource::SessionActive))
        }

        // Step 6: the picker.
        (None, None) => picker(env, session),
    }
}

/// The session's active workspace selection, resolved against liveness.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ActiveWorkspace {
    /// The recorded checkout root is still live.
    Live {
        /// The recorded checkout root.
        root: Utf8PathBuf,
    },

    /// The recorded checkout is gone, but the workspace ID still has live
    /// checkouts to recover through (RFD 087's reprompt).
    Recoverable {
        /// The workspace ID recovery expands.
        id: Id,

        /// The ID's surviving live checkouts.
        candidates: Vec<RootEntry>,
    },
}

impl ActiveWorkspace {
    /// Whether `cwd_root` is a checkout this selection already denotes, so
    /// standing inside the active workspace never prompts.
    fn covers(&self, cwd_root: &Utf8Path) -> bool {
        match self {
            Self::Live { root } => same_dir(root, cwd_root),
            // The recorded checkout is gone; when the cwd is one of the
            // workspace's surviving checkouts, a prompt would offer two
            // spellings of the same recovery answer — cwd wins instead.
            Self::Recoverable { candidates, .. } => candidates
                .iter()
                .any(|entry| same_dir(&entry.path, cwd_root)),
        }
    }

    /// The prompt-facing description of the active side.
    fn display(&self) -> String {
        match self {
            Self::Live { root } => root.to_string(),
            Self::Recoverable { id, candidates } => {
                format!("{id} (recorded checkout gone; {} live)", candidates.len())
            }
        }
    }
}

/// Resolve the session's recorded selection to its live state.
///
/// `None` when nothing usable remains — no parseable ID, or a workspace with
/// no live checkout left — which drops the selection out of the ladder; the
/// source-split cleanup pass prunes the record itself.
fn active_workspace(env: &TargetEnv<'_>, entry: WorkspaceSelection) -> Option<ActiveWorkspace> {
    let Some(id) = entry.id() else {
        debug!(
            root = %entry.root,
            "Session-active record holds an unparseable workspace ID; ignoring it."
        );
        return None;
    };

    if roots::is_live(&entry.root, &id, DEFAULT_STORAGE_DIR) {
        return Some(ActiveWorkspace::Live { root: entry.root });
    }

    // The recorded checkout is gone: recover through the ID's surviving
    // checkouts (RFD 087's reprompt-on-missing-workspace).
    let candidates = roots::resolve_live_roots(&env.workspaces_dir, &id, DEFAULT_STORAGE_DIR);
    if candidates.is_empty() {
        debug!(
            root = %entry.root,
            workspace = %id,
            "Session-active workspace has no live checkout left; falling through."
        );
        return None;
    }

    debug!(
        root = %entry.root,
        workspace = %id,
        "Session-active checkout is gone; recovering through the workspace's surviving checkouts."
    );
    Some(ActiveWorkspace::Recoverable { id, candidates })
}

/// The concrete checkout root of the active workspace.
///
/// A live recorded root is used directly.
/// Recovery uses one surviving checkout directly and prompts among several (RFD
/// 087); the recovered choice repairs the session record, so the next run does
/// not recover again.
fn active_root(
    env: &TargetEnv<'_>,
    session: &Session,
    active: ActiveWorkspace,
) -> Result<Utf8PathBuf> {
    match active {
        ActiveWorkspace::Live { root } => Ok(root),
        ActiveWorkspace::Recoverable { id, candidates } => {
            let selected = workspace_target::select_root(&id, candidates, env.interactive)?;

            if let Err(error) = env
                .store
                .record_selection(session, &id, &selected.root, Utc::now())
            {
                warn!(%error, "Failed to repair the session's active workspace record.");
            }

            Ok(selected.root)
        }
    }
}

/// A resolution of the cwd-vs-active conflict (RFD 087's `[c/C/a/A/q]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConflictChoice {
    /// `c` — use the cwd workspace for this run.
    Current,

    /// `C` — use the cwd workspace and record it as the session's active one.
    CurrentAndSelect,

    /// `a` — use the active workspace for this run.
    Active,

    /// `A` — use the active workspace and pin the session **sticky** to it.
    ActiveAndStick,

    /// `q` — quit without running the command.
    Quit,
}

/// The conflict prompt's rows, mirroring the RFD's `[c/C/a/A/q]` sketch.
const CONFLICT_ROWS: [(&str, ConflictChoice); 5] = [
    ("c - use current workspace", ConflictChoice::Current),
    (
        "C - use current workspace and make it session-active",
        ConflictChoice::CurrentAndSelect,
    ),
    ("a - use active workspace", ConflictChoice::Active),
    (
        "A - use active workspace and keep the session sticky to it",
        ConflictChoice::ActiveAndStick,
    ),
    ("q - quit without running command", ConflictChoice::Quit),
];

/// Prompt for a conflict resolution, writing to stderr.
fn conflict_choice(active: &ActiveWorkspace, cwd: &Utf8Path) -> Result<ConflictChoice> {
    let message = format!(
        "The current directory is workspace `{cwd}`, but this session's active workspace is `{}`. \
         How to proceed?",
        active.display()
    );

    let labels: Vec<&str> = CONFLICT_ROWS.iter().map(|(label, _)| *label).collect();
    let mut writer = io::stderr();
    let selected = Select::new(&message, labels).prompt_with_writer(&mut writer)?;

    Ok(CONFLICT_ROWS
        .iter()
        .find(|(label, _)| *label == selected)
        .expect("selected label came from the list")
        .1)
}

/// Apply a conflict resolution: the run's (root, source), plus the `C` / `A`
/// store effects.
fn apply_conflict_choice(
    env: &TargetEnv<'_>,
    session: &Session,
    choice: ConflictChoice,
    active: ActiveWorkspace,
    cwd: Utf8PathBuf,
) -> Result<(Utf8PathBuf, RootSource)> {
    match choice {
        ConflictChoice::Current => Ok((cwd, RootSource::Cwd)),

        ConflictChoice::CurrentAndSelect => {
            let id = Id::load(cwd.join(DEFAULT_STORAGE_DIR)).and_then(std::result::Result::ok);

            if let Some(id) = id {
                if let Err(error) = env.store.record_selection(session, &id, &cwd, Utc::now()) {
                    warn!(%error, "Failed to record the workspace selection.");
                }
            } else {
                warn!(
                    root = %cwd,
                    "The current workspace has no readable ID; selection not recorded."
                );
            }

            Ok((cwd, RootSource::Cwd))
        }

        ConflictChoice::Active => {
            active_root(env, session, active).map(|root| (root, RootSource::SessionActive))
        }

        ConflictChoice::ActiveAndStick => {
            let root = active_root(env, session, active)?;

            if let Err(error) = env.store.set_sticky(session, true) {
                warn!(%error, "Failed to pin the session to its active workspace.");
            }

            Ok((root, RootSource::SessionActive))
        }

        ConflictChoice::Quit => {
            Err(cmd::Error::from("Aborted: quit without running the command.").into())
        }
    }
}

/// The ladder's last step: pick from every known workspace.
///
/// The choice is recorded as the session's new active workspace: an
/// unrecordable choice would be re-made on every invocation, which is why the
/// session layer requires a session identity at all.
fn picker(env: &TargetEnv<'_>, session: &Session) -> Result<(Utf8PathBuf, RootSource)> {
    let Some(selected) = workspace_target::pick_known_workspace(env, "Select a workspace")? else {
        return Err(no_workspace_error(env));
    };

    if let Some(id) = &selected.id
        && let Err(error) = env
            .store
            .record_selection(session, id, &selected.root, Utc::now())
    {
        warn!(%error, "Failed to record the workspace selection.");
    }

    Ok((selected.root, RootSource::Picker))
}

/// The no-workspace error, with guidance matching how the run fell through:
/// non-interactive runs and identity-less sessions each get their way out.
fn no_workspace_error(env: &TargetEnv<'_>) -> crate::error::Error {
    let jp_init = "jp init".bold().yellow();
    let workspace_flag = "--workspace <id|path>".bold().yellow();

    let message = if !env.interactive {
        format!(
            "Could not locate workspace. Run from inside a workspace, pass `{workspace_flag}`, or \
             create one with `{jp_init}`."
        )
    } else if env.session.is_none() {
        format!(
            "Could not locate workspace, and no session identity is available to select one. Pass \
             `{workspace_flag}`, set $JP_SESSION (or run in a terminal with automatic session \
             detection), or create a workspace with `{jp_init}`."
        )
    } else {
        format!("Could not locate workspace. Use `{jp_init}` to create a new workspace.")
    };

    cmd::Error::from(message).into()
}

/// Whether two directory paths refer to the same location.
///
/// Canonicalizes both sides to tolerate symlinked spellings of the same
/// checkout, falling back to literal equality when either side cannot be
/// canonicalized.
fn same_dir(a: &Utf8Path, b: &Utf8Path) -> bool {
    if a == b {
        return true;
    }

    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
#[path = "bootstrap_tests.rs"]
mod tests;
