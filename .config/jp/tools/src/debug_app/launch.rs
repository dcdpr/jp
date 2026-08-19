//! `debug_app_launch` — build the macOS app and start a driveable instance.
//!
//! Launches through `LaunchServices` (`open -n`) rather than by executing the
//! binary inside the bundle, because some `AppKit` behaviour depends on the app
//! being registered normally.
//! `open` also injects the environment and redirects both output streams to
//! files, so the console stays readable without a pipe this process has to keep
//! draining.
//!
//! Isolation is by environment, all of it verified rather than assumed:
//!
//! - `JP_DEBUG_STATE_DIR` moves the app's recent-workspace list into a file
//!   under `tmp/debug-app/state/`, and is how the app reports its own pid.
//!   Without it the app writes the recents list it shares with the system,
//!   which no harness can read back or restore.
//! - `JP_USER_DATA_DIR` moves the user-local conversation store.
//! - `JP_WORKSPACE` names the workspace to open.
//!   It is consulted only when the recents list is empty, which a fresh state
//!   directory guarantees.
//!
//! Window state saved by `@SceneStorage` is keyed by bundle identifier rather
//! than by environment, so the bundle that runs is a copy carrying this slot's
//! own identifier.
//! That is what keeps a driven run out of the developer's own window state, and
//! two agents out of each other's.

use std::{
    fs, thread,
    time::{Duration, Instant},
};

use camino::{Utf8Path, Utf8PathBuf};
use jp_tool::Outcome;
use serde::Deserialize;

use crate::{
    Context, Error, Tool,
    debug_app::{
        capture,
        session::{Console, Session, Slot, state_dir, trace_path},
    },
    util::{
        ToolResult, error,
        paths::{self, Shortening, shorten},
        runner::{DuctProcessRunner, ProcessRunner},
    },
};

/// Xcode configuration built when the caller names none.
const DEFAULT_CONFIGURATION: &str = "Debug";

/// How long to wait for the app to report its pid before giving up.
///
/// Generous: this covers `LaunchServices` starting the process, `AppKit`
/// finishing its own setup, and the app reaching its `init`.
const PID_TIMEOUT: Duration = Duration::from_secs(30);

/// Poll interval while waiting for the pid file.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The entitlements a staged bundle is signed with.
///
/// `get-task-allow` is what lets a profiler attach, and the staged copy would
/// otherwise have none: the build's ad-hoc signature carries it, and re-signing
/// after the identifier rewrite replaces that signature wholesale.
///
/// Only this one, because the app declares no entitlements file and is not
/// sandboxed, so this is the whole of what a Debug build gets.
/// Applied whatever the configuration, because a staged bundle exists to be
/// driven and profiled and never leaves `tmp/`.
const ENTITLEMENTS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>com.apple.security.get-task-allow</key>
	<true/>
</dict>
</plist>
"#;

/// The workspace ID written into a scratch workspace.
///
/// Fixed rather than random: the scratch workspace is reused across runs, and a
/// stable ID means a stable user-local store rather than a new one each launch.
const SCRATCH_WORKSPACE_ID: &str = "probe";

/// Which appearance the app is told to draw in.
///
/// Given as an argument rather than an environment variable because that is the
/// only lever there is: `AppKit` reads `AppleInterfaceStyle` through
/// `NSUserDefaults`, whose argument domain outranks every other, and a launch
/// argument is how a caller writes to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Appearance {
    Light,
    Dark,
}

impl Appearance {
    /// What `AppleInterfaceStyle` is set to.
    ///
    /// `AppKit` tests this against `Dark` and treats anything else as light, so
    /// naming the light case explicitly forces light on a machine set to dark
    /// rather than merely leaving the choice alone.
    const fn style(self) -> &'static str {
        match self {
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    fn parse(value: &str) -> Result<Self, Error> {
        match value {
            "light" => Ok(Self::Light),
            "dark" => Ok(Self::Dark),
            other => Err(format!(
                "`appearance` takes `light` or `dark`, not `{other}`. Leave it out to follow the \
                 system."
            )
            .into()),
        }
    }
}

/// What to launch, and with what environment.
#[derive(Debug, Clone)]
pub(crate) struct LaunchSpec {
    pub bundle: Utf8PathBuf,
    pub workspace: Utf8PathBuf,
    pub state_dir: Utf8PathBuf,
    pub user_data_dir: Utf8PathBuf,
    pub stdout: Utf8PathBuf,
    pub stderr: Utf8PathBuf,

