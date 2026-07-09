//! The workspace targeting grammar (RFD 087).
//!
//! `jp w use`, `jp w show`, and the global `--workspace` flag share one
//! grammar, modeled on `ConversationTarget` (`jp_cli::cmd::target`) with the
//! keywords that carry over to workspaces:
//!
//! | Target           | Meaning                                              |
//! | ---------------- | ---------------------------------------------------- |
//! | `<id>`           | a literal workspace ID                               |
//! | `<path>`         | an existing path (shadows an ID of the same name)    |
//! | free text        | fuzzy-match known workspaces by slug / path / ID     |
//! | `?`              | pick from all known workspaces                       |
//! | `?s`, `?session` | pick from this session's workspace history           |
//! | `s`, `session`   | the previously active workspace (like `cd -`)        |
//! | `l`, `latest`    | the most recently used known workspace               |
//! | `cwd`, `.`       | the cwd-derived workspace (as a `use` target: clear) |
//! | `-`              | read a workspace ID from stdin                       |
//! | `help`           | print keyword help                                   |
//!
//! Keywords are matched before paths, so a literal directory named like a
//! keyword needs a path spelling (`./s`).
//! Session-derived targets (`s`, `?s`) and pickers (`?`, free text) resolve
//! against hidden per-session state or need a prompt, so they error when no
//! prompt is possible; scripts stay deterministic by targeting IDs or paths.

use std::{
    io::{self, BufRead, IsTerminal as _},
    str::FromStr,
};

use camino::{Utf8Path, Utf8PathBuf, absolute_utf8};
use crossterm::style::Stylize as _;
use inquire::Select;
use jp_workspace::{
    Id, Workspace,
    roots::{self, RootEntry},
    session::Session,
    session_store::WorkspaceSessionStore,
    user_data_dir,
};

use crate::{DEFAULT_STORAGE_DIR, USER_WORKSPACES_DIR, cmd, error::Result};

/// A parsed workspace target.
///
/// See the module documentation for the grammar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorkspaceTarget {
    /// A literal workspace ID.
    Id(Id),

    /// An existing filesystem path.
    ///
    /// A bare target is treated as a path when it resolves to an existing path,
    /// so a local directory whose name matches a workspace ID shadows the ID.
    Path(Utf8PathBuf),

    /// `?` — pick from all known workspaces.
    Picker,

    /// `?s` / `?session` — pick from this session's workspace history.
    SessionPicker,

    /// `s` / `session` — the session's previously active workspace.
    Session,

    /// `l` / `latest` — the live root with the newest `last_used` across the
    /// roots registry (global recency, distinct from `s`).
    Latest,

    /// `cwd` / `.` — the cwd-derived workspace.
    ///
    /// As a `jp w use` target this clears the session selection; as a
    /// `--workspace` target it resolves from the invocation directory.
    Cwd,

    /// `-` — read a workspace ID from stdin.
    Stdin,

    /// `help` — print keyword help.
    Help,

    /// Free text — fuzzy-match known workspaces by slug, path, and ID.
    Fuzzy(String),
}

impl FromStr for WorkspaceTarget {
    type Err = crate::error::Error;

    fn from_str(s: &str) -> Result<Self> {
        Ok(match s {
            "" => {
                return Err(crate::error::Error::NotFound(
                    "workspace",
                    "empty target".into(),
                ));
            }
            "?" => Self::Picker,
            "?s" | "?session" => Self::SessionPicker,
            "s" | "session" => Self::Session,
            "l" | "latest" => Self::Latest,
            "cwd" | "." => Self::Cwd,
            "-" => Self::Stdin,
            "help" => Self::Help,
            _ if Utf8Path::new(s).exists() => Self::Path(s.into()),
            _ => Id::from_str(s).map_or_else(|_| Self::Fuzzy(s.to_owned()), Self::Id),
        })
    }
}

