use jp_conversation::Error as ConversationError;
use jp_storage::backend::StoragePresence;
use jp_workspace::ConversationHandle;

use crate::{
    cmd::{ConversationLoadRequest, Output, conversation_id::PositionalIds},
    ctx::Ctx,
    format::{attachment_detail_item, compaction_detail_item, conversation::DetailsFmt},
    output::print_details,
};

#[derive(Debug, clap::Args)]
pub(crate) struct Show {
    #[command(flatten)]
    target: PositionalIds<true, true>,
}

impl Show {
    #[expect(clippy::unused_self)]
    pub(crate) fn run(self, ctx: &mut Ctx, handles: Vec<ConversationHandle>) -> Output {
        let active_id = ctx
            .session
            .as_ref()
            .and_then(|s| ctx.workspace.session_active_conversation(s));

        for handle in handles {
            let id = handle.id();
            let local =
                ctx.workspace.conversation_presence(&id) == Some(StoragePresence::UserLocalOnly);
            let conversation = ctx.workspace.metadata(&handle)?;
            let events = ctx.workspace.events(&handle)?;

            let mut attachments = vec![];
            for attachment in events
                .config()
                .map_err(ConversationError::from)?
                .conversation
                .attachments
            {
                attachments.push(attachment_detail_item(&attachment.to_url()?));
            }

            let compactions = events.compactions().map(compaction_detail_item).collect();

            let details = DetailsFmt::new(id)
                .with_last_message_at(events.last().map(|v| v.event.timestamp))
                .with_event_count(events.len())
                .with_turn_count(events.iter_turns().len())
                .with_title(conversation.title.as_ref())
                .with_last_activated_at(Some(conversation.last_activated_at))
                .with_pinned_flag(conversation.is_pinned())
                .with_local_flag(local)
                .with_active_conversation(active_id.unwrap_or(id))
                .with_expires_at(conversation.expires_at)
                .with_attachments(attachments)
                .with_compactions(compactions)
                .with_pretty_printing(ctx.printer.pretty_printing_enabled());

            print_details(&ctx.printer, details.title.as_deref(), details.rows());
        }
        Ok(())
    }

    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        ConversationLoadRequest::explicit_or_session(&self.target)
    }
}
