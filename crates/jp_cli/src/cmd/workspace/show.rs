use std::collections::BTreeSet;

use camino::Utf8Path;
use crossterm::style::Stylize as _;
use jp_conversation::ConversationId;
use jp_printer::Printer;
use jp_workspace::{Id, Workspace, roots, session_store::WorkspaceSelection};
use tracing::warn;

use crate::{
    DEFAULT_STORAGE_DIR,
    cmd::{
        Output,
        workspace::target::{self, ResolvedTarget, TargetEnv, WorkspaceTarget},
    },
    format::workspace::{DetailsFmt, checkout_detail_item},
    output::print_details,
};

/// Show a workspace: identity, checkouts, and how it resolves.
///
/// With no target, reports the session's active workspace, falling back to the
/// cwd-derived one; "no workspace selected" is a first-class outcome, not an
/// error (RFD 087).
/// `show` stays read-only and script-friendly: concrete targets never prompt —
/// a multi-checkout workspace lists every live root — and only the picker
/// targets (`?`, `?s`) require interactivity.
#[derive(Debug, clap::Args)]
pub(crate) struct Show {
    /// The workspace to show.
    /// See `jp w use help` for the grammar.
    ///
    /// Defaults to the session's active workspace, then the cwd-derived one.
    target: Option<WorkspaceTarget>,
}

/// What `show` reports on: a workspace and how it was reached.
struct Subject {
    id: Id,
    slug: Option<String>,
    /// Live checkouts, most recently used first.
    roots: Vec<roots::RootEntry>,
    /// How the subject was resolved, for the readout.
    resolved: &'static str,
}

impl Show {
    pub(crate) fn run(self, printer: &Printer, env: &TargetEnv<'_>, persist: bool) -> Output {
        if matches!(self.target, Some(WorkspaceTarget::Help)) {
            printer.println(target::help());
            return Ok(());
        }

        let active = env.session.and_then(|session| env.store.active(session));
        let cwd_root = Workspace::find_root(env.launch_cwd.clone(), DEFAULT_STORAGE_DIR);

        let Some(subject) = self.subject(env, active.as_ref(), cwd_root.as_deref())? else {
            printer.println(format!(
                "No workspace selected. Select one with `{}`, or run from inside a workspace.",
                "jp w use ?".bold().yellow(),
            ));
            return Ok(());
        };

        render(
            printer,
            env,
            &subject,
            active.as_ref(),
            cwd_root.as_deref(),
            persist,
        );
        Ok(())
    }

