use jp_conversation::ConversationId;
use jp_format::conversation::DetailsFmt;

use crate::{Output, cmd::Success, ctx::Ctx, error::Error};

#[derive(Debug, clap::Args)]
pub(crate) struct Show {
    /// Conversation ID to show.
    ///
    /// Defaults to the active conversation if not specified.
    id: Option<ConversationId>,
}

impl Show {
    pub(crate) fn run(self, ctx: &mut Ctx) -> Output {
        let active_id = ctx.workspace.active_conversation_id();
        let id = self.id.unwrap_or(active_id);
        let Some(conversation) = ctx.workspace.get_conversation(&id).cloned() else {
            return Err(Error::NotFound("Conversation", id.to_string()).into());
        };
        let events = ctx.workspace.get_events(&id);
        let user = conversation.user;
        let details = DetailsFmt::new(id, conversation, events)
            .with_local_flag(user)
            .with_active_conversation(active_id)
            .with_hyperlinks(ctx.term.args.hyperlinks)
            .with_color(ctx.term.args.colors);

        let rows = details.rows();

        Ok(Success::Details {
            title: details.title,
            rows,
        })
    }
}
