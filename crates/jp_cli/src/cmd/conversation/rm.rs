use crossterm::style::Stylize as _;
use inquire::Confirm;
use jp_conversation::{Conversation, ConversationId};
use jp_format::conversation::DetailsFmt;
use jp_workspace::query::ConversationQuery;

use crate::{cmd::Success, ctx::Ctx, Output};

#[derive(Debug, clap::Args)]
pub(crate) struct Rm {
    /// Conversation IDs to remove.
    ///
    /// Defaults to the active conversation if not specified.
    #[arg(conflicts_with = "from")]
    id: Vec<ConversationId>,

    /// Remove all conversations *starting from* the specified conversation,
    /// based on creation date.
    #[arg(long, conflicts_with = "id")]
    from: Option<ConversationId>,

    /// Do not prompt for confirmation.
    #[arg(long)]
    yes: bool,
}

impl Rm {
    pub(crate) fn run(self, ctx: &mut Ctx) -> Output {
        let active_id = ctx.workspace.active_conversation_id();
        let ids = if let Some(from) = &self.from {
            ctx.workspace
                .conversations()
                .map(|(id, _)| *id)
                .filter(|id| id >= from)
                .collect::<Vec<_>>()
        } else if self.id.is_empty() {
            vec![active_id]
        } else {
            self.id.clone()
        };

        for id in ids {
            self.remove(ctx, id)?;
        }

        Ok(Success::Message("Conversation(s) removed.".into()))
    }

    fn remove(&self, ctx: &mut Ctx, id: ConversationId) -> Output {
        let active_id = ctx.workspace.active_conversation_id();

        let Some(conversation) = ctx.workspace.get_conversation(&id).cloned() else {
            return Err(
                format!("Conversation {} not found", id.to_string().bold().yellow()).into(),
            );
        };
        let events = ctx.workspace.get_events(&id);
        let local = conversation.user;
        let mut details = DetailsFmt::new(id, conversation, events)
            .with_local_flag(local)
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

        Ok(().into())
    }
}
