use crossterm::style::Stylize as _;
use jp_conversation::ConversationId;
use jp_workspace::{ConversationHandle, LockResult};
use tracing::{debug, warn};

use crate::{
    cmd::{
        ConversationLoadRequest, Output,
        conversation_id::{ConversationIds as _, PositionalIds},
        target::{ConversationTarget, PickerFilter, resolve_archived_picker, resolve_picker},
        time::CreationRange,
    },
    ctx::Ctx,
    shared::search,
};

/// Set the active conversation.
///
/// Without flags, `jp c use [ID]` activates the given conversation (or opens a
/// picker when no target is provided).
/// `--grep` and `--from`/`--until` restrict the picker's candidate set; when
/// the combined filter leaves a single conversation, it is activated directly
/// without prompting.
#[derive(Debug, clap::Args)]
pub(crate) struct Use {
    #[command(flatten)]
    target: PositionalIds<true, false>,

    /// Restrict picker candidates to conversations whose title or chat content
    /// matches.
    ///
    /// Substring match.
    /// Case-insensitive unless the pattern contains an uppercase character
    /// (smart-case).
    /// Composable with target keywords (`?`, `?p`, `?s`, `?a`) and with
    /// `--from` / `--until`.
    #[arg(long)]
    grep: Option<String>,

    /// Restrict picker candidates to a creation-date range.
    #[command(flatten)]
    range: CreationRange<false>,
}

impl Use {
    /// Whether the targets resolve against the archive partition.
    fn is_archived(&self) -> bool {
        self.target
            .ids()
            .iter()
            .any(ConversationTarget::is_archived)
    }

    /// Whether any candidate-set filter (`--grep`, `--from`, `--until`) is set.
    /// When true, `Use` resolves its handle internally instead of going through
    /// the standard pipeline.
    fn has_filter(&self) -> bool {
        self.grep.is_some() || self.range.is_set()
    }

    pub(crate) fn run(self, ctx: &mut Ctx, handles: Vec<ConversationHandle>) -> Output {
        if self.has_filter() {
            return self.run_filtered(ctx);
        }

        // Archive targets bypass the normal resolution pipeline — the ID isn't
        // in the workspace index yet. We resolve + unarchive + activate in one
        // step.
        if self.is_archived() {
            return self.run_unarchive(ctx);
        }

        let handle = handles.into_iter().next().expect("Use requires a handle");
        Self::run_activate_inner(ctx, handle)
    }

    fn run_activate_inner(ctx: &mut Ctx, handle: ConversationHandle) -> Output {
        let id = handle.id();

        let active_id = ctx
            .session
            .as_ref()
            .and_then(|s| ctx.workspace.session_active_conversation(s));

        if active_id == Some(id) {
            let id_fmt = id.to_string().bold().yellow();
            ctx.printer
                .println(format!("Already active conversation: {id_fmt}"));
            return Ok(());
        }

        let Some(session) = &ctx.session else {
            return Err((
                1,
                "No session identity available. Set $JP_SESSION or run in a terminal with \
                 automatic session detection."
                    .to_string(),
            )
                .into());
        };

        // Try to acquire the conversation lock non-blocking. On contention
        // (another process is mid-query on this conversation), skip the
        // metadata bump — we can't write `last_activated_at` while someone else
        // holds the lock, and we don't want to block behind a streaming query
        // just to record the activation time in the metadata.
        //
        // The on-disk `last_activated_at` reflects the lock holder's activation
        // time (typically recent), which is close enough for sorting and
        // archive cutoffs in the common case. We still record this session's
        // mapping so the user's intent ("X is now my active conversation") is
        // captured immediately, and the `SessionHistoryEntry::activated_at` we
        // write here carries the exact `now`.
        let now = ctx.now();
        let result = match ctx.workspace.lock_conversation(handle, Some(session))? {
            LockResult::Acquired(lock) => ctx
                .workspace
                .activate_session_conversation(&lock, session, now),
            LockResult::AlreadyLocked(_) => {
                debug!(
                    %id,
                    "Conversation locked by another process; recording session activation only."
                );
                ctx.workspace.record_session_activation(session, id, now)
            }
        };
        if let Err(error) = result {
            warn!(%error, "Failed to record activation.");
        }

        let from = active_id.map_or_else(
            || "(none)".grey().to_string(),
            |id| id.to_string().bold().grey().to_string(),
        );
        let to = id.to_string().bold().yellow();
        let title_suffix = conversation_title(ctx, id)
            .map(|t| format!(": {}", t.yellow()))
            .unwrap_or_default();

        ctx.printer.println(format!(
            "Switched active conversation from {from} to {to}{title_suffix}"
        ));

        Ok(())
    }

