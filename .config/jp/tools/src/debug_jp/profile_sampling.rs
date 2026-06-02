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
    process::{Child, Command, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use camino::Utf8Path;
use jp_tool::Outcome;

use crate::{
    Context, Tool,
    debug_jp::util::{
        build::{self, BuildSpec},
        launch::{LaunchSpec, Launcher, RealLauncher, Timeouts},
        profile_sampling_parse as sample_parse, profile_sampling_render as sample_render,
        sandbox::{Sandbox, SandboxOpts},
        with_termination_note,
    },
    util::{ToolResult, error, runner::DuctProcessRunner},
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
        "CARGO_TARGET_DIR=tmp/sandbox-target \\\n  cargo build --profile profiling -p jp_cli \
         --bin jp\n",
    );
    out.push_str(&format!(
        "tmp/sandbox-target/profiling/jp {}\n",
        args.join(" ")
    ));
    out.push_str(&format!("sample <pid> {duration_secs} 1 -file <output>\n"));
    out.push_str("```\n\n");

    out.push_str(
        "Build artifacts go to `tmp/sandbox-target/` so the main `target/` and any\n`jp` binary \
         you're running from other tabs are not disturbed. Persistent\nacross sandbox runs; \
         recoverable with `rm -rf tmp/sandbox-target/`.\n\n",
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

/// Live execution path: set up the sandbox, build jp, then [`execute`].
fn run(
    workspace_root: &Utf8Path,
    args: &[String],
    duration_secs: u32,
    clone_user_data: bool,
) -> ToolResult {
    // Set up an isolated workspace + user data dir. RAII guarantees cleanup
    // even on early return.
    let sandbox = Sandbox::create(
        workspace_root,
        SandboxOpts { clone_user_data },
        &DuctProcessRunner,
    )?;

    // Build jp from the sandboxed source tree. `profiling` is release-level
    // optimization plus debug info, which is exactly what `sample(1)` needs
    // to resolve symbols cleanly.
    let binary = build::build(&DuctProcessRunner, &BuildSpec {
        working_dir: sandbox.working_dir(),
        package: "jp_cli",
        bin: "jp",
        profile: "profiling",
        features: &[],
    })?;

    let spec = LaunchSpec {
        binary,
        args: args.to_vec(),
        working_dir: sandbox.working_dir().to_owned(),
        env: sandbox.env(),
    };

    // Let jp run slightly past the sample window before the timeout intervenes,
    // so the explicit `duration_secs` governs the run length rather than the
    // flat default budget.
    let timeouts = Timeouts::with_run(Duration::from_secs(u64::from(duration_secs) + 10));
    execute(
        workspace_root,
        &spec,
        duration_secs,
        &RealLauncher,
        timeouts,
    )
}

/// Launch jp via `launcher` with `sample(1)` attached to its PID, then parse +
/// render.
///
/// Split from [`run`] so the launch boundary is injectable, independent of the
/// sandbox and build steps.
fn execute(
    workspace_root: &Utf8Path,
    spec: &LaunchSpec,
    duration_secs: u32,
    launcher: &dyn Launcher,
    timeouts: Timeouts,
) -> ToolResult {
    // Prepare output paths in the real workspace so the artifacts survive
    // sandbox cleanup.
    let ts = unix_seconds();
    let out_dir = workspace_root.join("tmp/profiling");
    fs::create_dir_all(&out_dir)?;
    let sample_path = out_dir.join(format!("sample-{ts}.txt"));
    let report_path = out_dir.join(format!("report-sampling-{ts}.md"));

    // Attach `sample(1)` to jp the moment it spawns, before the supervised
    // wait. `sample <pid> <dur> <interval_ms> -file <path>` blocks until the
    // target dies or the duration elapses. Errors are stashed because the
    // callback can't return them.
    let mut sampler: Option<Child> = None;
    let mut sampler_spawn_err: Option<String> = None;
    let launch_result = launcher.run(spec, timeouts, &mut |pid| match Command::new("sample")
        .args([
            &pid.to_string(),
            &duration_secs.to_string(),
            "1",
            "-file",
            sample_path.as_str(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => sampler = Some(child),
        Err(e) => sampler_spawn_err = Some(format!("Failed to spawn `sample`: {e}")),
    })?;

    if let Some(err) = sampler_spawn_err {
        return Err(err.into());
    }
    let sampler_output = sampler
        .expect("sampler is set when no spawn error was recorded")
        .wait_with_output()
        .map_err(|e| format!("Failed to wait on `sample`: {e}"))?;

    if !sampler_output.status.success() {
        let stderr = String::from_utf8_lossy(&sampler_output.stderr);
        return error(format!(
            "`sample(1)` failed: {stderr}\nNote: macOS `sample(1)` may require granting Terminal \
             `Developer Tools` permission in System Settings → Privacy & Security.",
        ));
    }

    let raw = fs::read_to_string(&sample_path)
        .map_err(|e| format!("Failed to read sample output at {sample_path}: {e}"))?;
    let threads = sample_parse::parse(&raw);
    let sample_path_display = crate::debug_jp::util::relative_to(workspace_root, &sample_path);
    let report = sample_render::render(&threads, &launch_result, &spec.args, &sample_path_display);
    let report = with_termination_note(report, &launch_result);

    fs::write(&report_path, &report)?;

    Ok(Outcome::Success { content: report })
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
