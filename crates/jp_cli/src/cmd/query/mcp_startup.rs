//! Waiting for configured MCP servers to start, and saying so on stderr.
//!
//! [`await_mcp_servers`] drives the wait and owns the status region it shows
//! while it runs.
//! [`report_skipped_servers`] is the other half: an optional server that failed
//! does not fail the query, so the tools it backed go missing unless the query
//! says which ones.

use std::{collections::HashMap, fmt::Write as _, sync::Arc, time::Duration};

use crossterm::style::Stylize as _;
use jp_config::{AppConfig, conversation::tool::ToolSource, style::mcp_startup::McpStartupConfig};
use jp_mcp::{StartupSet, id::McpServerId};
use jp_printer::{LineSink, PrintableExt as _, Printer, RegionStyle, StatusRegion};
use jp_term::width::{display_width, truncate_to_width};
use tokio::sync::broadcast::error::RecvError;

use crate::{cmd, render::tool::output_lines};

/// Wait for background MCP server startups to complete.
///
/// Shows an aggregate status row on stderr once the wait exceeds the configured
/// delay, updating the listed server names as startups finish, with a rolling
/// window of the servers' own stderr above it.
/// Servers that finish within the delay never trigger the row.
///
/// Returns the optional servers that failed and were skipped, so the caller can
/// account for the tools that went with them.
/// A required server's failure is returned as an error instead; the rows are
/// erased on the way out, so it renders on a clean line.
pub(super) async fn await_mcp_servers(
    mut startup: StartupSet,
    config: McpStartupConfig,
    printer: Arc<Printer>,
) -> Result<Vec<McpServerId>, cmd::Error> {
    if startup.joins.is_empty() {
        return Ok(Vec::new());
    }

    let region = claim_region(&printer, &config);
    region.set_detail(status(&startup.pending));

    // One sink per pending server, dropped the moment that server's join
    // completes. The forwarder behind the channel runs until the *server*
    // exits, which is long after it finished starting; a sink left open would
    // let a started server's operational logging evict the build output of one
    // still compiling.
    let mut sinks: HashMap<McpServerId, LineSink> = startup
        .pending
        .iter()
        .map(|id| (id.clone(), region.source(id.as_str())))
        .collect();

    let mut skipped = Vec::new();
    let mut lines_open = true;

    let result = loop {
        tokio::select! {
            line = startup.stderr.recv(), if lines_open => match line {
                Ok((id, text)) => if let Some(sink) = sinks.get(&id) {
                    sink.push(text);
                },
                // The window shows the most recent lines by definition, so
                // falling behind costs nothing worth reporting.
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => lines_open = false,
            },
            joined = startup.joins.join_next() => match joined {
                None => break Ok(()),
                Some(Err(error)) => break Err(cmd::Error::from(error)),
                Some(Ok(Err(error))) => break Err(cmd::Error::from(error)),
                Some(Ok(Ok(outcome))) => {
                    let id = outcome.id();
                    sinks.remove(id);
                    startup.pending.retain(|pending| pending != id);
                    if outcome.was_skipped() {
                        skipped.push(id.clone());
                    }
                    if !startup.pending.is_empty() {
                        region.set_detail(status(&startup.pending));
                    }
                }
            },
        }
    };

    result.map(|()| skipped)
}

/// Report optional MCP servers that failed to start.
///
/// A skipped server completes the wait successfully, so without this the query
/// quietly loses tools: the `warn!` explaining why goes to the trace log, which
/// is discarded unless the run itself fails.
///
/// Emitted whatever `style.mcp_startup.show` and `stderr_rows` say.
/// Those keys gate progress display; gating a failure report behind them would
/// reproduce the silence this closes.
///
/// `--format json` gets the parts rather than a sentence about them: a program
/// deciding what to do about a missing server reads `server` and `tools`, and
/// can render its own prose from them if it wants any.
pub(super) fn report_skipped_servers(
    printer: &Printer,
    config: &AppConfig,
    skipped: &[McpServerId],
) {
    for id in skipped {
        let tools = tools_backed_by(config, id);

        if printer.format().is_json() {
            printer.println_raw(skipped_server_record(printer, id, &tools).to_err());
            continue;
        }

        let mut line = format!("Optional MCP server '{id}' did not start");
        if !tools.is_empty() {
            let _err = write!(line, "; unavailable tools: {}", tools.join(", "));
        }
        line.push_str(" (run with -v for the reason)");

        printer.eprintln(line.yellow().to_string());
    }
}

