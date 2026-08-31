use std::{cell::RefCell, io::Cursor};

use jp_plugin::message::{PathsInfo, ReadyMessage, WorkspaceInfo};
use pretty_assertions::assert_eq;

use super::*;

/// Records what it was asked to launch, instead of launching it.
#[derive(Default)]
struct RecordingLauncher {
    launched: RefCell<Vec<(String, String)>>,
    fails: bool,
}

impl RecordingLauncher {
    fn failing() -> Self {
        Self {
            fails: true,
            ..Self::default()
        }
    }
}

impl Launcher for RecordingLauncher {
    fn launch(&self, bundle_id: &str, path: &str) -> Result<(), String> {
        self.launched
            .borrow_mut()
            .push((bundle_id.to_owned(), path.to_owned()));

        if self.fails {
            return Err("the app is not installed".to_owned());
        }

        Ok(())
    }
}

fn init_message(root: &str, args: &[&str]) -> String {
    let init = InitMessage {
        version: 1,
        workspace: WorkspaceInfo {
            root: root.into(),
            storage: format!("{root}/.jp").into(),
            id: "abcde".to_owned(),
        },
        paths: PathsInfo::default(),
        config: serde_json::json!({}),
        options: serde_json::Map::new(),
        args: args.iter().map(|a| (*a).to_owned()).collect(),
        log_level: 0,
    };

    format!(
        "{}\n",
        serde_json::to_string(&HostToPlugin::Init(init)).unwrap()
    )
}

/// Run the plugin against one host message, returning what it wrote back.
fn exchange(input: &str, launcher: &impl Launcher) -> Vec<PluginToHost> {
    let mut stdout = Vec::new();
    run(Cursor::new(input), &mut stdout, launcher).unwrap();

    String::from_utf8(stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn opens_the_workspace_the_host_resolved() {
    let launcher = RecordingLauncher::default();

    let sent = exchange(&init_message("/tmp/my-workspace", &[]), &launcher);

    assert_eq!(launcher.launched.borrow().as_slice(), [(
        "computer.jp.jean-pierre".to_owned(),
        "/tmp/my-workspace".to_owned()
    )]);
    assert_eq!(sent, vec![
        PluginToHost::Ready(ReadyMessage {
            protocol: REQUIRED_PROTOCOL
        }),
        PluginToHost::Exit(ExitMessage {
            code: 0,
            reason: None
        }),
    ]);
}

/// The host answers `-w/--workspace` before the plugin runs, so targeting
/// another workspace needs nothing here.
#[test]
fn opens_whatever_root_the_host_sent() {
    let launcher = RecordingLauncher::default();

    exchange(&init_message("/tmp/other", &[]), &launcher);

    assert_eq!(launcher.launched.borrow()[0].1, "/tmp/other");
}

/// A failure to launch is reported as a non-zero exit with a reason, rather
/// than a silent success.
#[test]
fn reports_a_failed_launch() {
    let launcher = RecordingLauncher::failing();

    let sent = exchange(&init_message("/tmp/my-workspace", &[]), &launcher);

    assert_eq!(sent, vec![
        PluginToHost::Ready(ReadyMessage {
            protocol: REQUIRED_PROTOCOL
        }),
        PluginToHost::Exit(ExitMessage {
            code: 1,
            reason: Some("the app is not installed".to_owned())
        }),
    ]);
}

#[test]
fn rejects_an_unknown_option() {
    let launcher = RecordingLauncher::default();

    let sent = exchange(&init_message("/tmp/my-workspace", &["--nope"]), &launcher);

    assert!(
        launcher.launched.borrow().is_empty(),
        "nothing should launch"
    );
    let PluginToHost::Exit(exit) = &sent[1] else {
        panic!("expected an exit message, got {:?}", sent[1]);
    };
    assert_eq!(exit.code, 1);
    assert!(
        exit.reason
            .as_deref()
            .is_some_and(|r| r.starts_with("unknown option: --nope")),
        "got: {:?}",
        exit.reason
    );
}

/// A trailing path that names a different workspace is a mistake: the host has
/// already chosen, so opening its choice would silently ignore the argument.
#[test]
fn rejects_a_path_outside_the_workspace() {
    let launcher = RecordingLauncher::default();
    let tmp = std::env::temp_dir();
    let outside = tmp.to_string_lossy().into_owned();

    let sent = exchange(
        &init_message(env!("CARGO_MANIFEST_DIR"), &[&outside]),
        &launcher,
    );

    assert!(
        launcher.launched.borrow().is_empty(),
        "nothing should launch"
    );
    let PluginToHost::Exit(exit) = &sent[1] else {
        panic!("expected an exit message, got {:?}", sent[1]);
    };
    assert_eq!(exit.code, 1);
    assert!(
        exit.reason
            .as_deref()
            .is_some_and(|r| r.contains("is not inside the workspace")),
        "got: {:?}",
        exit.reason
    );
}

/// `jp gui .` and `jp gui src/` name the workspace the host already resolved,
/// so they open it rather than erroring.
#[test]
fn accepts_a_path_inside_the_workspace() {
    let launcher = RecordingLauncher::default();
    let root = env!("CARGO_MANIFEST_DIR");

    exchange(&init_message(root, &[&format!("{root}/src")]), &launcher);

    assert_eq!(launcher.launched.borrow().len(), 1);
}

/// Every target the release workflow builds gets a `jp-gui` binary, because the
/// registry cannot express "macOS only".
/// The one a Linux user runs should say so.
#[test]
fn names_the_platform_when_it_cannot_open_the_app() {
    assert_eq!(launch::unsupported_platform("macos"), None);

    assert_eq!(
        launch::unsupported_platform("linux").as_deref(),
        Some("`jp gui` opens the JP macOS app, which does not run on linux.")
    );
    assert_eq!(
        launch::unsupported_platform("windows").as_deref(),
        Some("`jp gui` opens the JP macOS app, which does not run on windows.")
    );
}

#[test]
fn describes_itself_without_launching_anything() {
    let launcher = RecordingLauncher::default();
    let describe = format!(
        "{}\n",
        serde_json::to_string(&HostToPlugin::Describe).unwrap()
    );

    let sent = exchange(&describe, &launcher);

    assert!(
        launcher.launched.borrow().is_empty(),
        "nothing should launch"
    );
    let PluginToHost::Describe(response) = &sent[0] else {
        panic!("expected a describe response, got {:?}", sent[0]);
    };
    assert_eq!(response.name, "gui");
    assert_eq!(response.command, ["gui"]);
}
