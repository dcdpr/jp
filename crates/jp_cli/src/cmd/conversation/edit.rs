use crossterm::style::Stylize as _;
use jp_conversation::ConversationId;

use crate::{cmd::Success, ctx::Ctx, Output};

#[derive(Debug, clap::Args)]
#[group(required = true, id = "edit")]
#[command(arg_required_else_help = true)]
pub struct Args {
    /// Conversation ID to edit. Defaults to active conversation.
    id: Option<ConversationId>,

    /// Toggle the private flag of the conversation.
    #[arg(long, group = "edit")]
    private: Option<Option<bool>>,

    /// Edit the title of the conversation.
    #[arg(long, group = "edit")]
    title: Option<Option<String>>,
}

impl Args {
    pub fn run(self, ctx: &mut Ctx) -> Output {
        let active_id = ctx.workspace.active_conversation_id();
        let id = self.id.unwrap_or(active_id);
        let Some(conversation) = ctx.workspace.get_conversation_mut(&id) else {
            return Err(
                format!("Conversation {} not found", id.to_string().bold().yellow()).into(),
            );
        };

        if let Some(private) = self.private {
            let private = private.unwrap_or(!conversation.private);
            conversation.private = private;
        }

        if let Some(title) = self.title {
            // TODO: `--title` without value should ask LLM for title(s)
            conversation.title = title;
        }

        Ok(Success::Message("Conversation updated.".into()))
    }
}