    /// Resolve an archived conversation target, unarchive it, and activate it.
    fn run_unarchive(&self, ctx: &mut Ctx) -> Output {
        // Resolve the archive target to a concrete ID.
        let id = self
            .target
            .ids()
            .iter()
            .find_map(|t| {
                t.resolve(&ctx.workspace, ctx.session.as_ref())
                    .ok()
                    .and_then(|ids| ids.into_iter().next())
            })
            .ok_or_else(|| {
                crate::error::Error::NotFound("conversation", "no archived conversations".into())
            })?;

        let handle = ctx.workspace.unarchive_conversation(&id)?;

        let id_fmt = id.to_string().bold().yellow();
        ctx.printer
            .println(format!("Unarchived conversation {id_fmt}"));

        Self::run_activate_inner(ctx, handle)
    }

    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        if self.has_filter() || self.is_archived() {
            // Filter and archive modes resolve internally.
            ConversationLoadRequest::none()
        } else {
            ConversationLoadRequest::explicit_or_previous(&self.target)
        }
    }

    /// Resolve a handle by building a candidate set from the target, applying
    /// the range and grep filters, then either activating directly (when the
    /// survivor is unique) or opening a picker.
    fn run_filtered(self, ctx: &mut Ctx) -> Output {
        let session = ctx.session.as_ref();

        let target = self
            .target
            .ids()
            .first()
            .cloned()
            .unwrap_or(ConversationTarget::Picker(PickerFilter::default()));

        if matches!(target, ConversationTarget::Help) {
            return Err(crate::error::Error::TargetHelp {
                session: true,
                multi: false,
            }
            .into());
        }

        let archived_partition = target.is_archived();
        let sub_filter = match &target {
            ConversationTarget::Picker(f) => f.clone(),
            _ => PickerFilter::default(),
        };

        // 1. Source candidate IDs from the target's partition.
        let source_ids = source_ids(&ctx.workspace, session, &target, &sub_filter);

        // 2. Range filter.
        let ranged: Vec<ConversationId> = source_ids
            .into_iter()
            .filter(|id| self.range.matches(*id))
            .collect();

        // 3. Grep filter.
        let final_ids = match &self.grep {
            Some(pattern) => search::filter_ids(ctx, &ranged, pattern),
            None => ranged,
        };

        if final_ids.is_empty() {
            return Err(crate::error::Error::NotFound(
                "conversation",
                "no conversations match the filter".into(),
            )
            .into());
        }

        // 4. Pick — skip the prompt if the filter narrowed to one.
        let id = if final_ids.len() == 1 {
            final_ids.into_iter().next().expect("non-empty")
        } else {
            let mut filter = sub_filter;
            filter.archived = archived_partition;
            filter.candidate_ids = Some(final_ids);
            if archived_partition {
                resolve_archived_picker(&ctx.workspace, &filter)?
            } else {
                resolve_picker(&ctx.workspace, session, &filter)?
            }
        };

        // 5. Activate (unarchive first if drawn from the archive partition).
        if archived_partition {
            let handle = ctx.workspace.unarchive_conversation(&id)?;
            let id_fmt = id.to_string().bold().yellow();
            ctx.printer
                .println(format!("Unarchived conversation {id_fmt}"));
            Self::run_activate_inner(ctx, handle)
        } else {
            let handle = ctx.workspace.acquire_conversation(&id)?;
            Self::run_activate_inner(ctx, handle)
        }
    }
}

/// Build the source candidate ID set for filter mode.
///
/// When `target` resolves to a non-empty ID list (literal ID, `latest`,
/// `archived`, etc.), use it directly.
/// When it resolves empty (i.e. a picker target like `?`, `?p`, `?a`), draw
/// from the matching partition with the picker's sub-filter applied.
fn source_ids(
    workspace: &jp_workspace::Workspace,
    session: Option<&jp_workspace::session::Session>,
    target: &ConversationTarget,
    sub_filter: &PickerFilter,
) -> Vec<ConversationId> {
    if let Ok(ids) = target.resolve(workspace, session)
        && !ids.is_empty()
    {
        return ids;
    }

    if sub_filter.archived {
        return workspace
            .archived_conversations()
            .filter(|(_id, c, _)| !sub_filter.pinned || c.is_pinned())
            .map(|(id, _, _)| id)
            .collect();
    }

    let session_ids: Vec<_> = session
        .map(|s| workspace.session_conversation_ids(s))
        .unwrap_or_default();
    workspace
        .conversations()
        .filter(|(id, c)| {
            // Apply the picker's pinned/session sub-filter to the workspace
            // listing. `candidate_ids` is left unset here — the surviving set
            // is what we're computing.
            (!sub_filter.pinned || c.is_pinned())
                && (!sub_filter.session || session_ids.contains(id))
        })
        .map(|(id, _)| *id)
        .collect()
}

fn conversation_title(ctx: &Ctx, id: ConversationId) -> Option<String> {
    let h = ctx.workspace.acquire_conversation(&id).ok()?;
    ctx.workspace.metadata(&h).ok()?.title.clone()
}

#[cfg(test)]
#[path = "use_tests.rs"]
mod tests;