/// The keyword help table, printed for the `help` target.
pub(crate) fn help() -> String {
    indoc::indoc! {"
        Workspace targets:

          <id>          a literal workspace ID
          <path>        an existing path (shadows an ID of the same name)
          free text     fuzzy-match known workspaces by slug / path / ID
          ?             pick from all known workspaces
          ?s, ?session  pick from this session's workspace history
          s, session    the previously active workspace (like `cd -`)
          l, latest     the most recently used known workspace
          cwd, .        the cwd-derived workspace (as a `use` target: clears
                        the session selection)
          -             read a workspace ID from stdin
          help          print this help

        Keywords are matched before paths; spell a directory named like a
        keyword as a path (`./s`). Session-derived targets and pickers are
        interactive-only; scripts target IDs or paths."}
    .to_owned()
}

/// The pre-workspace dependencies target resolution runs against.
///
/// Bundled once by the bootstrap (or a `jp workspace` command) and passed
/// explicitly, so resolution never re-derives state from the process
/// environment at each call site.
#[derive(Debug)]
pub(crate) struct TargetEnv<'a> {
    /// Where the user invoked `jp`; relative path targets resolve against it.
    pub(crate) launch_cwd: Utf8PathBuf,

    /// The per-user `workspace/` data directory holding the registries.
    pub(crate) workspaces_dir: Utf8PathBuf,

    /// The user-global session → active-workspace store.
    pub(crate) store: WorkspaceSessionStore,

    /// The resolved session identity, if any.
    pub(crate) session: Option<&'a Session>,

    /// Whether interactive prompting is possible.
    pub(crate) interactive: bool,
}

impl<'a> TargetEnv<'a> {
    /// The environment for this invocation.
    pub(crate) fn new(session: Option<&'a Session>) -> Result<Self> {
        let data_dir = user_data_dir()?;

        Ok(Self {
            launch_cwd: absolute_utf8(".")?,
            workspaces_dir: data_dir.join(USER_WORKSPACES_DIR),
            store: WorkspaceSessionStore::at_user_data_dir(&data_dir),
            session,
            interactive: io::stdin().is_terminal(),
        })
    }
}

/// A concrete checkout root a target resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectedRoot {
    /// The workspace ID, when readable.
    ///
    /// `None` only for path targets whose checkout has no readable ID file;
    /// registry- and session-derived roots always carry one.
    pub(crate) id: Option<Id>,

    /// The checkout root.
    pub(crate) root: Utf8PathBuf,
}

/// What a workspace target resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResolvedTarget {
    /// A concrete, validated checkout root.
    Root(SelectedRoot),

    /// The cwd-derived workspace.
    ///
    /// Symbolic: `jp w use` clears the session selection, the bootstrap
    /// resolves from the launch cwd.
    Cwd,

    /// Print targeting help.
    Help,
}

/// Resolve a target to a concrete checkout root.
///
/// Selection prompts (multi-root IDs, pickers, fuzzy matches) write to stderr;
/// when `env.interactive` is `false`, targets that would prompt — and
/// session-derived targets, which resolve against hidden per-session state —
/// error instead.
#[expect(clippy::too_many_lines)]
pub(crate) fn resolve(target: &WorkspaceTarget, env: &TargetEnv<'_>) -> Result<ResolvedTarget> {
    match target {
        WorkspaceTarget::Help => Ok(ResolvedTarget::Help),
        WorkspaceTarget::Cwd => Ok(ResolvedTarget::Cwd),

        WorkspaceTarget::Id(id) => {
            select_root(id, live_roots(env, id), env.interactive).map(ResolvedTarget::Root)
        }

        WorkspaceTarget::Path(path) => {
            let base = if path.is_absolute() {
                path.clone()
            } else {
                env.launch_cwd.join(path)
            };

            let root = Workspace::find_root(base, DEFAULT_STORAGE_DIR)
                .ok_or(cmd::Error::from(format!("No workspace found at `{path}`.")))?;
            let id = Id::load(root.join(DEFAULT_STORAGE_DIR)).and_then(std::result::Result::ok);

            Ok(ResolvedTarget::Root(SelectedRoot { id, root }))
        }

        WorkspaceTarget::Stdin => {
            let id = stdin_id(io::stdin().lock())?;
            select_root(&id, live_roots(env, &id), env.interactive).map(ResolvedTarget::Root)
        }

        WorkspaceTarget::Session => {
            require_interactive(env, "s / session")?;
            let session = require_session(env)?;

            let entry = env.store.previous(session).ok_or(cmd::Error::from(
                "No previously active workspace recorded for this session.",
            ))?;

            let live = entry
                .id()
                .filter(|id| roots::is_live(&entry.root, id, DEFAULT_STORAGE_DIR));
            let Some(id) = live else {
                return Err(cmd::Error::from(format!(
                    "The previously active workspace checkout is gone ({}). Pick one with `{}`.",
                    entry.root,
                    "jp w use '?s'".bold().yellow(),
                ))
                .into());
            };

            Ok(ResolvedTarget::Root(SelectedRoot {
                id: Some(id),
                root: entry.root,
            }))
        }

        WorkspaceTarget::SessionPicker => {
            require_interactive(env, "?s / ?session")?;
            let session = require_session(env)?;

            let slugs = slug_index(env);
            let rows: Vec<WorkspaceRow> = env
                .store
                .load(session)
                .map(|mapping| mapping.history)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|entry| {
                    let id = entry
                        .id()
                        .filter(|id| roots::is_live(&entry.root, id, DEFAULT_STORAGE_DIR))?;
                    let slug = slugs
                        .iter()
                        .find(|(known, _)| *known == id)
                        .and_then(|(_, slug)| slug.clone());

                    Some(WorkspaceRow {
                        id,
                        slug,
                        root: entry.root,
                    })
                })
                .collect();

            if rows.is_empty() {
                return Err(
                    cmd::Error::from("No live workspaces in this session's history.").into(),
                );
            }

            pick("Select a workspace from this session's history", rows).map(ResolvedTarget::Root)
        }

        WorkspaceTarget::Picker => {
            require_interactive(env, "?")?;

            let rows = known_rows(env);
            if rows.is_empty() {
                return Err(no_known_workspaces().into());
            }

            pick("Select a workspace", rows).map(ResolvedTarget::Root)
        }

        WorkspaceTarget::Latest => latest_root(env)
            .ok_or(no_known_workspaces().into())
            .map(ResolvedTarget::Root),

        WorkspaceTarget::Fuzzy(text) => {
            require_interactive(env, "free-text matching")?;

            let needle = text.to_lowercase();
            let rows: Vec<WorkspaceRow> = known_rows(env)
                .into_iter()
                .filter(|row| row.matches(&needle))
                .collect();

            match rows.len() {
                0 => Err(cmd::Error::from(format!("No known workspace matches `{text}`.")).into()),
                1 => Ok(ResolvedTarget::Root(
                    rows.into_iter().next().expect("one row").into_selected(),
                )),
                _ => pick(&format!("Select a workspace matching `{text}`"), rows)
                    .map(ResolvedTarget::Root),
            }
        }
    }
}

