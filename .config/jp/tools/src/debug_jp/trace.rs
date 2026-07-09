//! `debug_jp_trace` — capture and render `JP_DEBUG=1` trace logs.
//!
//! Launches `jp` inside the sandbox with `JP_DEBUG=1` set.
//! `jp_cli` persists its tracing-subscriber output to a system temp file and
//! announces the path on stderr at exit, either as a `Full trace log written
//! to: <path>` line (text output) or a `{"trace_log": "<path>"}` object (JSON
//! output).
//! We parse that line, copy the file out to the real workspace so it survives
//! sandbox cleanup, filter the events by level/target/grep, and render the
//! result in a compact logfmt-like format.

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
        launch::{LaunchResult, LaunchSpec, Launcher, RealLauncher, Timeouts},
        sandbox::{Sandbox, SandboxOpts},
        trace_parse::{self, Level, TRACE_PATH_PREFIX, TraceEvent},
        trace_render::{self, CommandRun, OutputPaths},
        with_termination_note,
    },
    util::{ToolResult, error, runner::DuctProcessRunner},
};

/// Tool entrypoint.
/// Dispatches between the format-args preview and the live execution.
#[allow(clippy::unused_async, reason = "awaited by the debug_jp dispatcher")]
pub(crate) async fn debug_jp_trace(ctx: &Context, t: &Tool) -> ToolResult {
    let args: Vec<String> = t.req("args")?;
    let then: Vec<Vec<String>> = t.opt("then")?.unwrap_or_default();
    let level_str: Option<String> = t.opt("level")?;
    let target_filter: Option<String> = t.opt("target")?;
    let grep: Option<String> = t.opt("grep")?;
    let clone_user_data: bool = t.opt::<bool>("clone_user_data")?.unwrap_or(true);

    if args.is_empty() {
        return error("`args` must contain at least one element (the jp subcommand).");
    }
    for (i, command) in then.iter().enumerate() {
        if command.is_empty() {
            return error(format!(
                "`then[{i}]` must contain at least one element (the jp subcommand)."
            ));
        }
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

    // The first command is `args`; any `then` entries run after it in the same
    // sandbox, sharing state.
    let mut commands = Vec::with_capacity(1 + then.len());
    commands.push(args);
    commands.extend(then);

    if ctx.action.is_format_arguments() {
        return Ok(format_preview(
            &commands,
            level,
            target_filter.as_deref(),
            grep.as_deref(),
            clone_user_data,
        )
        .into());
    }

    run(
        &ctx.root,
        &commands,
        level,
        target_filter.as_deref(),
        grep.as_deref(),
        clone_user_data,
    )
}

/// Render the format-args preview shown before execution.
fn format_preview(
    commands: &[Vec<String>],
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
    if let [command] = commands {
        out.push_str(&format!(
            "JP_DEBUG=1 target/profiling/jp {}    # invoked by absolute path\n",
            command.join(" ")
        ));
    } else {
        out.push_str("# commands run in sequence in one sandbox (state persists between them):\n");
        for (i, command) in commands.iter().enumerate() {
            out.push_str(&format!(
                "JP_DEBUG=1 target/profiling/jp {}    # command {}\n",
                command.join(" "),
                i + 1
            ));
        }
    }
    out.push_str("```\n\n");
    out.push_str(
        "The build artifact at `target/profiling/jp` may be rebuilt to reflect\nthe sandbox's \
         source tree (HEAD + uncommitted changes from this worktree).\nThe installed `jp` binary \
         (`~/.cargo/bin/jp` etc.) is **not** touched.\n\n",
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

/// Live execution path: set up the sandbox, build jp once, then run every
/// command in it.
///
/// The sandbox and build are shared across all commands, and the commands run
/// in sequence against the same `JP_USER_DATA_DIR`, so state created by one
/// command is visible to the next.
fn run(
    workspace_root: &Utf8Path,
    commands: &[Vec<String>],
    level: Level,
    target_filter: Option<&str>,
    grep: Option<&str>,
    clone_user_data: bool,
) -> ToolResult {
    let sandbox = Sandbox::create(
        workspace_root,
        SandboxOpts { clone_user_data },
        &DuctProcessRunner,
    )?;

    let binary = build::build(&DuctProcessRunner, &BuildSpec {
        working_dir: sandbox.working_dir(),
        package: "jp_cli",
        bin: "jp",
        profile: "profiling",
        features: &[],
    })?;

    let mut env = sandbox.env();
    env.push(("JP_DEBUG".to_owned(), "1".to_owned()));

    let specs: Vec<LaunchSpec> = commands
        .iter()
        .map(|args| LaunchSpec {
            binary: binary.clone(),
            args: args.clone(),
            working_dir: sandbox.working_dir().to_owned(),
            env: env.clone(),
        })
        .collect();

    if let [spec] = specs.as_slice() {
        execute(
            workspace_root,
            spec,
            level,
            target_filter,
            grep,
            &RealLauncher,
            Timeouts::DEFAULT,
        )
    } else {
        execute_sequence(
            workspace_root,
            &specs,
            level,
            target_filter,
            grep,
            &RealLauncher,
            Timeouts::DEFAULT,
        )
    }
}

/// Run a single command and render it as a standalone report.
///
/// Split from [`run`] so it can be driven with a fake [`Launcher`] in tests,
/// independent of the sandbox and build steps.
fn execute(
    workspace_root: &Utf8Path,
    spec: &LaunchSpec,
    level: Level,
    target_filter: Option<&str>,
    grep: Option<&str>,
    launcher: &dyn Launcher,
    timeouts: Timeouts,
) -> ToolResult {
    let ts = unix_seconds();
    let out_dir = workspace_root.join("tmp/profiling");
    fs::create_dir_all(&out_dir)?;

    let art = run_one(
        &out_dir,
        spec,
        level,
        target_filter,
        grep,
        launcher,
        timeouts,
        ts,
        "",
    )?;

    let trace_display = crate::debug_jp::util::relative_to(workspace_root, &art.trace_dst);
    let stdout_display = crate::debug_jp::util::relative_to(workspace_root, &art.stdout_dst);
    let stderr_display = crate::debug_jp::util::relative_to(workspace_root, &art.stderr_dst);
    let report = trace_render::render(
        &art.events,
        art.total,
        &art.launch,
        &spec.args,
        OutputPaths {
            trace: &trace_display,
            stdout: &stdout_display,
            stderr: &stderr_display,
        },
    );
    let report = with_termination_note(report, &art.launch);
    fs::write(out_dir.join(format!("report-trace-{ts}.md")), &report)?;
    Ok(Outcome::Success { content: report })
}

/// Run a sequence of commands in order and render one combined report.
///
/// Fail-fast: if a command never flushes its trace (e.g. force-killed on
/// timeout), the sequence stops and the error names which command failed.
/// Artifacts for the commands that did run are already on disk.
fn execute_sequence(
    workspace_root: &Utf8Path,
    specs: &[LaunchSpec],
    level: Level,
    target_filter: Option<&str>,
    grep: Option<&str>,
    launcher: &dyn Launcher,
    timeouts: Timeouts,
) -> ToolResult {
    let ts = unix_seconds();
    let out_dir = workspace_root.join("tmp/profiling");
    fs::create_dir_all(&out_dir)?;

    let mut artifacts = Vec::with_capacity(specs.len());
    for (i, spec) in specs.iter().enumerate() {
        let label = format!("-cmd{}", i + 1);
        let art = run_one(
            &out_dir,
            spec,
            level,
            target_filter,
            grep,
            launcher,
            timeouts,
            ts,
            &label,
        )
        .map_err(|e| {
            format!(
                "Command {} (`jp {}`) failed, stopping the sequence: {e}\n\nArtifacts for any \
                 earlier commands remain under tmp/profiling.",
                i + 1,
                spec.args.join(" ")
            )
        })?;
        artifacts.push(art);
    }

    // Hold the display strings alive for the borrows in `CommandRun`.
    let displays: Vec<(String, String, String)> = artifacts
        .iter()
        .map(|a| {
            (
                crate::debug_jp::util::relative_to(workspace_root, &a.trace_dst),
                crate::debug_jp::util::relative_to(workspace_root, &a.stdout_dst),
                crate::debug_jp::util::relative_to(workspace_root, &a.stderr_dst),
            )
        })
        .collect();

    let runs: Vec<CommandRun<'_>> = specs
        .iter()
        .zip(&artifacts)
        .zip(&displays)
        .map(|((spec, art), display)| CommandRun {
            args: &spec.args,
            events: &art.events,
            total: art.total,
            launch: &art.launch,
            paths: OutputPaths {
                trace: &display.0,
                stdout: &display.1,
                stderr: &display.2,
            },
        })
        .collect();

    let report = trace_render::render_multi(&runs);
    fs::write(out_dir.join(format!("report-trace-{ts}.md")), &report)?;
    Ok(Outcome::Success { content: report })
}

/// One command's launch result plus its extracted, filtered trace events and
/// the workspace-local paths its artifacts were copied to.
struct CommandArtifacts {
    events: Vec<TraceEvent>,
    total: usize,
    launch: LaunchResult,
    trace_dst: Utf8PathBuf,
    stdout_dst: Utf8PathBuf,
    stderr_dst: Utf8PathBuf,
}

/// Launch jp via `launcher`, copy its trace log and streams into `out_dir`
/// (keyed by `<ts><label>`), and return the filtered events.
///
/// `label` distinguishes a command's files within a sequence (e.g. `-cmd1`); it
/// is empty for a single-command run, preserving the standalone file names.
/// Returns an error when jp exits without flushing its trace log.
#[allow(
    clippy::too_many_arguments,
    reason = "thin internal seam over the launch+copy step"
)]
fn run_one(
    out_dir: &Utf8Path,
    spec: &LaunchSpec,
    level: Level,
    target_filter: Option<&str>,
    grep: Option<&str>,
    launcher: &dyn Launcher,
    timeouts: Timeouts,
    ts: u64,
    label: &str,
) -> Result<CommandArtifacts, Error> {
    let launch_result = launcher.run(spec, timeouts, &mut |_| {})?;

    // jp announces the trace path on stderr right before exit, either as a
    // text marker line or a `trace_log` JSON field (depending on
    // `--format`). The path is in the system temp dir (e.g.
    // `/var/folders/...`), outside the sandbox. A force-killed jp never
    // flushes, so fold the termination note into the error when the marker
    // is absent.
    let Some(trace_path) = trace_parse::extract_trace_path(&launch_result.stderr) else {
        let note = launch_result
            .note()
            .map(|n| format!("{n}\n\n"))
            .unwrap_or_default();
        return Err(format!(
            "{note}Did not find a `{TRACE_PATH_PREFIX}<path>` line or a `trace_log` JSON field in \
             jp's stderr. jp may have exited before the tracing layer flushed, or stderr was \
             redirected. Last 20 lines of stderr:\n{}",
            tail_lines(&launch_result.stderr, 20)
        )
        .into());
    };
    let trace_src = Utf8PathBuf::from(trace_path);

    // Copy the trace log into the real workspace so it survives the system
    // temp dir's eventual cleanup and stays alongside other profile output.
    // Stdout/stderr are dumped alongside so each command's trace + streams are
    // one cohesive set of files keyed by `<ts><label>`.
    let trace_dst = out_dir.join(format!("trace-{ts}{label}.jsonl"));
    let stdout_dst = out_dir.join(format!("trace-{ts}{label}-stdout.txt"));
    let stderr_dst = out_dir.join(format!("trace-{ts}{label}-stderr.txt"));
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
    let events = filter_events(all_events, level, target_filter, grep);

    Ok(CommandArtifacts {
        events,
        total,
        launch: launch_result,
        trace_dst,
        stdout_dst,
        stderr_dst,
    })
}

/// Apply the level/target/grep filters to a command's parsed events.
fn filter_events(
    events: Vec<TraceEvent>,
    level: Level,
    target_filter: Option<&str>,
    grep: Option<&str>,
) -> Vec<TraceEvent> {
    let target_filter_lc = target_filter.map(str::to_lowercase);
    let grep_lc = grep.map(str::to_lowercase);
    events
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
        .collect()
}

/// Build a single haystack for the `grep` filter from everything a user might
/// want to search: target, message, field keys/values, span names.
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

#[cfg(test)]
#[path = "trace_tests.rs"]
mod tests;
