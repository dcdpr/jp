#![allow(clippy::print_stderr)]

use clap::Parser;
use grizzly::server::{GrizzlyService, ServerConfig};
use rmcp::{ServiceExt, transport::stdio};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Parser)]
struct Cli {
    /// Enable JP tool protocol (outputs as `jp_tool::Outcome` JSON).
    #[arg(long = "jp")]
    jp_protocol: bool,

    /// Enable the `note_create` tool (macOS only, writes to Bear via
    /// x-callback-url).
    #[arg(long)]
    note_create: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    // Log to stderr so stdout stays clean for MCP protocol
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_ansi(false)
                .compact(),
        )
        .init();

    tracing::info!(
        jp = cli.jp_protocol,
        note_create = cli.note_create,
        "Starting grizzly"
    );

    let config = ServerConfig {
        jp_protocol: cli.jp_protocol,
        note_create: cli.note_create,
    };

    let service = GrizzlyService::new(config)
        .serve(stdio())
        .await
        .map_err(|e| format!("Failed to start MCP server: {e}"))?;

    tracing::info!("grizzly ready");
    service.waiting().await?;

    Ok(())
}