/// One pickable (workspace, live checkout) pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspaceRow {
    /// The workspace ID.
    pub(crate) id: Id,

    /// The cosmetic display name, when the user-workspace directory has one.
    pub(crate) slug: Option<String>,

    /// The live checkout root.
    pub(crate) root: Utf8PathBuf,
}

impl WorkspaceRow {
    /// The display name: the slug when present, the ID otherwise.
    fn name(&self) -> &str {
        self.slug.as_deref().unwrap_or(&self.id)
    }

    /// Case-insensitive substring match over slug, ID, and root path.
    fn matches(&self, needle: &str) -> bool {
        self.slug
            .as_deref()
            .is_some_and(|slug| slug.to_lowercase().contains(needle))
            || self.id.to_lowercase().contains(needle)
            || self.root.as_str().to_lowercase().contains(needle)
    }

    fn into_selected(self) -> SelectedRoot {
        SelectedRoot {
            id: Some(self.id),
            root: self.root,
        }
    }
}

/// Every known (workspace, live checkout) pair, one row per checkout,
/// preserving the registry's most-recently-used-first workspace order.
pub(crate) fn known_rows(env: &TargetEnv<'_>) -> Vec<WorkspaceRow> {
    roots::known_workspaces(&env.workspaces_dir, DEFAULT_STORAGE_DIR)
        .into_iter()
        .flat_map(|workspace| {
            workspace.roots.into_iter().map(move |entry| WorkspaceRow {
                id: workspace.id.clone(),
                slug: workspace.slug.clone(),
                root: entry.path,
            })
        })
        .collect()
}

/// Prompt among all known workspaces, for the bootstrap's picker fallback.
///
/// `Ok(None)` when no workspace is known — the caller composes its own
/// no-workspace error.
pub(crate) fn pick_known_workspace(
    env: &TargetEnv<'_>,
    message: &str,
) -> Result<Option<SelectedRoot>> {
    let rows = known_rows(env);
    if rows.is_empty() {
        return Ok(None);
    }

    pick(message, rows).map(Some)
}

/// The live root with the newest `last_used` across every known workspace.
fn latest_root(env: &TargetEnv<'_>) -> Option<SelectedRoot> {
    // `known_workspaces` orders by most recently used checkout, rootless
    // workspaces last, so the first workspace with a root holds the answer.
    roots::known_workspaces(&env.workspaces_dir, DEFAULT_STORAGE_DIR)
        .into_iter()
        .find_map(|workspace| {
            let root = workspace.roots.into_iter().next()?;
            Some(SelectedRoot {
                id: Some(workspace.id),
                root: root.path,
            })
        })
}

