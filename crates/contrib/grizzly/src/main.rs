#![allow(clippy::print_stderr, clippy::print_stdout)]

use clap::{Parser, Subcommand};
use grizzly::{
    BearDb, SearchParams,
    server::{GrizzlyService, ServerConfig},
};
use rmcp::{ServiceExt, transport::stdio};
use serde::Serialize;
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

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Search notes and print the matches as JSON.
    ///
    /// The same query the `note_search` tool runs, for callers that speak a
    /// shell rather than MCP.
    Search {
        /// Text to match against titles and content.
        /// Repeatable.
        /// Omit it to filter on tags alone.
        query: Vec<String>,

        /// Only notes carrying all of these tags, written with or without the
        /// leading `#`.
        /// Repeatable.
        #[arg(long = "tag")]
        tags: Vec<String>,

        /// Only notes created on or after this day, as `YYYY-MM-DD`.
        #[arg(long)]
        created_after: Option<String>,

        /// Most notes to return.
        #[arg(long, default_value_t = 50)]
        limit: usize,

        /// Include each note's full content.
        ///
        /// Costs a second read per batch, so it is off by default.
        #[arg(long)]
        full: bool,
    },
}

/// One match in `search` output.
#[derive(Serialize)]
struct Match {
    id: String,
    title: String,
    tags: Vec<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    archived: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
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

    match cli.command {
        Some(command) => run(command),
        None => serve(&cli).await,
    }
}

/// Answer one query and exit.
fn run(command: Command) -> Result<(), Box<dyn std::error::Error>> {
    let Command::Search {
        query,
        tags,
        created_after,
        limit,
        full,
    } = command;

    let db = BearDb::open()?;
    let matches = db.search(&SearchParams {
        queries: query,
        tags: tags.iter().map(|tag| tag_name(tag).to_owned()).collect(),
        created_after,
        limit,
        ..Default::default()
    })?;

    // Content lives on the notes rather than the matches, so asking for it
    // costs one more read for the whole batch.
    let contents = if full {
        let ids: Vec<&str> = matches.iter().map(|found| found.note_id.as_str()).collect();
        db.get_notes(&ids)?
            .into_iter()
            .map(|note| (note.id, note.content.unwrap_or_default()))
            .collect()
    } else {
        std::collections::HashMap::new()
    };

    let output: Vec<Match> = matches
        .into_iter()
        .map(|found| Match {
            content: contents.get(&found.note_id).cloned(),
            id: found.note_id,
            title: found.title,
            tags: found.tags,
            created_at: found.created_at,
            updated_at: found.updated_at,
            archived: found.is_archived,
        })
        .collect();

    println!("{}", serde_json::to_string_pretty(&output)?);

    Ok(())
}

/// Read a tag as Bear stores it, without the `#` that writes it.
fn tag_name(tag: &str) -> &str {
    tag.strip_prefix('#').unwrap_or(tag)
}

/// Serve the MCP tools over stdio until the client goes away.
async fn serve(cli: &Cli) -> Result<(), Box<dyn std::error::Error>> {
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
