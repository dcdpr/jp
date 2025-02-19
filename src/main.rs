mod ask;
mod chat;
mod config;
mod openrouter;
mod reasoning;
mod server;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use log::info;
use openrouter::Client;
use std::env;
use std::sync::Arc;

// CLI command structure
#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Optional path to config file (defaults to ./clauder.toml)
    #[arg(short, long)]
    config: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Ask a question and get a response
    Ask {
        /// The question to ask
        #[arg(required = true)]
        question: String,
    },
    /// Start a server for API access
    Serve {
        /// Port to listen on (overrides config file)
        #[arg(short, long)]
        port: Option<u16>,
    },
    /// Generate a default config file
    Init {
        /// Path where to save the config file
        #[arg(default_value = "./clauder.toml")]
        path: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    env_logger::init();

    // Load environment variables
    dotenv::dotenv().ok();

    // Parse CLI arguments
    let cli = Cli::parse();

    // Load config
    let mut config = config::Config::load(cli.config.as_deref())?;

    match cli.command {
        Commands::Ask { question } => {
            info!("Ask command invoked.");
            let client = Client::from_config(&config)?;
            ask::process_question(&client, &config, &question).await?;
        }
        Commands::Serve { port } => {
            if let Some(port) = port {
                config.server.port = port;
            }

            info!("Starting server on port {}", config.server.port);
            server::start_server(config.into()).await?;
        }
        Commands::Init { path } => {
            info!("Generating default config file at {}", path);
            config::generate_default_config(&path)?;
            println!("Config file generated successfully at {}", path);
        }
    }

    Ok(())
}
