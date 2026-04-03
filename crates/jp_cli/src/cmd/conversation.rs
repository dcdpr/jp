use jp_workspace::ConversationHandle;

use super::{ConversationLoadRequest, Output};
use crate::ctx::Ctx;

mod edit;
pub(crate) mod fork;
mod grep;
mod ls;
mod print;
mod rm;
mod show;
mod use_;

#[derive(Debug, clap::Args)]
pub(crate) struct Conversation {
    #[command(subcommand)]
    command: Commands,
}

impl Conversation {
    pub(crate) async fn run(self, ctx: &mut Ctx, handles: Vec<ConversationHandle>) -> Output {
        match self.command {
            Commands::Show(args) => args.run(ctx, handles),
            Commands::Remove(args) => args.run(ctx, handles),
            Commands::Edit(args) => args.run(ctx, handles).await,
            Commands::Fork(args) => args.run(ctx, &handles),
            Commands::Grep(args) => args.run(ctx, handles),
            Commands::Print(args) => args.run(ctx, &handles),
            Commands::List(args) => args.run(ctx, handles),
            Commands::Use(args) => args.run(
                ctx,
                handles.into_iter().next().expect("Use requires a handle"),
            ),
        }
    }

    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        match &self.command {
            Commands::Show(args) => args.conversation_load_request(),
            Commands::Remove(args) => args.conversation_load_request(),
            Commands::Edit(args) => args.conversation_load_request(),
            Commands::Fork(args) => args.conversation_load_request(),
            Commands::Grep(args) => args.conversation_load_request(),
            Commands::Print(args) => args.conversation_load_request(),
            Commands::List(args) => args.conversation_load_request(),
            Commands::Use(args) => args.conversation_load_request(),
        }
    }
}

#[derive(Debug, clap::Subcommand)]
enum Commands {
    /// Remove conversations.
    #[command(name = "rm", aliases = ["remove", "rem", "delete", "del"])]
    Remove(rm::Rm),

    /// List conversations.
    #[command(name = "ls", alias = "list", visible_alias = "l")]
    List(ls::Ls),

    /// Show conversation details.
    #[command(name = "show", visible_alias = "s")]
    Show(show::Show),

    /// Set the active conversation.
    #[command(name = "use", visible_alias = "u")]
    Use(use_::Use),

    /// Edit conversation details.
    #[command(name = "edit", visible_alias = "e")]
    Edit(edit::Edit),

    /// Fork a conversation.
    #[command(name = "fork", visible_alias = "f")]
    Fork(fork::Fork),

    /// Search through conversation history.
    #[command(name = "grep", alias = "rg", visible_alias = "g")]
    Grep(grep::Grep),

    /// Print conversation history to the terminal.
    #[command(name = "print", visible_alias = "p")]
    Print(print::Print),
    // /// Merge a conversation.
    // Merge(merge::Merge),

    // /// Rollback a conversation.
    // Rollback(rollback::Rollback),
}
