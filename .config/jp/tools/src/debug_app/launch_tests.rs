use std::{fs, time::Duration};

use camino::Utf8Path;

use super::{
    Appearance, bundle_identifier, bundle_path, create_scratch_workspace, format_preview,
    open_args, prepare_state_dir, resolve_workspace, run, stage_bundle, wait_for_pid,
};
use crate::{
    debug_app::{
        launch::LaunchSpec,
        session::{Console, Session, Slot},
    },
    util::runner::MockProcessRunner,
};

/// What `xcodebuild -showBuildSettings -json` reports, trimmed to the two keys
/// that matter and the noise it puts in front of them.
const BUILD_SETTINGS: &str = r#"--- xcodebuild: WARNING: Using the first of multiple matching destinations:
{ platform:macOS, arch:arm64, id:00006021-0004606A3E30C01E, name:My Mac }
[
  {
    "action": "build",
    "target": "JPTests",
    "buildSettings": {
      "BUILT_PRODUCTS_DIR": "/derived/Build/Products/Debug",
      "FULL_PRODUCT_NAME": "JPTests.xctest"
    }
  },
  {
    "action": "build",
    "target": "JP",
    "buildSettings": {
      "BUILT_PRODUCTS_DIR": "/derived/Build/Products/Debug",
      "FULL_PRODUCT_NAME": "JP.app"
    }
  }
]"#;

fn spec(allocation_stacks: bool) -> LaunchSpec {
    styled_spec(allocation_stacks, None)
}

fn styled_spec(allocation_stacks: bool, appearance: Option<Appearance>) -> LaunchSpec {
    windowed_spec(allocation_stacks, appearance, false)
}

fn windowed_spec(
    allocation_stacks: bool,
    appearance: Option<Appearance>,
    ignore_saved_windows: bool,
) -> LaunchSpec {
    LaunchSpec {
        bundle: "/derived/JP.app".into(),
        workspace: "/repo/tmp/debug-app/workspace".into(),
        state_dir: "/repo/tmp/debug-app/state".into(),
        user_data_dir: "/repo/tmp/debug-app/data".into(),
        stdout: "/repo/tmp/debug-app/console.out".into(),
        stderr: "/repo/tmp/debug-app/console.err".into(),
        trace: "/repo/tmp/debug-app/state/trace.jsonl".into(),
        allocation_stacks,
        appearance,
        ignore_saved_windows,
    }
}

#[test]
fn bundle_path_finds_the_app_target_past_the_leading_notice() {
    assert_eq!(
        bundle_path(BUILD_SETTINGS).unwrap(),
        "/derived/Build/Products/Debug/JP.app"
    );
}

#[test]
fn bundle_path_rejects_settings_without_the_app_target() {
    let settings = r#"[{"target": "JPTests", "buildSettings": {"BUILT_PRODUCTS_DIR": "/d", "FULL_PRODUCT_NAME": "JPTests.xctest"}}]"#;

    assert_eq!(
        bundle_path(settings).unwrap_err().to_string(),
        "xcodebuild reported no settings for the `JP` target"
    );
}

#[test]
fn bundle_path_rejects_output_holding_no_json() {
    assert_eq!(
        bundle_path("xcodebuild: error: nothing to see")
            .unwrap_err()
            .to_string(),
        "`xcodebuild -showBuildSettings -json` produced no JSON"
    );
}

/// The whole isolation story is in this argument vector, so it is pinned
/// exactly.
/// `-g` is part of it: without it every launch steals keyboard focus from
/// whatever the caller was typing into.
#[test]
fn open_args_carry_the_environment_and_both_redirects() {
    assert_eq!(open_args(&spec(false)), vec![
        "-n",
        "-g",
        "-a",
        "/derived/JP.app",
        "--env",
        "JP_DEBUG_STATE_DIR=/repo/tmp/debug-app/state",
        "--env",
        "JP_USER_DATA_DIR=/repo/tmp/debug-app/data",
        "--env",
        "JP_WORKSPACE=/repo/tmp/debug-app/workspace",
        "--stdout",
        "/repo/tmp/debug-app/console.out",
        "--stderr",
        "/repo/tmp/debug-app/console.err",
    ]);
}