    /// Resolve the readout subject.
    ///
    /// `Ok(None)` is the no-target, nothing-selected outcome.
    #[expect(clippy::too_many_lines)]
    fn subject(
        &self,
        env: &TargetEnv<'_>,
        active: Option<&WorkspaceSelection>,
        cwd_root: Option<&Utf8Path>,
    ) -> Result<Option<Subject>, crate::cmd::Error> {
        let Some(target) = &self.target else {
            // No target: the session's active workspace, then cwd.
            if let Some(id) = active.and_then(WorkspaceSelection::id) {
                return Ok(Some(subject_for(env, id, "session-active")));
            }
            return Ok(cwd_root
                .and_then(root_id)
                .map(|id| subject_for(env, id, "current directory")));
        };

        match target {
            WorkspaceTarget::Help => unreachable!("handled before subject resolution"),

            WorkspaceTarget::Id(id) => Ok(Some(subject_for(env, id.clone(), "explicit target"))),

            WorkspaceTarget::Path(path) => {
                let base = if path.is_absolute() {
                    path.clone()
                } else {
                    env.launch_cwd.join(path)
                };
                let root = Workspace::find_root(base, DEFAULT_STORAGE_DIR)
                    .ok_or_else(|| format!("No workspace found at `{path}`."))?;
                let id = root_id(&root)
                    .ok_or_else(|| format!("`{root}` has no readable workspace ID."))?;

                Ok(Some(subject_for(env, id, "explicit target")))
            }

            WorkspaceTarget::Cwd => Ok(cwd_root
                .and_then(root_id)
                .map(|id| subject_for(env, id, "current directory"))),

            WorkspaceTarget::Session => {
                let session = env.session.ok_or(
                    "No session identity available. Set $JP_SESSION or run in a terminal with \
                     automatic session detection.",
                )?;
                let entry = env
                    .store
                    .previous(session)
                    .ok_or("No previously active workspace recorded for this session.")?;
                let id = entry
                    .id()
                    .ok_or("The session history entry holds an invalid workspace ID.")?;

                Ok(Some(subject_for(env, id, "session history")))
            }

            WorkspaceTarget::Latest => Ok(roots::known_workspaces(
                &env.workspaces_dir,
                DEFAULT_STORAGE_DIR,
            )
            .into_iter()
            .find(|workspace| !workspace.roots.is_empty())
            .map(|workspace| subject_for(env, workspace.id, "most recently used"))),

            WorkspaceTarget::Stdin => {
                let id = target::stdin_id(std::io::stdin().lock())?;
                Ok(Some(subject_for(env, id, "explicit target")))
            }

            // Free text stays promptless here: a unique match resolves, an
            // ambiguous one errors with the candidates. `show` is the
            // scriptable readout; interactive exploration is `jp w use ?`.
            WorkspaceTarget::Fuzzy(text) => {
                let needle = text.to_lowercase();
                let matches: Vec<_> =
                    roots::known_workspaces(&env.workspaces_dir, DEFAULT_STORAGE_DIR)
                        .into_iter()
                        .filter(|workspace| {
                            workspace
                                .slug
                                .as_deref()
                                .is_some_and(|slug| slug.to_lowercase().contains(&needle))
                                || workspace.id.to_lowercase().contains(&needle)
                                || workspace.roots.iter().any(|entry| {
                                    entry.path.as_str().to_lowercase().contains(&needle)
                                })
                        })
                        .collect();

                match matches.len() {
                    0 => Err(format!("No known workspace matches `{text}`.").into()),
                    1 => Ok(matches
                        .into_iter()
                        .next()
                        .map(|workspace| subject_for(env, workspace.id, "fuzzy match"))),
                    _ => {
                        let candidates = matches
                            .iter()
                            .map(|workspace| {
                                format!(
                                    "  {} ({})",
                                    workspace.slug.as_deref().unwrap_or("-"),
                                    workspace.id,
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n");

                        Err(format!(
                            "`{text}` matches multiple workspaces:\n{candidates}\nNarrow the \
                             match or use an ID."
                        )
                        .into())
                    }
                }
            }

            // The pickers prompt, which `resolve` gates on interactivity.
            WorkspaceTarget::Picker | WorkspaceTarget::SessionPicker => {
                match target::resolve(target, env)? {
                    ResolvedTarget::Root(selected) => {
                        Ok(selected.id.map(|id| subject_for(env, id, "picked")))
                    }
                    ResolvedTarget::Cwd | ResolvedTarget::Help => {
                        unreachable!("pickers resolve to a root")
                    }
                }
            }
        }
    }
}

/// Build the subject for a workspace ID: its slug and live checkouts.
fn subject_for(env: &TargetEnv<'_>, id: Id, resolved: &'static str) -> Subject {
    let slug = roots::known_workspaces(&env.workspaces_dir, DEFAULT_STORAGE_DIR)
        .into_iter()
        .find(|workspace| workspace.id == id)
        .and_then(|workspace| workspace.slug);
    let roots = roots::resolve_live_roots(&env.workspaces_dir, &id, DEFAULT_STORAGE_DIR);

    Subject {
        id,
        slug,
        roots,
        resolved,
    }
}

/// The workspace ID stored at a checkout root, when readable.
fn root_id(root: &Utf8Path) -> Option<Id> {
    Id::load(root.join(DEFAULT_STORAGE_DIR)).and_then(Result::ok)
}

/// Print the readout.
fn render(
    printer: &Printer,
    env: &TargetEnv<'_>,
    subject: &Subject,
    active: Option<&WorkspaceSelection>,
    cwd_root: Option<&Utf8Path>,
    persist: bool,
) {
    let pretty = printer.pretty_printing_enabled();

    // Sticky is session-level state about the *active* workspace, so it only
    // renders when the subject is the active one.
    let subject_is_active = active.is_some_and(|entry| entry.workspace_id == *subject.id);
    let sticky = if subject_is_active
        && let Some(session) = env.session
        && let Some(mapping) = env.store.load(session)
    {
        Some(mapping.sticky)
    } else {
        None
    };

    let checkouts = subject
        .roots
        .iter()
        .map(|entry| {
            let is_active = active.is_some_and(|selection| selection.root == entry.path);
            checkout_detail_item(&entry.path, entry.last_used, is_active, pretty)
        })
        .collect();

    let stats = conversation_stats(env, &subject.roots, persist);

    let details = DetailsFmt::new(subject.id.clone(), subject.resolved)
        .with_slug(subject.slug.as_deref())
        .with_sticky(sticky)
        .with_checkouts(checkouts)
        .with_conversations(stats.as_ref().map(|stats| stats.count))
        .with_active_conversation(stats.and_then(|stats| stats.active))
        .with_pretty_printing(pretty);

    print_details(printer, details.title(), details.rows(), &details.json());

    // The cwd-vs-active tension, surfaced instead of silently resolved (RFD
    // 087's precedence ladder): a sticky session keeps the active workspace;
    // otherwise commands prompt when the two disagree.
    if subject_is_active
        && let Some(cwd_root) = cwd_root
        && active.is_some_and(|entry| entry.root != cwd_root)
    {
        let note = if sticky.unwrap_or(false) {
            format!(
                "Note: the current directory resolves to `{cwd_root}`, but this session is sticky \
                 to its active workspace, which takes precedence for commands run here."
            )
        } else {
            format!(
                "Note: the current directory resolves to `{cwd_root}`; commands run here prompt \
                 between it and the active workspace."
            )
        };
        printer.println(note);
    }
}

/// The union conversation count, and the session's active conversation there.
struct ConversationStats {
    count: usize,
    active: Option<(ConversationId, Option<String>)>,
}

/// Union the conversation IDs across the user-local durable store and every
/// live checkout, deduplicated by ID — accurate *and* cheap (RFD 087).
///
/// Only the first loadable root pays a full workspace load: that load already
/// merges the user-local durable store, so every sibling checkout can only add
/// conversations that live in its own projection alone.
/// Those are picked up with a bare directory scan per sibling — no workspace
/// construction, no user-local re-merge, and no roots-registry writes, keeping
/// `show` read-only for the checkouts it merely reports on.
///
/// Roots that fail to load are skipped with a warning; `None` when no root
/// produced an index.
fn conversation_stats(
    env: &TargetEnv<'_>,
    roots: &[roots::RootEntry],
    persist: bool,
) -> Option<ConversationStats> {
    let mut ids: BTreeSet<ConversationId> = BTreeSet::new();
    let mut active = None;
    let mut remaining = roots.iter();

    // One full load: user-local union, plus the session's active
    // conversation. The session → conversation mapping is per workspace ID,
    // so any loaded checkout answers it.
    let mut loaded_any = false;
    for entry in remaining.by_ref() {
        let root = &entry.path;
        let (mut workspace, _backend) = match crate::load_workspace(root, persist) {
            Ok(loaded) => loaded,
            Err(error) => {
                warn!(%error, %root, "Skipping unloadable checkout in the conversation count.");
                continue;
            }
        };

        workspace.load_conversation_index();
        ids.extend(workspace.conversations().map(|(id, _)| *id));
        loaded_any = true;

        if let Some(session) = env.session
            && let Some(id) = workspace.session_active_conversation(session)
        {
            let title = workspace
                .acquire_conversation(&id)
                .ok()
                .and_then(|handle| workspace.metadata(&handle).ok())
                .and_then(|metadata| metadata.title.clone());
            active = Some((id, title));
        }

        break;
    }

    if !loaded_any {
        return None;
    }

    // The sibling checkouts: checkout-only conversations via directory scan.
    for entry in remaining {
        ids.extend(jp_storage::load::projected_conversation_ids(
            &entry.path.join(DEFAULT_STORAGE_DIR),
        ));
    }

    Some(ConversationStats {
        count: ids.len(),
        active,
    })
}

#[cfg(test)]
#[path = "show_tests.rs"]
mod tests;
