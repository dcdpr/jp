use crossterm::style::Stylize as _;
use jp_conversation::ConversationId;

use crate::{Output, ctx::Ctx};

#[derive(Debug, clap::Args)]
pub(crate) struct Use {
    /// Conversation ID to use as the active conversation.
    ///
    /// If not specified, the *previous* active conversation is used (if any).
    id: Option<ConversationId>,
}

impl Use {
    pub(crate) fn run(self, ctx: &mut Ctx) -> Output {
        let id = self.id.unwrap_or_else(|| {
            let active_id = ctx.workspace.active_conversation_id();
            let mut conversations = ctx
                .workspace
                .conversations()
                .filter(|(id, _)| *id != &active_id)
                .collect::<Vec<_>>();

            conversations.sort_by(|a, b| b.1.last_activated_at.cmp(&a.1.last_activated_at));
            conversations
                .into_iter()
                .next()
                .map_or(active_id, |(id, _)| *id)
        });

        let id_fmt = id.to_string().bold().yellow();
        let active_id = ctx.workspace.active_conversation_id();
        if id == active_id {
            return Ok(format!("Already active conversation: {id_fmt}").into());
        }

        if ctx.workspace.set_active_conversation_id(id).is_err() {
            Err((1, format!("Conversation not found: {}", id_fmt.red())))?;
        }

        Ok(format!(
            "Switched active conversation from {} to {}",
            active_id.to_string().bold().grey(),
            id.to_string().bold().yellow()
        )
        .into())
    }
}