/// libmalloc reads `MallocStackLogging` at process start, so this is the only
/// moment allocation attribution can be turned on at all.
/// It reaches the app through the same `open` call, and only when the caller
/// asked for it.
#[test]
fn open_args_pass_malloc_stack_logging_only_when_asked() {
    assert_eq!(open_args(&spec(true)), vec![
        "-n",
        "-g",
        "-a",
        "/derived/JP.app",
        "--env",
        "JP_DEBUG_STATE_DIR=/repo/tmp/debug-app/state",
        "--env",
        "JP_USER_DATA_DIR=/repo/tmp/debug-app/data",
        "--env",
        "JP_WORKSPACE=/repo/tmp/debug-app/workspace",
        "--env",
        "MallocStackLogging=1",
        "--stdout",
        "/repo/tmp/debug-app/console.out",
        "--stderr",
        "/repo/tmp/debug-app/console.err",
    ]);

    assert!(!open_args(&spec(false)).contains(&"MallocStackLogging=1".to_owned()));
}

/// The staged copy is re-signed after its identifier is rewritten, and a new
/// signature carries only what it is given.
/// Without `--entitlements` the copy loses `get-task-allow`, which nothing
/// notices until a profile bracket tries to attach and finds it cannot.
#[test]
fn staging_signs_the_copy_so_a_profiler_can_attach() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let dir = workspace.path();
    let staged = dir.join("JP.app");
    let entitlements = dir.join("entitlements.plist");

    let runner = MockProcessRunner::builder()
        .expect("cp")
        .returns_success("")
        .expect("plutil")
        .returns_success("")
        .expect("codesign")
        .args(&[
            "--force",
            "--sign",
            "-",
            "--entitlements",
            entitlements.as_str(),
            staged.as_str(),
        ])
        .returns_success("");

    stage_bundle(
        Utf8Path::new("/derived/JP.app"),
        dir,
        &Slot::fixed("test"),
        dir,
        &runner,
    )
    .unwrap();

    let written = fs::read_to_string(&entitlements).unwrap();
    assert!(
        written.contains("<key>com.apple.security.get-task-allow</key>"),
        "unexpected entitlements: {written}"
    );
}

#[test]
fn wait_for_pid_reads_the_pid_the_app_reported() {
    let workspace = camino_tempfile::tempdir().unwrap();
    fs::write(workspace.path().join("pid"), "88223\n").unwrap();

    assert_eq!(
        wait_for_pid(workspace.path(), Duration::from_millis(50)).unwrap(),
        88223
    );
}

#[test]
fn wait_for_pid_gives_up_when_the_app_never_reports() {
    let workspace = camino_tempfile::tempdir().unwrap();

    let error = wait_for_pid(workspace.path(), Duration::from_millis(50))
        .unwrap_err()
        .to_string();

    assert!(
        error.starts_with(&format!(
            "The app never reported its pid at {}",
            workspace.path().join("pid")
        )),
        "unexpected error: {error}"
    );
}

#[test]
fn wait_for_pid_ignores_a_file_that_is_not_a_pid() {
    let workspace = camino_tempfile::tempdir().unwrap();
    fs::write(workspace.path().join("pid"), "not a number\n").unwrap();

    assert!(wait_for_pid(workspace.path(), Duration::from_millis(50)).is_err());
}

#[test]
fn create_scratch_workspace_writes_a_store_jp_accepts() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let path = workspace.path().join("scratch");

    create_scratch_workspace(&path).unwrap();

    let id = fs::read_to_string(path.join(".jp/.id")).unwrap();
    assert_eq!(
        id,
        "DO NOT EDIT THIS FILE! IT IS AUTO-GENERATED BY JP.\nprobe\n"
    );
}

/// Reusing the scratch workspace keeps its ID, so the app's user-local store
/// for it survives across runs.
#[test]
fn create_scratch_workspace_keeps_an_existing_id() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let path = workspace.path().join("scratch");
    fs::create_dir_all(path.join(".jp")).unwrap();
    fs::write(path.join(".jp/.id"), "kept\n").unwrap();

    create_scratch_workspace(&path).unwrap();

    assert_eq!(fs::read_to_string(path.join(".jp/.id")).unwrap(), "kept\n");
}

