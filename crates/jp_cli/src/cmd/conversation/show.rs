use jp_conversation::{ConversationId, ConversationStream};
use jp_format::conversation::DetailsFmt;

use crate::{Output, cmd::Success, ctx::Ctx};

#[derive(Debug, clap::Args)]
pub(crate) struct Show {
    /// Conversation ID to show.
    ///
    /// Defaults to the active conversation if not specified.
    id: Option<ConversationId>,
}

impl Show {
    #[expect(clippy::unnecessary_wraps)]
    pub(crate) fn run(self, ctx: &mut Ctx) -> Output {
        let active_id = ctx.workspace.active_conversation_id();
        let id = self.id.unwrap_or(active_id);
        let conversation = ctx.workspace.get_conversation(&id);
        let events = ctx.workspace.get_events(&id);
        let user = conversation.is_some_and(|v| v.user);
        let details = DetailsFmt::new(id)
            .with_last_message_at(events.and_then(|v| v.last().map(|v| v.event.timestamp)))
            .with_event_count(events.map(ConversationStream::len).unwrap_or_default())
            .with_title(conversation.and_then(|v| v.title.as_ref()))
            .with_last_activated_at(conversation.map(|v| v.last_activated_at))
            .with_local_flag(user)
            .with_active_conversation(active_id)
            .with_expires_at(conversation.and_then(|v| v.expires_at))
            .with_hyperlinks(ctx.term.args.hyperlinks)
            .with_color(ctx.term.args.colors);

        let rows = details.rows();

        Ok(Success::Details {
            title: details.title,
            rows,
        })
    }
}
