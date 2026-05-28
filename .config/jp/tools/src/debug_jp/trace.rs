//! `debug_jp_trace` — capture and render `JP_DEBUG=1` trace logs.
//!
//! Launches `jp` inside the sandbox with `JP_DEBUG=1` set. `jp_cli` persists
//! its tracing-subscriber output to a system temp file and prints
//! `Full trace log written to: <path>` on stderr at exit. We parse that
//! line, copy the file out to the real workspace so it survives sandbox
//! cleanup, filter the events by level/target/grep, and render the result
//! in a compact logfmt-like format.

use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

use camino::{Utf8Path, Utf8PathBuf};
use jp_tool::Outcome;
use serde_json::Value;

use crate::{
    Context, Error, Tool,
    debug_jp::util::{
        build::{self, BuildSpec},
        launch::{self, LaunchSpec},
        sandbox::{Sandbox, SandboxOpts},
        trace_parse::{self, Level, TraceEvent},
        trace_render::{self, OutputPaths},
    },
    util::{ToolResult, error},
};

/// Marker line `jp_cli::run` writes to stderr when `JP_DEBUG=1` and the
/// output format is text (we never pass `--format=json`, so this is what
/// we get).
const TRACE_PATH_PREFIX: &str = "Full trace log written to: ";

/// Tool entrypoint.
/// Dispatches between the format-args preview and the live execution.
#[allow(clippy::unused_async, reason = "awaited by the debug_jp dispatcher")]
pub(crate) async fn debug_jp_trace(ctx: &Context, t: &Tool) -> ToolResult {
    let args: Vec<String> = t.req("args")?;
    let level_str: Option<String> = t.opt("level")?;
    let target_filter: Option<String> = t.opt("target")?;
    let grep: Option<String> = t.opt("grep")?;
    let clone_user_data: bool = t.opt::<bool>("clone_user_data")?.unwrap_or(true);

    if args.is_empty() {
        return error("`args` must contain at least one element (the jp subcommand).");
    }

    let level = match level_str.as_deref() {
        None => Level::Info,
        Some(s) => match Level::parse(s) {
            Some(level) => level,
            None => {
                return error(format!(
                    "Invalid `level`: '{s}'. Must be one of: trace, debug, info, warn, error."
                ));
            }
        },
    };

    if ctx.action.is_format_arguments() {
        return Ok(format_preview(
            &args,
            level,
            target_filter.as_deref(),
            grep.as_deref(),
            clone_user_data,
        )
        .into());
    }

    run(
        &ctx.root,
        &args,
        level,
        target_filter.as_deref(),
        grep.as_deref(),
        clone_user_data,
    )
}

/// Render the format-args preview shown before execution.
fn format_preview(
    args: &[String],
    level: Level,
    target_filter: Option<&str>,
    grep: Option<&str>,
    clone_user_data: bool,
) -> String {
    let mut out = String::new();
    out.push_str("`debug_jp_trace`\n\n");
    out.push_str("Will execute (under sandbox isolation):\n\n");
    out.push_str("```sh\n");
    out.push_str("cargo build --profile profiling -p jp_cli --bin jp\n");
    out.push_str(&format!(
        "JP_DEBUG=1 target/profiling/jp {}    # invoked by absolute path\n",
        args.join(" ")
    ));
    out.push_str("```\n\n");
    out.push_str(
        "The build artifact at `target/profiling/jp` may be rebuilt to reflect\n\
         the sandbox's source tree (HEAD + uncommitted changes from this worktree).\n\
         The installed `jp` binary (`~/.cargo/bin/jp` etc.) is **not** touched.\n\n",
    );

    out.push_str("Filters:\n\n");
    out.push_str(&format!(
        "- **Level:** {} (events below this level are excluded)\n",
        level.as_str()
    ));
    if let Some(t) = target_filter {
        out.push_str(&format!(
            "- **Target:** substring `{t}` (case-insensitive)\n"
        ));
    }
    if let Some(g) = grep {
        out.push_str(&format!("- **Grep:** substring `{g}` (case-insensitive)\n"));
    }
    out.push('\n');

    out.push_str("Isolation:\n\n");
    out.push_str("- Workspace: detached `git worktree` under `tmp/jp-sandbox-<ts>/`\n");
    out.push_str("- User data: scratch dir under `tmp/jp-sandbox-data-<ts>/`,\n");
    out.push_str("  bound via `JP_USER_DATA_DIR`. ");
    if clone_user_data {
        out.push_str("Current user data is **cloned** into it.\n");
    } else {
        out.push_str("Sandbox starts with **empty** user data.\n");
    }
    out.push_str("- Sandbox is removed on tool exit (best-effort `Drop`).\n\n");

    out.push_str("Outputs (in the real workspace):\n\n");
    out.push_str("- `tmp/profiling/trace-<ts>.jsonl` — raw JSON-per-line trace log\n");
    out.push_str("- `tmp/profiling/report-trace-<ts>.md` — rendered report\n\n");

    out.push_str("Returns the rendered Markdown report inline.\n");
    out
}