/// `.` is not a path component, so an uncanonicalized `<root>/.` both renders
/// as an empty string in the report and reaches the app as a second spelling of
/// a workspace it already keys windows by.
#[test]
fn resolve_workspace_canonicalizes_a_relative_path() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();
    let expected = root.canonicalize_utf8().unwrap();

    assert_eq!(resolve_workspace(root, ".").unwrap(), expected);
}

#[test]
fn resolve_workspace_rejects_a_path_with_no_directory() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();

    let error = resolve_workspace(root, "nope").unwrap_err().to_string();

    assert_eq!(error, format!("No directory at {}.", root.join("nope")));
}

/// Two agents on one identifier would restore each other's windows, and both
/// would write the recent-workspace list the developer's own app reads.
#[test]
fn each_slot_runs_under_its_own_bundle_identifier() {
    assert_eq!(
        bundle_identifier("default"),
        "computer.jp.jean-pierre.drive.default"
    );
    assert_ne!(bundle_identifier("one"), bundle_identifier("two"));

    // Never the identifier the developer's own build runs under.
    assert_ne!(bundle_identifier("default"), "computer.jp.jean-pierre");
}

#[test]
fn prepare_state_dir_when_fresh_drops_the_recents_list() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let state = workspace.path().join("state");
    fs::create_dir_all(&state).unwrap();
    fs::write(state.join("recents.json"), "[]").unwrap();
    fs::write(state.join("pid"), "1\n").unwrap();

    prepare_state_dir(&state, true).unwrap();

    assert!(!state.join("recents.json").exists());
    assert!(!state.join("pid").exists());
}

/// A relaunch that tests restoration keeps the list, but never the pid: the app
/// writes that once at startup, so a stale one would be read as this run's.
#[test]
fn prepare_state_dir_when_not_fresh_keeps_the_recents_list_but_not_the_pid() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let state = workspace.path().join("state");
    fs::create_dir_all(&state).unwrap();
    fs::write(state.join("recents.json"), "[\"/a\"]").unwrap();
    fs::write(state.join("pid"), "1\n").unwrap();

    prepare_state_dir(&state, false).unwrap();

    assert_eq!(
        fs::read_to_string(state.join("recents.json")).unwrap(),
        "[\"/a\"]"
    );
    assert!(!state.join("pid").exists());
}

#[test]
fn prepare_state_dir_creates_a_missing_directory() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let state = workspace.path().join("state");

    prepare_state_dir(&state, true).unwrap();

    assert!(state.is_dir());
}

/// Two instances would fight over one pid file and one state directory, leaving
/// neither addressable.
/// The refusal has to come before the build, which is the expensive part.
#[test]
fn run_refuses_while_an_app_is_already_running() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();
    let slot = Slot::fixed("test");
    let dir = Session::dir(root, &slot);
    let pid = std::process::id();

    let session = Session {
        pid,
        bundle: "/derived/JP.app".into(),
        configuration: "Debug".to_owned(),
        workspace: root.join("workspace"),
        state_dir: dir.join("state"),
        user_data_dir: dir.join("data"),
        stdout: Console::new(dir.join("console.out")),
        stderr: Console::new(dir.join("console.err")),
        trace: Console::new(dir.join("state/trace.jsonl")),
        reported_footprint_mb: None,
        dsym: None,
        allocation_stacks: false,
    };
    session.store(&dir).unwrap();
    fs::create_dir_all(&session.state_dir).unwrap();
    fs::write(session.pid_path(), format!("{pid}\n")).unwrap();

    // Fails the test on any command at all, so a refusal that still built the app
    // cannot pass.
    let runner = MockProcessRunner::never_called();
    let outcome = run(root, &slot, None, "Debug", true, false, None, &runner).unwrap();

    let jp_tool::Outcome::Error { message, .. } = outcome else {
        panic!("expected an error outcome, got: {outcome:?}");
    };
    // Named relative to the repository, like every other path a report prints.
    assert_eq!(
        message,
        format!(
            "An app session is already running as pid {pid}, launched against workspace. Stop it \
             with `debug_app_quit` before launching another."
        )
    );
    assert!(session.workspace.starts_with(root));
}

