use crossterm::style::Stylize as _;
use jp_conversation::ConversationId;

use crate::{
    cmd::{
        ConversationLoadRequest, Output,
        conversation_id::{ConversationIds as _, PositionalIds},
        target::{ConversationTarget, PickerFilter, resolve_archived_picker},
    },
    ctx::Ctx,
};

/// Unarchive conversations.
///
/// Without IDs, shows a picker of archived conversations to restore.
/// With IDs, unarchives each one (non-archived IDs are skipped with a
/// warning).
#[derive(Debug, clap::Args)]
pub(crate) struct Unarchive {
    #[command(flatten)]
    target: PositionalIds<false, true>,
}

impl Unarchive {
    #[expect(clippy::unused_self)]
    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        // Archived conversations aren't in the active index, so we bypass
        // normal resolution entirely.
        ConversationLoadRequest::none()
    }

    pub(crate) fn run(self, ctx: &mut Ctx) -> Output {
        let ids = self.resolve_ids(ctx)?;

        for id in ids {
            if ctx.workspace.acquire_conversation(&id).is_ok() {
                ctx.printer.println(format!(
                    "Conversation {} is not archived, skipping.",
                    id.to_string().bold()
                ));
            } else {
                ctx.workspace.unarchive_conversation(&id)?;
                ctx.printer.println(format!(
                    "Conversation {} unarchived.",
                    id.to_string().bold().yellow()
                ));
            }
        }

        Ok(())
    }

    fn resolve_ids(&self, ctx: &Ctx) -> Result<Vec<ConversationId>, crate::error::Error> {
        let targets = self.target.ids();

        if targets.is_empty() {
            if ctx.workspace.archived_conversations().next().is_none() {
                ctx.printer.println("No archived conversations.");
                return Ok(vec![]);
            }

            let id = resolve_archived_picker(&ctx.workspace, &PickerFilter {
                archived: true,
                ..Default::default()
            })?;
            return Ok(vec![id]);
        }

        let mut ids = Vec::new();
        for target in targets {
            match target {
                ConversationTarget::Id(id) => ids.push(*id),
                other => {
                    ids.extend(other.resolve(&ctx.workspace, ctx.session.as_ref())?);
                }
            }
        }
        Ok(ids)
    }
}