/// Live execution path. Builds jp, launches with `JP_DEBUG=1`, retrieves the
/// trace log, parses + filters + renders.
fn run(
    workspace_root: &Utf8Path,
    args: &[String],
    level: Level,
    target_filter: Option<&str>,
    grep: Option<&str>,
    clone_user_data: bool,
) -> ToolResult {
    let sandbox = Sandbox::create(workspace_root, SandboxOpts { clone_user_data })?;

    let binary = build::build(&BuildSpec {
        working_dir: sandbox.working_dir(),
        package: "jp_cli",
        bin: "jp",
        profile: "profiling",
        features: &[],
    })?;

    let mut env = sandbox.env();
    env.push(("JP_DEBUG".to_owned(), "1".to_owned()));
    let handle = launch::spawn(&LaunchSpec {
        binary,
        args: args.to_vec(),
        working_dir: sandbox.working_dir().to_owned(),
        env,
    })?;
    let launch_result = handle.wait()?;

    // jp prints `Full trace log written to: <path>` on stderr right before
    // exit. The path is in the system temp dir (e.g. `/var/folders/...`),
    // outside the sandbox.
    let trace_src = launch_result
        .stderr
        .lines()
        .find_map(|line| line.strip_prefix(TRACE_PATH_PREFIX))
        .ok_or_else(|| {
            format!(
                "Did not find `{TRACE_PATH_PREFIX}<path>` in jp's stderr. \
                 jp may have exited before the tracing layer flushed, or \
                 stderr was redirected. Last 20 lines of stderr:\n{}",
                tail_lines(&launch_result.stderr, 20)
            )
        })?;
    let trace_src = Utf8PathBuf::from(trace_src.trim());

    // Copy the trace log into the real workspace so it survives the system
    // temp dir's eventual cleanup and stays alongside other profile output.
    // Stdout/stderr are dumped alongside so the whole run — trace +
    // streams — is one cohesive set of files keyed by `<ts>`.
    let ts = unix_seconds();
    let out_dir = workspace_root.join("tmp/profiling");
    fs::create_dir_all(&out_dir)?;
    let trace_dst = out_dir.join(format!("trace-{ts}.jsonl"));
    let stdout_dst = out_dir.join(format!("trace-{ts}-stdout.txt"));
    let stderr_dst = out_dir.join(format!("trace-{ts}-stderr.txt"));
    let report_dst = out_dir.join(format!("report-trace-{ts}.md"));
    fs::copy(&trace_src, &trace_dst)
        .map_err(|e| format!("Failed to copy trace log from {trace_src} to {trace_dst}: {e}"))?;
    fs::write(&stdout_dst, &launch_result.stdout)
        .map_err(|e| format!("Failed to write stdout capture to {stdout_dst}: {e}"))?;
    fs::write(&stderr_dst, &launch_result.stderr)
        .map_err(|e| format!("Failed to write stderr capture to {stderr_dst}: {e}"))?;

    let raw = fs::read_to_string(&trace_dst)
        .map_err(|e| format!("Failed to read trace log at {trace_dst}: {e}"))?;
    let all_events = trace_parse::parse_lines(&raw);
    let total = all_events.len();

    let target_filter_lc = target_filter.map(str::to_lowercase);
    let grep_lc = grep.map(str::to_lowercase);
    let filtered: Vec<TraceEvent> = all_events
        .into_iter()
        .filter(|e| e.level >= level)
        .filter(|e| {
            target_filter_lc
                .as_deref()
                .is_none_or(|filter| e.target.to_lowercase().contains(filter))
        })
        .filter(|e| {
            grep_lc
                .as_deref()
                .is_none_or(|needle| event_searchable_text(e).to_lowercase().contains(needle))
        })
        .collect();

    let trace_dst_display = crate::debug_jp::util::relative_to(workspace_root, &trace_dst);
    let stdout_dst_display = crate::debug_jp::util::relative_to(workspace_root, &stdout_dst);
    let stderr_dst_display = crate::debug_jp::util::relative_to(workspace_root, &stderr_dst);
    let report = trace_render::render(
        &filtered,
        total,
        &launch_result,
        args,
        OutputPaths {
            trace: &trace_dst_display,
            stdout: &stdout_dst_display,
            stderr: &stderr_dst_display,
        },
    );
    fs::write(&report_dst, &report)?;
    Ok(Outcome::Success { content: report })
}

/// Build a single haystack for the `grep` filter from everything a user
/// might want to search: target, message, field keys/values, span names.
fn event_searchable_text(e: &TraceEvent) -> String {
    let mut s = String::new();
    s.push_str(&e.target);
    s.push(' ');
    s.push_str(&e.message);
    for (k, v) in &e.fields {
        s.push(' ');
        s.push_str(k);
        s.push('=');
        match v {
            Value::String(v) => s.push_str(v),
            other => s.push_str(&other.to_string()),
        }
    }
    for span in &e.spans {
        s.push(' ');
        s.push_str(span);
    }
    s
}

/// Return the last `n` lines of `text`, newline-joined.
fn tail_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[allow(dead_code, reason = "kept readable for future Error-aware refactors")]
fn _propagate_error(e: Error) -> Error {
    e
}
