use std::path::PathBuf;

use bookworm::{
    dl, index,
    mcp::{BookwormService, ServerConfig},
};
use clap::{Parser, Subcommand};
use rmcp::{ServiceExt, transport::stdio};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Parser)]
#[command(name = "bookworm", about = "Rust crate documentation MCP server", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the MCP server over stdio.
    Mcp {
        /// Enable JP tool protocol (outputs as `jp_tool::Outcome` JSON).
        #[arg(long = "jp")]
        jp_protocol: bool,
    },

    /// Download crate documentation from docs.rs.
    Download {
        /// Name of the crate to download documentation for.
        crate_name: String,

        /// Version of the crate (defaults to "latest").
        #[arg(short, long)]
        version: Option<String>,

        /// Root directory to save the documentation to (defaults to temp dir).
        #[arg(short, long)]
        root: Option<PathBuf>,
    },

    /// Index locally stored crate documentation into a SQLite database.
    Index {
        /// Path to the documentation directory to index.
        source: PathBuf,

        /// Path to save the SQLite database to (defaults to ./index.sqlite).
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_tracing()?;

    let cli = Cli::parse();

    match cli.command {
        Command::Mcp { jp_protocol } => run_mcp(jp_protocol).await,
        Command::Download {
            crate_name,
            version,
            root,
        } => run_download(crate_name, version, root).await,
        Command::Index { source, output } => run_index(source, output),
    }
}

async fn run_mcp(jp_protocol: bool) -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!(jp = jp_protocol, "Starting bookworm MCP server");

    let config = ServerConfig { jp_protocol };
    let service = BookwormService::new(config)
        .serve(stdio())
        .await
        .map_err(|e| format!("Failed to start MCP server: {e}"))?;

    tracing::info!("bookworm ready");
    service.waiting().await?;

    Ok(())
}

async fn run_download(
    crate_name: String,
    version: Option<String>,
    root: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut config = dl::Config::default()
        .crate_name(crate_name)
        .client(reqwest::Client::new());

    if let Some(v) = version {
        config = config.version(v);
    }
    if let Some(r) = root {
        config = config.root(r);
    }

    let path = dl::download(config).await?;
    tracing::info!(
        path = %path.to_string_lossy(),
        "Documentation downloaded"
    );

    Ok(())
}

fn run_index(source: PathBuf, output: Option<PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
    let output = output.unwrap_or_else(|| PathBuf::from("index.sqlite"));
    let config = index::Config::default().source(source).output(&output);

    index::index(config)?;
    tracing::info!(
        path = %output.display(),
        "Documentation indexed"
    );

    Ok(())
}

/// Configure tracing. Logs go to stderr (so the MCP protocol on stdout stays
/// clean), or to `WRM_LOG_FILE` if set and its parent dir exists. Filter is
/// read from `WRM_LOG`, defaulting to `info`.
fn init_tracing() -> Result<(), Box<dyn std::error::Error>> {
    let filter = EnvFilter::try_from_env("WRM_LOG").unwrap_or_else(|_| EnvFilter::new("info"));

    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_line_number(true)
        .with_ansi(false)
        .compact();

    if let Ok(file) = std::env::var("WRM_LOG_FILE").map(PathBuf::from)
        && file.parent().is_some_and(std::path::Path::is_dir)
    {
        let writer = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file)?;

        tracing_subscriber::registry()
            .with(filter)
            .with(fmt_layer.with_writer(writer))
            .init();
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(fmt_layer.with_writer(std::io::stderr))
            .init();
    }

    Ok(())
}
