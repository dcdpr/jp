use jp_workspace::ConversationHandle;

use crate::{
    cmd::{ConversationLoadRequest, Output, conversation_id::PositionalIds},
    ctx::Ctx,
    format::conversation::DetailsFmt,
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
            let conversation = ctx.workspace.metadata(&handle)?;
            let events = ctx.workspace.events(&handle)?;
            let details = DetailsFmt::new(id)
                .with_last_message_at(events.last().map(|v| v.event.timestamp))
                .with_event_count(events.len())
                .with_title(conversation.title.as_ref())
                .with_last_activated_at(Some(conversation.last_activated_at))
                .with_pinned_flag(conversation.pinned)
                .with_local_flag(conversation.user)
                .with_active_conversation(active_id.unwrap_or(id))
                .with_expires_at(conversation.expires_at)
                .with_pretty_printing(ctx.printer.pretty_printing_enabled());

            print_details(&ctx.printer, details.title.as_deref(), details.rows());
        }
        Ok(())
    }

    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        ConversationLoadRequest::explicit_or_session(&self.target.ids)
    }
}