/// Serialize one skipped-server report, indented when the format asks for it.
fn skipped_server_record(printer: &Printer, id: &McpServerId, tools: &[String]) -> String {
    let record = serde_json::json!({
        "event": "mcp_server_unavailable",
        "server": id.as_str(),
        "tools": tools,
    });

    if printer.format().is_json_pretty() {
        serde_json::to_string_pretty(&record)
    } else {
        serde_json::to_string(&record)
    }
    .unwrap_or_else(|_| record.to_string())
}

/// Names of the enabled tools sourced from `server`.
///
/// Sorted, so the report reads the same way twice.
fn tools_backed_by(config: &AppConfig, server: &McpServerId) -> Vec<String> {
    let mut names: Vec<String> = config
        .conversation
        .tools
        .iter()
        .filter(|(_, tool)| tool.is_enabled())
        .filter(|(_, tool)| match tool.source() {
            ToolSource::Mcp { server: name, .. } => &McpServerId::new(name.as_str()) == server,
            _ => false,
        })
        .map(|(name, _)| name.to_string())
        .collect();

    names.sort();
    names
}

/// Claim the status region for the MCP server startup wait.
///
/// Returns an inert region when `style.mcp_startup.show` is off, or when the
/// terminal cannot carry one.
fn claim_region(printer: &Printer, config: &McpStartupConfig) -> StatusRegion {
    if !config.show {
        return StatusRegion::inert();
    }

    // The row bounds itself rather than letting the region cut its tail: the
    // elapsed time lives at the end, and a long server list would take it with
    // it.
    let columns = printer.chrome_columns();

    printer.status_region(
        RegionStyle::new(
            Duration::from_secs(config.delay_secs.into()),
            Duration::from_millis(config.interval_ms.into()),
            move |secs, detail| line(secs, detail, columns),
        )
        .with_output(output_lines(config.stderr_rows)),
    )
}

/// Render the MCP startup status row for `secs` elapsed and `status`, bounding
/// the visible text to `width` columns when known.
///
/// Truncation falls on the server-list fragment only: the ` ⏱ Starting  `
/// prefix and the `  {secs:.1}s ` timer suffix are always preserved, so the
/// elapsed time keeps moving even when a long list overflows.
/// A terminal too narrow for even the prefix and suffix falls back to a bounded
/// `⏱ {secs:.1}s`.
fn line(secs: f64, status: Option<&str>, width: Option<u16>) -> String {
    let status = status.unwrap_or("MCP servers");
    let full = format!("⏱ Starting {status}… {secs:.1}s");
    match width {
        Some(w) if display_width(&full) > usize::from(w) => {
            let w = usize::from(w);
            let prefix = "⏱ Starting ";
            let suffix = format!(" {secs:.1}s");
            let reserved = display_width(prefix) + display_width(&suffix);
            if w <= reserved {
                truncate_to_width(&format!("⏱ {secs:.1}s"), w)
            } else {
                let status = truncate_to_width(status, w - reserved);
                format!("{prefix}{status}{suffix}")
            }
        }
        _ => full,
    }
}

/// Render the pending-server fragment for the MCP startup timer line.
///
/// One server renders as `MCP server bookworm`; several render as `2 MCP
/// servers (bookworm, grizzly)`.
fn status(pending: &[McpServerId]) -> String {
    match pending {
        [id] => format!("MCP server {id}"),
        ids => format!(
            "{} MCP servers ({})",
            ids.len(),
            ids.iter()
                .map(McpServerId::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

#[cfg(test)]
#[path = "mcp_startup_tests.rs"]
mod tests;
