use std::sync::Arc;

use jp_config::{
    AppConfig,
    conversation::tool::{PartialEnableConfig, PartialToolConfig},
    style::stderr_rows::{RowCount, StderrRows},
    util::build,
};
use jp_mcp::{Startup, StderrLine};
use jp_printer::{OutputFormat, Printer, SharedBuffer, TerminalCapability};
use jp_term::width::display_width;
use tokio::sync::broadcast;

use super::*;

#[test]
fn status_names_a_single_server() {
    assert_eq!(
        status(&[McpServerId::new("bookworm")]),
        "MCP server bookworm"
    );
}

#[test]
fn status_counts_and_lists_several_servers() {
    assert_eq!(
        status(&[McpServerId::new("bookworm"), McpServerId::new("grizzly")]),
        "2 MCP servers (bookworm, grizzly)"
    );
}

/// Timer settings that render immediately, so tests don't wait out a delay.
fn immediate_config() -> McpStartupConfig {
    McpStartupConfig {
        show: true,
        delay_secs: 0,
        interval_ms: 10,
        // Most of these cases assert on the status row alone; the ones that
        // exercise the window override this.
        stderr_rows: StderrRows::Off,
    }
}

/// A startup wait that shows two window rows above the status row.
fn windowed_config() -> McpStartupConfig {
    McpStartupConfig {
        stderr_rows: StderrRows::Fixed(RowCount { rows: 2 }),
        ..immediate_config()
    }
}

/// A startup set over `joins`, plus the sender a test can feed stderr through.
///
/// Callers that don't exercise the window drop the sender, which closes the
/// channel; the wait treats that as "no more lines" rather than an error.
fn startup_set(
    joins: tokio::task::JoinSet<Result<Startup, jp_mcp::Error>>,
    pending: Vec<McpServerId>,
) -> (StartupSet, broadcast::Sender<StderrLine>) {
    let (tx, rx) = broadcast::channel(64);

    (
        StartupSet {
            joins,
            pending,
            stderr: rx,
        },
        tx,
    )
}

/// Poll `err` until `needle` appears, failing after a hard timeout.
///
/// Synchronizes on the rendered output instead of a fixed sleep: the timer
/// writes frames from its own task, so tests wait for the frame to land rather
/// than guessing how long that takes.
async fn wait_for_frame(err: &SharedBuffer, needle: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !err.lock().contains(needle) {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("frame {needle:?} never rendered"));
}

/// An `AppConfig` whose `search` tool is backed by the `bookworm` MCP server.
fn config_with_mcp_tool(enabled: bool) -> AppConfig {
    let mut partial = AppConfig::new_test().to_partial();
    partial
        .conversation
        .tools
        .tools
        .insert("search".to_owned(), PartialToolConfig {
            source: Some(ToolSource::Mcp {
                server: "bookworm".to_owned(),
                tool: None,
            }),
            enable: Some(PartialEnableConfig {
                state: Some(enabled),
                ..PartialEnableConfig::default()
            }),
            ..PartialToolConfig::default()
        });

    build(partial).expect("the fixture config resolves")
}

#[tokio::test]
async fn drains_all_startups() {
    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);

    let mut joins = tokio::task::JoinSet::new();
    joins.spawn(async { Ok(Startup::Ready(McpServerId::new("bookworm"))) });
    joins.spawn(async { Ok(Startup::Ready(McpServerId::new("grizzly"))) });
    let (startup, _lines) = startup_set(joins, vec![
        McpServerId::new("bookworm"),
        McpServerId::new("grizzly"),
    ]);

    let skipped = await_mcp_servers(startup, immediate_config(), Arc::new(printer))
        .await
        .expect("all startups succeed");

    assert!(skipped.is_empty(), "no server was skipped");
}