    /// Where the app writes its own trace, inside the state directory.
    pub trace: Utf8PathBuf,

    /// Whether the app is told to keep a stack for every allocation.
    ///
    /// Decided here because libmalloc reads `MallocStackLogging` at process
    /// start, so an app that was not launched for it can never report
    /// allocation stacks, whatever a profile bracket later asks for.
    pub allocation_stacks: bool,

    /// Which appearance to draw in, or `None` to follow the system.
    pub appearance: Option<Appearance>,

    /// Whether to make the app ignore the window state `AppKit` saved for it.
    ///
    /// Window frames and `@SceneStorage` are kept by `AppKit` under the bundle
    /// identifier, in the user's home directory, which no environment variable
    /// moves and no state directory holds.
    /// Without this a run restores whatever the last one left, including a
    /// frame that cannot be recovered from: a window restored to no size is
    /// absent from the window server's list entirely, so it can be neither read
    /// nor captured nor resized.
    pub ignore_saved_windows: bool,
}

/// Tool entrypoint.
#[allow(clippy::unused_async, reason = "awaited by the debug_app dispatcher")]
pub(crate) async fn debug_app_launch(ctx: &Context, t: &Tool) -> ToolResult {
    let workspace: Option<String> = t.opt("workspace")?;
    let configuration = t
        .opt::<String>("configuration")?
        .unwrap_or_else(|| DEFAULT_CONFIGURATION.to_owned());
    let fresh = t.opt::<bool>("fresh")?.unwrap_or(true);
    let allocation_stacks = t.opt::<bool>("allocation_stacks")?.unwrap_or(false);
    let appearance = t
        .opt::<String>("appearance")?
        .map(|value| Appearance::parse(&value))
        .transpose()?;

    if ctx.action.is_format_arguments() {
        return Ok(format_preview(
            workspace.as_deref(),
            &configuration,
            fresh,
            allocation_stacks,
            appearance,
        )
        .into());
    }

    if !cfg!(target_os = "macos") {
        return error(
            "debug_app_launch only supports macOS: it drives an AppKit application through \
             LaunchServices.",
        );
    }

    let slot = Slot::for_context(ctx);
    run(
        &ctx.root,
        &slot,
        workspace.as_deref(),
        &configuration,
        fresh,
        allocation_stacks,
        appearance,
        &DuctProcessRunner,
    )
}

/// Render the preview shown before execution.
fn format_preview(
    workspace: Option<&str>,
    configuration: &str,
    fresh: bool,
    allocation_stacks: bool,
    appearance: Option<Appearance>,
) -> String {
    let malloc = if allocation_stacks {
        "\nAlso passes `--env MallocStackLogging=1`, so the app keeps a stack for \
         every\nallocation and `debug_app_profile` can record allocations against it. That costs \
         2x\nto 10x and not evenly, so every timing in the session becomes comparable only \
         with\nanother such session.\n"
    } else {
        ""
    };

    let style = appearance.map_or_else(String::new, |appearance| {
        format!(
            "\nAlso passes `--args -AppleInterfaceStyle {}`, so the app draws in {} appearance \
             whatever\nthe machine is set to.\n",
            appearance.style(),
            appearance.style().to_lowercase()
        )
    });

    let workspace = workspace.unwrap_or("tmp/debug-app/workspace (scratch, created if missing)");
    let state = if fresh {
        "- State directory is emptied first, so the app opens the workspace named above.\n"
    } else {
        "- State directory is kept, so the app restores whatever it had open last and\n  ignores \
         the workspace named above.\n"
    };

    format!(
        "`debug_app_launch`\n\nWill execute:\n\n```sh\njust build-app {configuration}\nopen -n -g \
         -a <bundle> \\\n  --env JP_DEBUG_STATE_DIR=tmp/debug-app/state \\\n  --env \
         JP_USER_DATA_DIR=tmp/debug-app/data \\\n  --env JP_WORKSPACE={workspace} \\\n  --stdout \
         tmp/debug-app/console.out \\\n  --stderr tmp/debug-app/console.err\n```\n\nLeaves a \
         running GUI application behind, addressed by later `debug_app_*` calls and\nstopped with \
         `debug_app_quit`.\n\nIsolation:\n\n- Recent-workspace list: a file under \
         `tmp/debug-app/<slot>/state/`, not the list the app\n  shares with the system.\n- \
         Conversation store: `tmp/debug-app/<slot>/data/`.\n- Window state (`@SceneStorage`): a \
         bundle copy carrying this slot's own\n  identifier.\n{state}\nRecords \
         `tmp/debug-app/session.json` and returns whatever the app wrote to its\nconsole while \
         starting up.\n{malloc}{style}"
    )
}

/// Build, launch, and record the session.
#[allow(clippy::too_many_arguments, reason = "a launch has this many knobs")]
fn run(
    root: &Utf8Path,
    slot: &Slot,
    workspace: Option<&str>,
    configuration: &str,
    fresh: bool,
    allocation_stacks: bool,
    appearance: Option<Appearance>,
    runner: &dyn ProcessRunner,
) -> ToolResult {
    let dir = Session::dir(root, slot);

    // A second instance would take the pid file and the state directory from the
    // first, leaving neither addressable. Refuse rather than pick one.
    if let Some(existing) = Session::load(&dir)?
        && existing.is_running()
    {
        return error(format!(
            "An app session is already running as pid {}, launched against {}. Stop it with \
             `debug_app_quit` before launching another.",
            existing.pid,
            shorten(existing.workspace.as_str(), &paths::shortenings(root))
        ));
    }

    let state_dir = state_dir(&dir);
    let user_data_dir = dir.join("data");

    // Before the state directory is prepared, which with `fresh` removes it
    // wholesale and would take the previous run's stream with it.
    let archived = capture::archive_stream(&dir, &trace_path(&state_dir))?;

    prepare_state_dir(&state_dir, fresh)?;
    fs::create_dir_all(&user_data_dir)?;

    let workspace = if let Some(path) = workspace {
        resolve_workspace(root, path)?
    } else {
        let scratch = dir.join("workspace");
        create_scratch_workspace(&scratch)?;
        scratch
    };

    let build = runner
        .run("just", &["build-app", configuration], root)
        .map_err(|e| format!("Failed to spawn `just build-app`: {e}"))?;
    if !build.success() {
        return error(format!(
            "`just build-app {configuration}` failed:\n\n```\n{}\n```",
            build.stderr.trim_end()
        ));
    }

    let built = locate_bundle(root, configuration, runner)?;
    let bundle = stage_bundle(&built, &dir, slot, root, runner)?;

    let trace = trace_path(&state_dir);
    let spec = LaunchSpec {
        bundle,
        workspace,
        state_dir,
        user_data_dir,
        stdout: dir.join("console.out"),
        stderr: dir.join("console.err"),
        trace,
        allocation_stacks,
        appearance,
        // A fresh run is fresh in every respect it can be, and the window frame is
        // one of them. Kept when `fresh` is false, because observing what the app
        // restores is the whole point of that.
        ignore_saved_windows: fresh,
    };

    // Truncate before launching so the first delta holds this run's output and
    // not the previous run's. The trace stream was archived rather than
    // truncated, so what is created here is an empty file for this run alone.
    fs::write(&spec.stdout, "")?;
    fs::write(&spec.stderr, "")?;
    fs::write(&spec.trace, "")?;

    let args = open_args(&spec);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let launch = runner
        .run("open", &arg_refs, root)
        .map_err(|e| format!("Failed to spawn `open`: {e}"))?;
    if !launch.success() {
        return error(format!(
            "`open` refused to launch {}:\n\n```\n{}\n```",
            spec.bundle,
            launch.stderr.trim_end()
        ));
    }

    let pid = wait_for_pid(&spec.state_dir, PID_TIMEOUT)?;

    let mut session = Session {
        pid,
        bundle: spec.bundle,
        configuration: configuration.to_owned(),
        workspace: spec.workspace,
        state_dir: spec.state_dir,
        user_data_dir: spec.user_data_dir,
        stdout: Console::new(spec.stdout),
        stderr: Console::new(spec.stderr),
        trace: Console::new(spec.trace),
        reported_footprint_mb: None,
        dsym: locate_dsym(&built),
        allocation_stacks,
    };

    let out = session.stdout.delta()?;
    let err = session.stderr.delta()?;
    session.store(&dir)?;

    Ok(Outcome::Success {
        content: report(
            &paths::shortenings(root),
            &dir,
            &session,
            &out,
            &err,
            archived.as_deref(),
        ),
    })
}

/// The dSYM matching `built`, when the build produced one.
///
/// Xcode writes it beside the bundle under the bundle's own name.
/// A configuration built without `dwarf-with-dsym` has none, and symbolication
/// then falls back to whatever the binary itself carries.
fn locate_dsym(built: &Utf8Path) -> Option<Utf8PathBuf> {
    let name = built.file_name()?;
    let stem = built.file_stem()?.to_owned();
    let path = built
        .with_file_name(format!("{name}.dSYM"))
        .join("Contents/Resources/DWARF")
        .join(stem);

    path.is_file().then_some(path)
}

/// The `open(1)` command line for `spec`.
///
/// `-g` keeps the app off the foreground.
/// A driven launch that stole keyboard focus would interrupt whatever the
/// caller was typing, and reading an accessibility tree does not need the app
/// frontmost.
///
/// `MallocStackLogging` rides along when a profile bracket recording
/// allocations is already open, because libmalloc reads it at process start and
/// there is no later moment at which it can be switched on.
fn open_args(spec: &LaunchSpec) -> Vec<String> {
    let mut args = vec![
        "-n".to_owned(),
        "-g".to_owned(),
        "-a".to_owned(),
        spec.bundle.to_string(),
        "--env".to_owned(),
        format!("JP_DEBUG_STATE_DIR={}", spec.state_dir),
        "--env".to_owned(),
        format!("JP_USER_DATA_DIR={}", spec.user_data_dir),
        "--env".to_owned(),
        format!("JP_WORKSPACE={}", spec.workspace),
    ];

    if spec.allocation_stacks {
        args.push("--env".to_owned());
        args.push("MallocStackLogging=1".to_owned());
    }

    args.extend([
        "--stdout".to_owned(),
        spec.stdout.to_string(),
        "--stderr".to_owned(),
        spec.stderr.to_string(),
    ]);

    // Last, and last for a reason: everything after `--args` is handed to the app
    // rather than read by `open`. One `--args`, because a second would be passed
    // through as an argument rather than starting a new list.
    let mut app_args: Vec<String> = Vec::new();

    if spec.ignore_saved_windows {
        app_args.extend(["-ApplePersistenceIgnoreState".to_owned(), "YES".to_owned()]);
    }

    if let Some(appearance) = spec.appearance {
        app_args.extend([
            "-AppleInterfaceStyle".to_owned(),
            appearance.style().to_owned(),
        ]);
    }

    if !app_args.is_empty() {
        args.push("--args".to_owned());
        args.extend(app_args);
    }

    args
}

/// Ask Xcode where it put the bundle.
///
/// The derived data directory is keyed by a hash of the project path, so there
/// is no path to hardcode.
fn locate_bundle(
    root: &Utf8Path,
    configuration: &str,
    runner: &dyn ProcessRunner,
) -> Result<Utf8PathBuf, Error> {
    let output = runner
        .run(
            "xcodebuild",
            &[
                "-project",
                "apps/macos/JP.xcodeproj",
                "-scheme",
                "JP",
                "-configuration",
                configuration,
                "-showBuildSettings",
                "-json",
            ],
            root,
        )
        .map_err(|e| format!("Failed to spawn `xcodebuild`: {e}"))?;

    if !output.success() {
        return Err(format!(
            "`xcodebuild -showBuildSettings` failed: {}",
            output.stderr.trim_end()
        )
        .into());
    }

    let bundle = bundle_path(&output.stdout)?;
    if !bundle.is_dir() {
        return Err(format!(
            "Xcode reports the app at {bundle}, but nothing is there. Try `just build-app \
             {configuration}` by hand."
        )
        .into());
    }

    Ok(bundle)
}

/// The bundle identifier a driven instance runs under.
///
/// Derived from the slot, because everything macOS keys by bundle identifier is
/// shared by every process using it: the recent-workspace list, and the window
/// state `@SceneStorage` writes.
/// An environment variable reaches neither, so the identifier is the only
/// lever, and without it two agents would restore each other's windows.
fn bundle_identifier(slot: &str) -> String {
    format!("computer.jp.jean-pierre.drive.{slot}")
}

/// Copy the built bundle and give it this slot's identifier.
///
/// Copied on every launch rather than kept: the copy is the thing that runs, so
/// a stale one would silently drive the previous build.
///
/// Re-signing is not optional.
/// Rewriting `Info.plist` invalidates the signature the build produced, and
/// macOS refuses to launch a bundle whose signature does not match its
/// contents.
/// It is also what makes [`ENTITLEMENTS`] necessary: the new signature carries
/// only what it is given.
fn stage_bundle(
    built: &Utf8Path,
    dir: &Utf8Path,
    slot: &Slot,
    root: &Utf8Path,
    runner: &dyn ProcessRunner,
) -> Result<Utf8PathBuf, Error> {
    let staged = dir.join("JP.app");
    if staged.exists() {
        fs::remove_dir_all(&staged).map_err(|e| format!("Failed to clear {staged}: {e}"))?;
    }

    run_step(runner, root, "cp", &["-R", built.as_str(), staged.as_str()])?;

    let identifier = bundle_identifier(slot.as_str());
    let plist = staged.join("Contents/Info.plist");
    run_step(runner, root, "plutil", &[
        "-replace",
        "CFBundleIdentifier",
        "-string",
        &identifier,
        plist.as_str(),
    ])?;

    let entitlements = dir.join("entitlements.plist");
    fs::write(&entitlements, ENTITLEMENTS)
        .map_err(|e| format!("Failed to write {entitlements}: {e}"))?;

    run_step(runner, root, "codesign", &[
        "--force",
        "--sign",
        "-",
        "--entitlements",
        entitlements.as_str(),
        staged.as_str(),
    ])?;

    Ok(staged)
}

/// Run one staging command, failing with what it said.
fn run_step(
    runner: &dyn ProcessRunner,
    root: &Utf8Path,
    program: &str,
    args: &[&str],
) -> Result<(), Error> {
    let output = runner
        .run(program, args, root)
        .map_err(|e| format!("Failed to spawn `{program}`: {e}"))?;

    if !output.success() {
        return Err(format!(
            "`{program} {}` failed: {}",
            args.join(" "),
            output.stderr.trim_end()
        )
        .into());
    }

    Ok(())
}

/// One target's build settings, as `xcodebuild -showBuildSettings -json`
/// reports them.
#[derive(Debug, Deserialize)]
struct TargetSettings {
    target: String,
    #[serde(rename = "buildSettings")]
    settings: BuildSettings,
}

#[derive(Debug, Deserialize)]
struct BuildSettings {
    #[serde(rename = "BUILT_PRODUCTS_DIR")]
    products_dir: String,
    #[serde(rename = "FULL_PRODUCT_NAME")]
    product_name: String,
}

/// The `JP` target's bundle path, from `xcodebuild -showBuildSettings -json`.
///
/// Parsing starts at the first `[` because xcodebuild prepends free-form
/// notices about ambiguous destinations, and which stream those land on varies
/// by version.
fn bundle_path(raw: &str) -> Result<Utf8PathBuf, Error> {
    let json = raw
        .find('[')
        .map(|start| &raw[start..])
        .ok_or("`xcodebuild -showBuildSettings -json` produced no JSON")?;

    let targets: Vec<TargetSettings> = serde_json::from_str(json)
        .map_err(|e| format!("Failed to parse xcodebuild build settings: {e}"))?;

    let target = targets
        .into_iter()
        .find(|t| t.target == "JP")
        .ok_or("xcodebuild reported no settings for the `JP` target")?;

    Ok(Utf8PathBuf::from(format!(
        "{}/{}",
        target.settings.products_dir, target.settings.product_name
    )))
}

/// Wait for the app to write its pid.
///
/// `open` reports no process id of its own, and matching on the executable path
/// cannot tell this instance from one the developer left running, so the app
/// reporting its own pid is the only unambiguous answer.
fn wait_for_pid(state_dir: &Utf8Path, timeout: Duration) -> Result<u32, Error> {
    let path = state_dir.join("pid");
    let deadline = Instant::now() + timeout;

    loop {
        if let Ok(raw) = fs::read_to_string(&path)
            && let Ok(pid) = raw.trim().parse::<u32>()
        {
            return Ok(pid);
        }

        if Instant::now() >= deadline {
            return Err(format!(
                "The app never reported its pid at {path} within {}s. It may have failed to \
                 start, or it may be a build without `JP_DEBUG_STATE_DIR` support.",
                timeout.as_secs()
            )
            .into());
        }

        thread::sleep(POLL_INTERVAL);
    }
}

/// Resolve a caller-supplied workspace path against the repository root.
///
/// The result is canonical.
/// The app stores whatever path it is given and keys windows by it, so a `.` or
/// a `..` left in would have the same workspace open twice under two spellings.
fn resolve_workspace(root: &Utf8Path, path: &str) -> Result<Utf8PathBuf, Error> {
    let candidate = if Utf8Path::new(path).is_absolute() {
        Utf8PathBuf::from(path)
    } else {
        root.join(path)
    };

    if !candidate.is_dir() {
        return Err(format!("No directory at {candidate}.").into());
    }

    candidate
        .canonicalize_utf8()
        .map_err(|e| format!("Failed to resolve {candidate}: {e}").into())
}

/// Create a workspace with an empty store at `path`, if it is not there
/// already.
fn create_scratch_workspace(path: &Utf8Path) -> Result<(), Error> {
    let store = path.join(".jp");
    fs::create_dir_all(&store)?;

    let id = store.join(".id");
    if id.exists() {
        return Ok(());
    }

    // `Id::load` reads the last line and rejects anything that is not five
    // characters of `[0-9a-z]`.
    fs::write(
        &id,
        format!("DO NOT EDIT THIS FILE! IT IS AUTO-GENERATED BY JP.\n{SCRATCH_WORKSPACE_ID}\n"),
    )
    .map_err(|e| format!("Failed to write {id}: {e}").into())
}

/// Ready the state directory for a launch.
///
/// The stale pid always goes, whichever mode this runs in: the app writes that
/// file once at startup, so leaving the previous run's value there would let
/// [`wait_for_pid`] return a dead process immediately and report success.
///
/// `fresh` decides the rest.
/// Emptying the directory leaves the app with no recents list, which is what
/// makes it consult `JP_WORKSPACE` — the app prefers its most recent workspace
/// over the environment.
/// Keeping the directory is what makes a quit-and-relaunch pair test
/// restoration, since the app then reopens what it had before and ignores
/// `JP_WORKSPACE`.
fn prepare_state_dir(dir: &Utf8Path, fresh: bool) -> Result<(), Error> {
    if fresh && dir.exists() {
        fs::remove_dir_all(dir).map_err(|e| format!("Failed to clear {dir}: {e}"))?;
    }

    fs::create_dir_all(dir).map_err(|e| format!("Failed to create {dir}: {e}"))?;

    let pid = dir.join("pid");
    match fs::remove_file(&pid) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Failed to remove the stale pid file at {pid}: {e}").into()),
    }
}

