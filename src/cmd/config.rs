pub mod init;
use anyhow::Result;
use clap::{Args, Subcommand};
use exodus_trace::span;
use init::InitArgs;

use crate::context::Context;

#[derive(Args)]
pub struct ConfigArgs {
    /// Manage global configuration instead of project-local
    #[arg(long)]
    global: bool,

    #[command(subcommand)]
    command: ConfigCommands,
}

#[derive(Subcommand)]
enum ConfigCommands {
    /// Initialize a new configuration file
    Init(InitArgs),
}

pub async fn run(ctx: Context, args: &ConfigArgs) -> Result<()> {
    let _g = span!();

    match &args.command {
        ConfigCommands::Init(init_args) => init::run(ctx, args, init_args).await,
    }
}