#[test]
fn preview_names_the_workspace_and_the_state_handling() {
    let fresh = format_preview(None, "Debug", true, false, None);
    assert!(fresh.contains("tmp/debug-app/workspace (scratch, created if missing)"));
    assert!(fresh.contains("State directory is emptied first"));
    assert!(!fresh.contains("MallocStackLogging"));

    let kept = format_preview(Some("/repo"), "Release", false, false, None);
    assert!(kept.contains("JP_WORKSPACE=/repo"));
    assert!(kept.contains("just build-app Release"));
    assert!(kept.contains("State directory is kept"));
}

/// Asking for allocation stacks costs 2x to 10x on every timing in the session,
/// so approving the launch means seeing that.
#[test]
fn preview_names_the_cost_of_allocation_stacks() {
    let preview = format_preview(None, "Debug", true, true, None);

    assert!(preview.contains("--env MallocStackLogging=1"), "{preview}");
    assert!(
        preview.contains("2x\nto 10x"),
        "unexpected preview: {preview}"
    );
}

/// Everything after `--args` goes to the app rather than to `open`, so the
/// appearance has to be the last thing on the command line: an `open` flag
/// after it would be swallowed by the app and silently do nothing.
#[test]
fn appearance_is_passed_to_the_app_after_every_open_flag() {
    let args = open_args(&styled_spec(false, Some(Appearance::Dark)));

    assert_eq!(args.iter().rev().take(3).rev().collect::<Vec<_>>(), [
        "--args",
        "-AppleInterfaceStyle",
        "Dark"
    ]);
}

/// Both arguments go after one `--args`.
/// A second `--args` would be handed to the app as a literal argument rather
/// than starting another list, so the flag after it would be read by nobody.
#[test]
fn every_app_argument_goes_after_a_single_args_marker() {
    let args = open_args(&windowed_spec(false, Some(Appearance::Dark), true));

    assert_eq!(
        args.iter().filter(|arg| *arg == "--args").count(),
        1,
        "{args:?}"
    );
    assert_eq!(args.iter().rev().take(5).rev().collect::<Vec<_>>(), [
        "--args",
        "-ApplePersistenceIgnoreState",
        "YES",
        "-AppleInterfaceStyle",
        "Dark"
    ]);
}

/// A window restored to no size is absent from the window server's list, so it
/// can be neither read, captured, nor resized back: a fresh run has to be free
/// of whatever the last one saved.
#[test]
fn a_fresh_run_ignores_the_window_state_appkit_saved() {
    let args = open_args(&windowed_spec(false, None, true));

    assert!(
        args.contains(&"-ApplePersistenceIgnoreState".to_owned()),
        "{args:?}"
    );
}

/// Keeping the state is what `fresh: false` is for, and window restoration is
/// most of what there is to observe about it.
#[test]
fn a_run_keeping_its_state_restores_its_windows() {
    let args = open_args(&windowed_spec(false, None, false));

    assert!(
        !args.contains(&"-ApplePersistenceIgnoreState".to_owned()),
        "{args:?}"
    );
}

/// Light is named rather than left out, so the app draws light on a machine set
/// to dark instead of following it.
#[test]
fn light_appearance_is_named_explicitly() {
    let args = open_args(&styled_spec(false, Some(Appearance::Light)));

    assert!(args.contains(&"Light".to_owned()), "{args:?}");
}

/// Following the system is the default, and passes nothing at all: an app given
/// an empty `--args` list is not the same as one given none.
#[test]
fn no_appearance_passes_no_arguments_to_the_app() {
    let args = open_args(&spec(false));

    assert!(!args.contains(&"--args".to_owned()), "{args:?}");
}

#[test]
fn appearance_refuses_a_value_that_is_neither_light_nor_dark() {
    let error = Appearance::parse("sepia").unwrap_err().to_string();

    assert!(
        error.starts_with("`appearance` takes `light` or `dark`"),
        "{error}"
    );
}

/// Approving a launch means seeing that the app will not follow the machine's
/// own appearance.
#[test]
fn preview_names_the_appearance_it_forces() {
    let preview = format_preview(None, "Debug", true, false, Some(Appearance::Dark));

    assert!(
        preview.contains("--args -AppleInterfaceStyle Dark"),
        "{preview}"
    );
    assert!(preview.contains("draws in dark appearance"), "{preview}");
}