/// Render the launch report.
fn report(
    shortenings: &[Shortening],
    dir: &Utf8Path,
    session: &Session,
    out: &str,
    err: &str,
    archived: Option<&str>,
) -> String {
    let mut report = format!(
        "Launched the macOS app as pid {}.\n\n- bundle: `{}`\n- configuration: `{}`\n- workspace: \
         `{}`\n- state: `{}`\n- user data: `{}`\n- session: `{}`\n",
        session.pid,
        shorten(session.bundle.as_str(), shortenings),
        session.configuration,
        shorten(session.workspace.as_str(), shortenings),
        shorten(session.state_dir.as_str(), shortenings),
        shorten(session.user_data_dir.as_str(), shortenings),
        shorten(Session::path(dir).as_str(), shortenings),
    );

    if let Some(id) = archived {
        report.push_str(&format!(
            "\nThe previous run's traced intervals were archived as `{id}`, so \
             `debug_app_profile` with `mode: \"report\"` can still read them and compare this run \
             against them.\n"
        ));
    }

    if session.allocation_stacks {
        report.push_str(
            "\nThe app keeps a stack for every allocation, so `debug_app_profile` can record \
             allocations against it. **Timings in this session are distorted**: \
             `MallocStackLogging` costs 2x to 10x and not evenly, so allocation-heavy paths slow \
             disproportionately.\n",
        );
    }

    for (name, content) in [("stdout", out), ("stderr", err)] {
        if content.trim().is_empty() {
            continue;
        }

        report.push_str(&format!(
            "\nConsole ({name}):\n\n```\n{}\n```\n",
            content.trim_end()
        ));
    }

    if out.trim().is_empty() && err.trim().is_empty() {
        report.push_str("\nThe app wrote nothing to either console stream while starting.\n");
    }

    report
}

#[cfg(test)]
#[path = "launch_tests.rs"]
mod tests;
