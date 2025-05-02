use crossterm::style::Stylize as _;
use inquire::Confirm;
use jp_conversation::{Conversation, ConversationId};
use jp_format::conversation::DetailsFmt;
use jp_workspace::query::ConversationQuery;

use crate::{cmd::Success, ctx::Ctx, Output};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Conversation ID to remove.
    ///
    /// Defaults to the active conversation if not specified.
    id: Option<ConversationId>,

    /// Do not prompt for confirmation.
    #[arg(long)]
    yes: bool,
}

impl Args {
    pub fn run(self, ctx: &mut Ctx) -> Output {
        let active_id = ctx.workspace.active_conversation_id();
        let id = self.id.unwrap_or(active_id);
        let Some(conversation) = ctx.workspace.get_conversation(&id).cloned() else {
            return Err(
                format!("Conversation {} not found", id.to_string().bold().yellow()).into(),
            );
        };
        let messages = ctx.workspace.get_messages(&id);
        let private = conversation.private;
        let mut details = DetailsFmt::new(id, conversation, messages)
            .with_private_flag(private)
            .with_active_conversation(active_id)
            .with_hyperlinks(ctx.term.args.hyperlinks)
            .with_color(ctx.term.args.colors);

        if !self.yes {
            details.title = Some(format!(
                "Removing conversation {}",
                id.to_string().bold().yellow()
            ));
            println!("{details}\n");

            let confirm = Confirm::new("Are you sure?")
                .with_default(false)
                .with_help_message("this action cannot be undone");

            match confirm.prompt() {
                Ok(true) => {}
                Ok(false) | Err(_) => return Err(1.into()),
            }
        }

        // We can't remove the active conversation, so we need to first switch
        // the active conversation to another one, or create a new one if
        // needed.
        if id == active_id {
            let new_active_id = {
                let mut conversations = ctx.workspace.conversations();
                let mut query = ConversationQuery::new(active_id, &mut conversations);
                query.last_active_conversation_id().copied()
            }
            .unwrap_or_else(|| ctx.workspace.create_conversation(Conversation::default()));

            ctx.workspace.set_active_conversation_id(new_active_id)?;
        }

        if let Err(err) = ctx.workspace.remove_conversation(&id) {
            return Err(format!(
                "Failed to remove conversation {}: {}",
                id.to_string().bold().yellow(),
                err.to_string().red()
            )
            .into());
        }

        Ok(Success::Message("Conversation removed.".into()))
    }
}
