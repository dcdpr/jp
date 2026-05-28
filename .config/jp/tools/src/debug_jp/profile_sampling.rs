//! `profile_jp_sampling` — wall-clock profile via macOS `sample(1)`.
//!
//! Builds `jp` in the `profiling` cargo profile, launches it inside a
//! [`Sandbox`], attaches `sample(1)` keyed on its PID, waits for both, and
//! renders a Markdown report.
//!
//! When invoked with `Context::action::FormatArguments`, returns a preview of
//! exactly what will run so the user can approve against full visibility before
//! the live execution.

use std::{
    fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use camino::Utf8Path;
use jp_tool::Outcome;

use crate::{
    Context, Tool,
    debug_jp::util::{
        build::{self, BuildSpec},
        launch::{self, LaunchSpec},
        profile_sampling_parse as sample_parse,
        profile_sampling_render as sample_render,
        sandbox::{Sandbox, SandboxOpts},
    },
    util::{ToolResult, error},
};

/// Default sample(1) duration ceiling.
/// `sample` exits early when the target dies, so this only bounds runaway
/// profiling runs.
const DEFAULT_DURATION_SECS: u32 = 120;

/// Tool entrypoint.
/// Dispatches between the format-args preview and the live execution.
#[allow(clippy::unused_async, reason = "awaited by the debug_jp dispatcher")]
pub(crate) async fn debug_jp_profile_sampling(ctx: &Context, t: &Tool) -> ToolResult {
    let args: Vec<String> = t.req("args")?;
    let duration_secs: u32 = t
        .opt::<u32>("duration_secs")?
        .unwrap_or(DEFAULT_DURATION_SECS);
    let clone_user_data: bool = t.opt::<bool>("clone_user_data")?.unwrap_or(true);

    if args.is_empty() {
        return error("`args` must contain at least one element (the jp subcommand).");
    }

    if ctx.action.is_format_arguments() {
        return Ok(format_preview(&args, duration_secs, clone_user_data).into());
    }

    if !cfg!(target_os = "macos") {
        return error(
            "debug_jp_profile_sampling currently only supports macOS (uses `sample(1)`). A Linux \
             equivalent (perf / samply) is a planned follow-up.",
        );
    }

    run(&ctx.root, &args, duration_secs, clone_user_data)
}

/// Render the format-args preview shown before execution.
fn format_preview(args: &[String], duration_secs: u32, clone_user_data: bool) -> String {
    let mut out = String::new();
    out.push_str("`debug_jp_profile_sampling`\n\n");
    out.push_str("Will execute (under sandbox isolation):\n\n");
    out.push_str("```sh\n");
    out.push_str(
        "CARGO_TARGET_DIR=tmp/sandbox-target \\\n  \
         cargo build --profile profiling -p jp_cli --bin jp\n",
    );
    out.push_str(&format!(
        "tmp/sandbox-target/profiling/jp {}\n",
        args.join(" ")
    ));
    out.push_str(&format!("sample <pid> {duration_secs} 1 -file <output>\n"));
    out.push_str("```\n\n");

    out.push_str(
        "Build artifacts go to `tmp/sandbox-target/` so the main `target/` and any\n\
         `jp` binary you're running from other tabs are not disturbed. Persistent\n\
         across sandbox runs; recoverable with `rm -rf tmp/sandbox-target/`.\n\n",
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
    out.push_str("- `tmp/profiling/sample-<ts>.txt` — raw `sample(1)` output\n");
    out.push_str("- `tmp/profiling/report-sampling-<ts>.md` — rendered report\n\n");

    out.push_str("Returns the rendered Markdown report inline.\n");
    out
}

/// Live execution path.
/// Sets up the sandbox, builds jp, runs sample(1) against it, parses + renders.
/// Returns the Markdown report.
fn run(
    workspace_root: &Utf8Path,
    args: &[String],
    duration_secs: u32,
    clone_user_data: bool,
) -> ToolResult {
    // Set up an isolated workspace + user data dir. RAII guarantees cleanup
    // even on early return.
    let sandbox = Sandbox::create(workspace_root, SandboxOpts { clone_user_data })?;

    // Build jp from the sandboxed source tree. `profiling` is release-level
    // optimization plus debug info, which is exactly what `sample(1)` needs
    // to resolve symbols cleanly.
    let binary = build::build(&BuildSpec {
        working_dir: sandbox.working_dir(),
        package: "jp_cli",
        bin: "jp",
        profile: "profiling",
        features: &[],
    })?;

    // Prepare output paths in the real workspace so the artifacts survive
    // sandbox cleanup.
    let ts = unix_seconds();
    let out_dir = workspace_root.join("tmp/profiling");
    fs::create_dir_all(&out_dir)?;
    let sample_path = out_dir.join(format!("sample-{ts}.txt"));
    let report_path = out_dir.join(format!("report-sampling-{ts}.md"));

    // Launch jp.
    let handle = launch::spawn(&LaunchSpec {
        binary,
        args: args.to_vec(),
        working_dir: sandbox.working_dir().to_owned(),
        env: sandbox.env(),
    })?;

    // Attach sample(1) immediately. `sample <pid> <dur> <interval_ms> -file
    // <path>` blocks until the process dies or the duration elapses.
    let sampler_child = Command::new("sample")
        .args([
            &handle.pid().to_string(),
            &duration_secs.to_string(),
            "1",
            "-file",
            sample_path.as_str(),
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn `sample`: {e}"))?;

    // Wait for jp first; sample will exit on its own when jp dies. If
    // sample lingers (e.g. duration not yet hit but jp is gone), wait_with_output
    // returns once it actually exits.
    let launch_result = handle.wait()?;
    let sampler_output = sampler_child
        .wait_with_output()
        .map_err(|e| format!("Failed to wait on `sample`: {e}"))?;

    if !sampler_output.status.success() {
        let stderr = String::from_utf8_lossy(&sampler_output.stderr);
        return error(format!(
            "`sample(1)` failed: {stderr}\nNote: macOS `sample(1)` may require granting Terminal \
             `Developer Tools` permission in System Settings → Privacy & Security.",
        ));
    }

    // Parse + render.
    let raw = fs::read_to_string(&sample_path)
        .map_err(|e| format!("Failed to read sample output at {sample_path}: {e}"))?;
    let threads = sample_parse::parse(&raw);
    let sample_path_display = crate::debug_jp::util::relative_to(workspace_root, &sample_path);
    let report = sample_render::render(&threads, &launch_result, args, &sample_path_display);

    fs::write(&report_path, &report)?;

    Ok(Outcome::Success { content: report })
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
