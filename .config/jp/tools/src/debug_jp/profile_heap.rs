//! `profile_jp_heap` — dhat heap profile of a `jp` command.
//!
//! Builds `jp` with the `dhat` feature, runs it inside a [`Sandbox`], finds the
//! heap-profile JSON dhat writes to the sandbox's `tmp/profiling/` dir, copies
//! it back to the real workspace, then parses and renders a report.

use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

use camino::{Utf8Path, Utf8PathBuf};
use jp_tool::Outcome;

use crate::{
    Context, Error, Tool,
    debug_jp::util::{
        build::{self, BuildSpec},
        launch::{self, LaunchSpec},
        profile_heap_parse as heap_parse,
        profile_heap_render as heap_render,
        sandbox::{Sandbox, SandboxOpts},
    },
    util::{ToolResult, error},
};

/// Tool entrypoint.
/// Dispatches between the format-args preview and the live execution.
#[allow(clippy::unused_async, reason = "awaited by the debug_jp dispatcher")]
pub(crate) async fn debug_jp_profile_heap(ctx: &Context, t: &Tool) -> ToolResult {
    let args: Vec<String> = t.req("args")?;
    let clone_user_data: bool = t.opt::<bool>("clone_user_data")?.unwrap_or(true);

    if args.is_empty() {
        return error("`args` must contain at least one element (the jp subcommand).");
    }

    if ctx.action.is_format_arguments() {
        return Ok(format_preview(&args, clone_user_data).into());
    }

    run(&ctx.root, &args, clone_user_data)
}

/// Render the format-args preview shown before execution.
fn format_preview(args: &[String], clone_user_data: bool) -> String {
    let mut out = String::new();
    out.push_str("`debug_jp_profile_heap`\n\n");
    out.push_str("Will execute (under sandbox isolation):\n\n");
    out.push_str("```sh\n");
    out.push_str("cargo build --profile profiling --features dhat -p jp_cli --bin jp\n");
    out.push_str(&format!(
        "target/profiling/jp {}    # dhat-instrumented, invoked by absolute path\n",
        args.join(" ")
    ));
    out.push_str("```\n\n");
    out.push_str(
        "Heap profiling adds **significant overhead** — expect runs to be much slower\n\
         than uninstrumented or wall-clock-sampled runs. Allocator-heavy commands can take\n\
         minutes; the dhat profiler must be allowed to finish for the report to be useful.\n\n",
    );
    out.push_str(
        "The build artifact at `target/profiling/jp` may be rebuilt to reflect\n\
         the sandbox's source tree (HEAD + uncommitted changes from this worktree).\n\
         The installed `jp` binary (`~/.cargo/bin/jp` etc.) is **not** touched.\n\n",
    );

    out.push_str("Isolation:\n\n");
    out.push_str("- Workspace: detached `git worktree` under `tmp/jp-sandbox-<ts>/`\n");
    out.push_str("  - Uncommitted tracked changes applied via `git apply`\n");
    out.push_str("  - Untracked (non-ignored) files copied across\n");
    out.push_str("- User data: scratch dir under `tmp/jp-sandbox-data-<ts>/`,\n");
    out.push_str("  bound via `JP_USER_DATA_DIR`. ");
    if clone_user_data {
        out.push_str("Current user data is **cloned** into it.\n");
    } else {
        out.push_str("Sandbox starts with **empty** user data.\n");
    }
    out.push_str("- Sandbox is removed on tool exit (best-effort `Drop`).\n\n");

    out.push_str("Outputs (in the real workspace):\n\n");
    out.push_str("- `tmp/profiling/heap-<ts>.json` — raw dhat output\n");
    out.push_str("- `tmp/profiling/report-heap-<ts>.md` — rendered report\n\n");

    out.push_str("Returns the rendered Markdown report inline.\n");
    out
}

/// Live execution path.
/// Builds dhat-instrumented jp, runs it, finds the heap JSON, parses + renders.
/// Returns the Markdown report.
fn run(workspace_root: &Utf8Path, args: &[String], clone_user_data: bool) -> ToolResult {
    let sandbox = Sandbox::create(workspace_root, SandboxOpts { clone_user_data })?;

    let binary = build::build(&BuildSpec {
        working_dir: sandbox.working_dir(),
        package: "jp_cli",
        bin: "jp",
        profile: "profiling",
        features: &["dhat"],
    })?;

    let handle = launch::spawn(&LaunchSpec {
        binary,
        args: args.to_vec(),
        working_dir: sandbox.working_dir().to_owned(),
        env: sandbox.env(),
    })?;
    let launch_result = handle.wait()?;

    // dhat-rs writes `<workspace_root>/tmp/profiling/heap-<ts>.json` (the
    // workspace root is resolved at runtime via `cargo locate-project`).
    // We run inside the sandbox worktree, so that path is relative to the
    // sandbox.
    let heap_src = find_latest_heap_json(&sandbox.working_dir().join("tmp/profiling"))?;

    // Copy the heap JSON out of the sandbox so it survives sandbox cleanup.
    let ts = unix_seconds();
    let out_dir = workspace_root.join("tmp/profiling");
    fs::create_dir_all(&out_dir)?;
    let heap_dst = out_dir.join(format!("heap-{ts}.json"));
    let report_dst = out_dir.join(format!("report-heap-{ts}.md"));
    fs::copy(&heap_src, &heap_dst)
        .map_err(|e| format!("Failed to copy heap profile from {heap_src} to {heap_dst}: {e}"))?;

    let json = fs::read_to_string(&heap_dst)
        .map_err(|e| format!("Failed to read dhat output at {heap_dst}: {e}"))?;
    let profile = heap_parse::parse(&json)
        .map_err(|e| format!("Failed to parse dhat JSON at {heap_dst}: {e}"))?;
    let heap_dst_display = crate::debug_jp::util::relative_to(workspace_root, &heap_dst);
    let report = heap_render::render(&profile, &launch_result, args, &heap_dst_display);

    fs::write(&report_dst, &report)?;
    Ok(Outcome::Success { content: report })
}

/// Locate the `heap-*.json` file dhat wrote during this run.
///
/// The sandbox starts clean (no `tmp/profiling/`), so any file matching the
/// pattern was produced by the run we just executed.
/// Picks the most-recently-modified one defensively, in case dhat writes more
/// than one (it normally doesn't).
fn find_latest_heap_json(dir: &Utf8Path) -> Result<Utf8PathBuf, Error> {
    if !dir.exists() {
        return Err(format!(
            "Expected dhat to write a heap profile to {dir}, but the directory doesn't exist. \
             Was jp built with the `dhat` feature?"
        )
        .into());
    }

    let mut latest: Option<(SystemTime, Utf8PathBuf)> = None;
    for entry in fs::read_dir(dir.as_std_path())? {
        let entry = entry?;
        let name_os = entry.file_name();
        let name = name_os.to_string_lossy();
        if !name.starts_with("heap-") || !name.ends_with(".json") {
            continue;
        }
        let mtime = entry.metadata()?.modified()?;
        let path = Utf8PathBuf::from_path_buf(entry.path())
            .map_err(|p| format!("non-UTF-8 dhat output path: {}", p.display()))?;
        if latest.as_ref().is_none_or(|(prev_mtime, _)| *prev_mtime < mtime) {
            latest = Some((mtime, path));
        }
    }

    latest.map(|(_, p)| p).ok_or_else(|| {
        format!(
            "No heap-*.json file found under {dir}. The `dhat` feature may not have been \
             active, or the program exited before the profiler could write its output."
        )
        .into()
    })
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
