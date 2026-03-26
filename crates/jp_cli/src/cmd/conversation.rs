use super::Output;
use crate::ctx::Ctx;

mod edit;
mod fork;
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
    pub(crate) async fn run(self, ctx: &mut Ctx) -> Output {
        match self.command {
            Commands::Show(args) => args.run(ctx),
            Commands::Remove(args) => args.run(ctx),
            Commands::List(args) => args.run(ctx),
            Commands::Use(args) => args.run(ctx),
            Commands::Edit(args) => args.run(ctx).await,
            Commands::Fork(args) => args.run(ctx),
            Commands::Grep(args) => args.run(ctx),
            Commands::Print(args) => args.run(ctx),
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