/// Choose a checkout root among a workspace's live roots.
///
/// One live root is used directly.
/// Several open an interactive picker, or — when no prompt is possible — fail
/// with the candidates listed.
/// None is an error pointing the user at a checkout.
pub(crate) fn select_root(
    id: &Id,
    mut roots: Vec<RootEntry>,
    interactive: bool,
) -> Result<SelectedRoot> {
    match roots.len() {
        0 => Err(cmd::Error::from(format!(
            "Workspace '{id}' has no known live checkouts. Run a JP command from inside a \
             checkout of this workspace to register it, or target it by path with `{}`.",
            "--workspace <path>".bold().yellow(),
        ))
        .into()),
        1 => Ok(SelectedRoot {
            id: Some(id.clone()),
            root: roots.remove(0).path,
        }),
        _ if !interactive => {
            let candidates = roots
                .iter()
                .map(|entry| format!("  {}", entry.path))
                .collect::<Vec<_>>()
                .join("\n");

            Err(cmd::Error::from(format!(
                "Workspace '{id}' has multiple checkouts:\n{candidates}\nTarget one with \
                 `--workspace <path>`."
            ))
            .into())
        }
        _ => {
            let message = format!("Select a checkout of workspace '{id}'");
            let labels: Vec<String> = roots.iter().map(|entry| entry.path.to_string()).collect();
            let mut writer = io::stderr();
            let selected = Select::new(&message, labels).prompt_with_writer(&mut writer)?;

            Ok(SelectedRoot {
                id: Some(id.clone()),
                root: Utf8PathBuf::from(selected),
            })
        }
    }
}

/// Prompt among rows, mirroring the RFD's picker sketch: the display name,
/// padded, then the checkout path.
fn pick(message: &str, rows: Vec<WorkspaceRow>) -> Result<SelectedRoot> {
    let width = rows.iter().map(|row| row.name().len()).max().unwrap_or(0);
    let labels: Vec<String> = rows
        .iter()
        .map(|row| format!("{:<width$}  {}", row.name(), row.root))
        .collect();

    let mut writer = io::stderr();
    let selected = Select::new(message, labels.clone()).prompt_with_writer(&mut writer)?;
    let index = labels
        .iter()
        .position(|label| *label == selected)
        .expect("selected label came from the list");

    Ok(rows
        .into_iter()
        .nth(index)
        .expect("index within bounds")
        .into_selected())
}

/// The deduplicated (ID, slug) pairs known to the registry.
fn slug_index(env: &TargetEnv<'_>) -> Vec<(Id, Option<String>)> {
    roots::known_workspaces(&env.workspaces_dir, DEFAULT_STORAGE_DIR)
        .into_iter()
        .map(|workspace| (workspace.id, workspace.slug))
        .collect()
}

/// Read a workspace ID from a stdin-style reader (the `-` target).
pub(crate) fn stdin_id(mut reader: impl BufRead) -> Result<Id> {
    let mut line = String::new();
    reader.read_line(&mut line)?;

    let value = line.trim();
    if value.is_empty() {
        return Err(cmd::Error::from("No workspace ID on stdin.").into());
    }

    Ok(Id::from_str(value)?)
}

/// Expand an ID through the roots registry, pruning dead entries.
fn live_roots(env: &TargetEnv<'_>, id: &Id) -> Vec<RootEntry> {
    roots::resolve_live_roots(&env.workspaces_dir, id, DEFAULT_STORAGE_DIR)
}

/// The shared "nothing registered yet" error.
fn no_known_workspaces() -> cmd::Error {
    cmd::Error::from(
        "No known workspaces. JP registers a workspace when a command runs from inside it.",
    )
}

/// Error unless a prompt is possible: `target` resolves against hidden
/// per-session state or needs a picker, so scripts must not depend on it.
fn require_interactive(env: &TargetEnv<'_>, target: &str) -> Result<()> {
    if env.interactive {
        return Ok(());
    }

    Err(cmd::Error::from(format!(
        "Workspace target `{target}` is interactive-only. Non-interactive runs target a workspace \
         by ID or path."
    ))
    .into())
}

/// Error unless a session identity exists to resolve session state against.
fn require_session<'a>(env: &TargetEnv<'a>) -> Result<&'a Session> {
    env.session.ok_or(
        cmd::Error::from(
            "No session identity available. Set $JP_SESSION or run in a terminal with automatic \
             session detection.",
        )
        .into(),
    )
}

#[cfg(test)]
#[path = "target_tests.rs"]
mod tests;
