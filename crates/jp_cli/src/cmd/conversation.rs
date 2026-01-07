use super::Output;
use crate::ctx::Ctx;

mod edit;
mod fork;
mod ls;
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
        }
    }
}

#[derive(Debug, clap::Subcommand)]
enum Commands {
    /// Remove conversations.
    #[command(name = "rm")]
    Remove(rm::Rm),

    /// List conversations.
    #[command(name = "ls")]
    List(ls::Ls),

    /// Show conversation details.
    #[command(name = "show")]
    Show(show::Show),

    /// Set the active conversation.
    #[command(name = "use")]
    Use(use_::Use),

    /// Edit conversation details.
    #[command(name = "edit")]
    Edit(edit::Edit),

    /// Fork a conversation.
    Fork(fork::Fork),
}
