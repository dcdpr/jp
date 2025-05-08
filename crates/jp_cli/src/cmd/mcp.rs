use super::Output;
use crate::ctx::Ctx;

mod attach;
mod detach;
mod edit;
mod list;
mod setup;

#[derive(Debug, clap::Args)]
pub struct Args {
    #[command(subcommand)]
    command: Commands,
}

impl Args {
    pub fn run(self, ctx: &mut Ctx) -> Output {
        match self.command {
            Commands::Setup(args) => args.run(ctx),
            Commands::Attach(args) => args.run(ctx),
            Commands::Detach(args) => args.run(ctx),
            Commands::List(args) => args.run(ctx),
            Commands::Edit(args) => args.run(ctx),
        }
    }
}

#[derive(Debug, clap::Subcommand)]
enum Commands {
    /// Add an MCP server configuration
    #[command(name = "setup")]
    Setup(setup::Args),

    /// Edit (or create) an MCP server configuration in your editor
    #[command(name = "edit")]
    Edit(edit::Args),

    /// Attach an MCP server to the current conversation
    #[command(name = "attach", alias = "a")]
    Attach(attach::Args),

    /// Detach an MCP server from the current conversation
    #[command(name = "detach", alias = "d")]
    Detach(detach::Args),

    /// List all MCP servers
    #[command(name = "ls")]
    List(list::Args),
}