#[tokio::test]
async fn propagates_startup_error() {
    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);

    let mut joins = tokio::task::JoinSet::new();
    joins.spawn(async { Err(jp_mcp::Error::UnknownServer(McpServerId::new("bookworm"))) });
    let (startup, _lines) = startup_set(joins, vec![McpServerId::new("bookworm")]);

    let error = await_mcp_servers(startup, immediate_config(), Arc::new(printer))
        .await
        .expect_err("a failed required server must fail the wait");

    assert_eq!(error.message.as_deref(), Some("MCP error"));
}

#[tokio::test(flavor = "multi_thread")]
async fn shows_and_clears_the_timer_line() {
    let (printer, _out, err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer.with_terminal(TerminalCapability::interactive(Some(80))));

    // Hold the startup window open until the test releases it, so the timer
    // is guaranteed to tick while the server is still "starting".
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let mut joins = tokio::task::JoinSet::new();
    joins.spawn(async move {
        release_rx.await.ok();
        Ok(Startup::Ready(McpServerId::new("bookworm")))
    });
    let (startup, _lines) = startup_set(joins, vec![McpServerId::new("bookworm")]);

    let wait = tokio::spawn(await_mcp_servers(
        startup,
        immediate_config(),
        printer.clone(),
    ));

    // Let a few ticks land before releasing the startup.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    release_tx.send(()).expect("wait task is still running");
    wait.await
        .expect("task did not panic")
        .expect("startup succeeds");
    printer.flush();

    let chrome = err.lock();
    assert!(
        chrome.contains("⏱ Starting MCP server bookworm…"),
        "timer line should name the pending server.\nChrome:\n{chrome}"
    );
    assert!(
        chrome.ends_with("\r\x1b[K"),
        "finishing the wait must leave the line cleared.\nChrome:\n{chrome}"
    );
}

#[test]
fn skipped_server_report_names_the_tools_that_went_with_it() {
    let (printer, _out, err) = Printer::memory(OutputFormat::TextPretty);

    report_skipped_servers(&printer, &config_with_mcp_tool(true), &[McpServerId::new(
        "bookworm",
    )]);
    printer.flush();

    let chrome = err.lock();
    assert!(
        chrome.contains("Optional MCP server 'bookworm' did not start"),
        "the report must name the server.\nChrome:\n{chrome}"
    );
    assert!(
        chrome.contains("unavailable tools: search"),
        "the report must name the tools that went with it.\nChrome:\n{chrome}"
    );
    assert!(
        chrome.contains("-v"),
        "the report must point at where the reason lives.\nChrome:\n{chrome}"
    );
}

#[test]
fn skipped_server_report_is_ndjson_under_json_format() {
    let (printer, _out, err) = Printer::memory(OutputFormat::Json);

    report_skipped_servers(&printer, &config_with_mcp_tool(true), &[McpServerId::new(
        "bookworm",
    )]);
    printer.flush();

    let chrome = err.lock().clone();
    let parsed: serde_json::Value =
        serde_json::from_str(chrome.trim()).expect("chrome is one NDJSON record");

    assert_eq!(parsed["event"], "mcp_server_unavailable");
    assert_eq!(parsed["server"], "bookworm");
    assert_eq!(parsed["tools"][0], "search");
}

