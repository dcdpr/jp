use super::Output;
use crate::ctx::Ctx;

mod edit;
mod ls;
mod rm;
mod show;
mod use_;

#[derive(Debug, clap::Args)]
pub struct Args {
    #[command(subcommand)]
    command: Commands,
}

impl Args {
    pub fn run(self, ctx: &mut Ctx) -> Output {
        match self.command {
            Commands::Show(args) => args.run(ctx),
            Commands::Remove(args) => args.run(ctx),
            Commands::List(args) => args.run(ctx),
            Commands::Use(args) => args.run(ctx),
            Commands::Edit(args) => args.run(ctx),
        }
    }
}

#[derive(Debug, clap::Subcommand)]
enum Commands {
    /// Remove conversations.
    #[command(name = "rm")]
    Remove(rm::Args),

    /// List conversations.
    #[command(name = "ls")]
    List(ls::Args),

    /// Show conversation details.
    #[command(name = "show")]
    Show(show::Args),

    /// Set the active conversation.
    #[command(name = "use")]
    Use(use_::Args),

    /// Edit conversation details.
    #[command(name = "edit")]
    Edit(edit::Args),
}
