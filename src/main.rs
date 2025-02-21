use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use exodus_trace::debug;
use jp::{
    cmd::{self, ask::AskArgs, canonical_path, config::ConfigArgs, serve::ServeArgs},
    context::Context,
    workspace::Workspace,
    Config,
};

// CLI command structure
#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Optional path to config file
    #[arg(short, long, value_parser = canonical_path)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Ask a question and get a response
    Ask(AskArgs),
    /// Start a server for API access
    Serve(ServeArgs),
    /// Manage configuration files
    Config(ConfigArgs),
}

#[tokio::main]
async fn main() -> Result<()> {
    let _guard = exodus_trace::init(None);

    dotenv::dotenv().ok();
    let cli = Cli::parse();

    let config = Config::load(cli.config.as_deref())?;
    let workspace = Workspace::load()
        .inspect_err(|err| debug!("Could not find workspace, using non-workspace workflow: {err}"))
        .ok();

    let ctx = Context { config, workspace };

    match &cli.command {
        Commands::Ask(args) => {
            cmd::ask::run(ctx, args).await?;
        }
        Commands::Serve(args) => {
            cmd::serve::run(ctx, args).await?;
        }
        Commands::Config(args) => {
            cmd::config::run(ctx, args).await?;
        }
    }

    Ok(())
}