#[test]
fn skipped_server_report_skips_disabled_tools() {
    let (printer, _out, err) = Printer::memory(OutputFormat::TextPretty);

    report_skipped_servers(&printer, &config_with_mcp_tool(false), &[McpServerId::new(
        "bookworm",
    )]);
    printer.flush();

    let chrome = err.lock();
    assert!(
        chrome.contains("Optional MCP server 'bookworm' did not start"),
        "the server is still reported.\nChrome:\n{chrome}"
    );
    assert!(
        !chrome.contains("unavailable tools"),
        "a tool that was already off did not become unavailable.\nChrome:\n{chrome}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn shows_server_stderr_while_it_starts() {
    let (printer, _out, err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(
        printer.with_terminal(TerminalCapability::interactive(Some(80)).with_rows(Some(24))),
    );

    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let mut joins = tokio::task::JoinSet::new();
    joins.spawn(async move {
        release_rx.await.ok();
        Ok(Startup::Ready(McpServerId::new("bookworm")))
    });
    let (startup, lines) = startup_set(joins, vec![McpServerId::new("bookworm")]);

    let wait = tokio::spawn(await_mcp_servers(
        startup,
        windowed_config(),
        printer.clone(),
    ));

    lines
        .send((McpServerId::new("bookworm"), "Compiling serde".to_owned()))
        .expect("the wait holds a receiver");
    wait_for_frame(&err, "Compiling serde").await;

    release_tx.send(()).expect("wait task is still running");
    wait.await
        .expect("task did not panic")
        .expect("startup succeeds");
    printer.flush();

    let chrome = err.lock();
    assert!(
        chrome.contains("⏱ Starting MCP server bookworm…"),
        "the status row still names the pending server.\nChrome:\n{chrome}"
    );
    assert!(
        !chrome.contains("[bookworm]"),
        "a single source renders verbatim, without a label.\nChrome:\n{chrome}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn window_lines_are_labelled_once_two_servers_contribute() {
    let (printer, _out, err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(
        printer.with_terminal(TerminalCapability::interactive(Some(80)).with_rows(Some(24))),
    );

    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let mut joins = tokio::task::JoinSet::new();
    joins.spawn(async move {
        release_rx.await.ok();
        Ok(Startup::Ready(McpServerId::new("bookworm")))
    });
    let (startup, lines) = startup_set(joins, vec![
        McpServerId::new("bookworm"),
        McpServerId::new("grizzly"),
    ]);

    let wait = tokio::spawn(await_mcp_servers(
        startup,
        windowed_config(),
        printer.clone(),
    ));

    // Interleaved output from two sources is worse than none unlabelled: it
    // misattributes progress.
    lines
        .send((McpServerId::new("bookworm"), "Compiling serde".to_owned()))
        .expect("the wait holds a receiver");
    lines
        .send((McpServerId::new("grizzly"), "Compiling tantivy".to_owned()))
        .expect("the wait holds a receiver");
    // Labelling only starts once the window holds two sources, so the first
    // label appearing means both lines have landed.
    wait_for_frame(&err, "[bookworm]").await;

    release_tx.send(()).expect("wait task is still running");
    wait.await
        .expect("task did not panic")
        .expect("startup succeeds");
    printer.flush();

    // The label's own colour is `jp_printer`'s business; what matters here is
    // that each line carries its source's name, padded to line up, and that the
    // colour closes before the source's own text starts.
    let chrome = err.lock();
    assert!(
        chrome.contains("[bookworm]\x1b[39m Compiling serde"),
        "the first source must be labelled.\nChrome:\n{chrome}"
    );
    assert!(
        chrome.contains("[grizzly ]\x1b[39m Compiling tantivy"),
        "the second source must be labelled and aligned.\nChrome:\n{chrome}"
    );
}

#[tokio::test]
async fn reports_skipped_optional_servers() {
    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);

    let mut joins = tokio::task::JoinSet::new();
    joins.spawn(async { Ok(Startup::Skipped(McpServerId::new("bookworm"))) });
    joins.spawn(async { Ok(Startup::Ready(McpServerId::new("grizzly"))) });
    let (startup, _lines) = startup_set(joins, vec![
        McpServerId::new("bookworm"),
        McpServerId::new("grizzly"),
    ]);

    let skipped = await_mcp_servers(startup, immediate_config(), Arc::new(printer))
        .await
        .expect("an optional failure completes the wait");

    assert_eq!(skipped, vec![McpServerId::new("bookworm")]);
}

/// Drives the aggregate redraw: two servers start, one finishes while the other
/// is still pending, then the second finishes.
/// The line must go from both names, to the survivor alone, to cleared.
#[tokio::test(flavor = "multi_thread")]
async fn redraws_as_servers_finish() {
    let (printer, _out, err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer.with_terminal(TerminalCapability::interactive(Some(80))));

    // Two independently-released tasks: releasing `bookworm` first makes
    // `grizzly` the deterministic survivor of the mid-drain redraw.
    let (bookworm_tx, bookworm_rx) = tokio::sync::oneshot::channel::<()>();
    let (grizzly_tx, grizzly_rx) = tokio::sync::oneshot::channel::<()>();
    let mut joins = tokio::task::JoinSet::new();
    joins.spawn(async move {
        bookworm_rx.await.ok();
        Ok(Startup::Ready(McpServerId::new("bookworm")))
    });
    joins.spawn(async move {
        grizzly_rx.await.ok();
        Ok(Startup::Ready(McpServerId::new("grizzly")))
    });
    let (startup, _lines) = startup_set(joins, vec![
        McpServerId::new("bookworm"),
        McpServerId::new("grizzly"),
    ]);

    let wait = tokio::spawn(await_mcp_servers(
        startup,
        immediate_config(),
        printer.clone(),
    ));

    // Advance on the rendered frames, not the clock: wait until each frame is
    // actually in the buffer before releasing the next server, so a slow timer
    // task can't make the release outrun the redraw it's supposed to observe.
    wait_for_frame(&err, "2 MCP servers (bookworm, grizzly)").await;
    bookworm_tx.send(()).expect("wait task is still running");
    wait_for_frame(&err, "MCP server grizzly…").await;
    grizzly_tx.send(()).expect("wait task is still running");
    wait.await
        .expect("task did not panic")
        .expect("all startups succeed");
    printer.flush();

    let chrome = err.lock();
    let both = chrome
        .find("2 MCP servers (bookworm, grizzly)")
        .expect("the aggregate two-server frame must render first");
    let survivor = chrome
        .find("MCP server grizzly…")
        .expect("the survivor-only frame must render after bookworm finishes");
    assert!(
        both < survivor,
        "the two-server frame must precede the survivor-only frame.\nChrome:\n{chrome}"
    );
    assert!(
        !chrome.contains("MCP server bookworm…"),
        "bookworm was never the sole pending server; it must not render alone.\nChrome:\n{chrome}"
    );
    assert!(
        chrome.ends_with("\r\x1b[K"),
        "finishing the wait must leave the line cleared.\nChrome:\n{chrome}"
    );
}

#[test]
fn line_renders_full_when_it_fits() {
    assert_eq!(
        line(4.2, Some("MCP server bookworm"), Some(80)),
        "⏱ Starting MCP server bookworm… 4.2s"
    );
    // Unknown width leaves the line unbounded.
    assert_eq!(
        line(4.2, Some("MCP server bookworm"), None),
        "⏱ Starting MCP server bookworm… 4.2s"
    );
}

// A long server list forced to truncate must keep the elapsed-time suffix: the
// whole point of the line is the moving timer, so truncation has to fall on the
// server list, not the `Ns` tail. Testing the pure formatter at a fixed `secs`
// pins the invariant without depending on when the timer task first ticks.
#[test]
fn line_truncation_preserves_timer_suffix() {
    let long = "MCP server bookworm-with-a-very-long-descriptive-server-name";
    let rendered = line(12.3, Some(long), Some(30));

    assert!(
        rendered.ends_with(" 12.3s"),
        "suffix must survive: {rendered:?}"
    );
    assert!(
        rendered.contains('…'),
        "server list must truncate: {rendered:?}"
    );
    assert!(
        display_width(&rendered) <= 30,
        "must fit width: {rendered:?}"
    );
}

// A terminal too narrow for even the prefix and suffix still keeps a moving
// timer rather than a static stub.
#[test]
fn line_ultra_narrow_keeps_bounded_timer() {
    let rendered = line(7.0, Some("MCP server bookworm"), Some(6));

    assert!(
        display_width(&rendered) <= 6,
        "must fit width: {rendered:?}"
    );
    assert!(
        rendered.contains("7.0s"),
        "timer must survive: {rendered:?}"
    );
}
