use camino_tempfile::Utf8TempDir;
use jp_plugin::message::{OutputFormat, PathsInfo, WorkspaceInfo};
use serde_json::{Map, json};

use super::*;

/// The `init` running `jp release github` with `args` in the workspace at
/// `root`.
fn init_message(root: &Utf8Path, args: &[&str]) -> InitMessage {
    InitMessage {
        version: jp_plugin::PROTOCOL_VERSION,
        workspace: WorkspaceInfo {
            root: root.to_path_buf(),
            storage: root.join(".jp"),
            id: "test".to_owned(),
        },
        paths: PathsInfo::default(),
        config: json!({}),
        options: Map::new(),
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        log_level: 0,
        output_format: OutputFormat::default(),
        tool: None,
    }
}

/// The messages a reply in `output` holds, one per line.
fn messages(output: &[u8]) -> Vec<PluginToHost> {
    String::from_utf8(output.to_vec())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// Run a session the way the host does, starting with `message`.
fn session(message: &HostToPlugin) -> Vec<PluginToHost> {
    let input = serde_json::to_string(message).unwrap() + "\n";
    let mut output = vec![];
    run(input.as_bytes(), &mut output).unwrap();

    messages(&output)
}

/// Run the command `init` names, with `token` as the environment's.
fn command(init: &InitMessage, token: Option<&str>) -> Vec<PluginToHost> {
    let mut output = vec![];
    handle_command(init, token, &mut output).unwrap();

    messages(&output)
}

/// Everything the session printed.
fn printed(messages: &[PluginToHost]) -> String {
    messages
        .iter()
        .filter_map(|message| match message {
            PluginToHost::Print(print) => Some(print.text.as_str()),
            _ => None,
        })
        .collect()
}

/// How the session ended.
fn exit(messages: &[PluginToHost]) -> &ExitMessage {
    match messages.last() {
        Some(PluginToHost::Exit(exit)) => exit,
        other => panic!("expected an exit, got {other:?}"),
    }
}

#[test]
fn the_command_line_is_well_formed() {
    Args::command().debug_assert();
}

#[test]
fn describe_names_the_plugin_and_its_two_word_command() {
    let messages = session(&HostToPlugin::Describe);

    let [PluginToHost::Describe(describe)] = messages.as_slice() else {
        panic!("expected one describe, got {messages:?}");
    };
    assert_eq!(describe.name, "release-github");
    assert_eq!(describe.manifest.command, ["release", "github"]);
    assert_eq!(describe.manifest.protocol, REQUIRED_PROTOCOL);
}

#[test]
fn help_is_printed_and_is_no_failure() {
    let root = Utf8TempDir::new().unwrap();
    let messages = command(&init_message(root.path(), &["--help"]), None);

    assert!(printed(&messages).contains("publish"), "{messages:?}");
    assert!(printed(&messages).contains("sync"), "{messages:?}");
    assert_eq!(exit(&messages).code, 0);
}

/// Neither command can reach GitHub without a token, so both say where one goes
/// before reading anything else.
#[test]
fn publishing_and_syncing_need_a_token() {
    let root = Utf8TempDir::new().unwrap();

    for args in [&["publish", "0.3.0"][..], &["sync"][..]] {
        let messages = command(&init_message(root.path(), args), None);

        let exit = exit(&messages);
        assert_eq!(exit.code, 1, "{args:?}");
        assert_eq!(
            exit.reason.as_deref(),
            Some(no_token().as_str()),
            "{args:?}"
        );
    }
}

#[test]
fn publish_needs_user_local_storage_where_builds_are_kept() {
    let root = Utf8TempDir::new().unwrap();
    let mut init = init_message(root.path(), &["publish", "0.3.0"]);
    init.config = json!({ "plugins": { "command": { "release": { "options": {
        "repository": "https://github.com/acme/widgets"
    }}}}});

    let messages = command(&init, Some("token"));

    assert_eq!(
        exit(&messages).reason.as_deref(),
        Some("This workspace has no user-local storage, where `jp release build` keeps builds.")
    );
}

#[test]
fn publish_without_a_build_says_how_to_make_one() {
    let root = Utf8TempDir::new().unwrap();
    let mut init = init_message(root.path(), &["publish", "v0.3.0"]);
    init.config = json!({ "plugins": { "command": { "release": { "options": {
        "repository": "https://github.com/acme/widgets"
    }}}}});
    init.paths.user_workspace = Some(root.path().join("storage"));

    let messages = command(&init, Some("token"));

    assert_eq!(
        exit(&messages).reason.as_deref(),
        Some("No build of 0.3.0 is kept here; build one with `jp release build 0.3.0`.")
    );
}

/// Notes are rendered from the checkout, so a release whose notes branch hasn't
/// merged has nothing to describe its Release with.
#[test]
fn publish_before_the_notes_merge_says_to_merge_them() {
    let root = Utf8TempDir::new().unwrap();
    let build = root.path().join("storage/release/0.3.0");
    fs::create_dir_all(&build).unwrap();
    fs::write(build.join("jp.tar.xz"), "an archive\n").unwrap();
    fs::write(
        build.join("checksums.txt"),
        "4cd36b3ca046f7c17e81e1aa31bf2dc6a91b98c94238810db57456b4904d3a22  jp.tar.xz\n",
    )
    .unwrap();
    fs::write(
        build.join("manifest.json"),
        json!({
            "format": 1,
            "version": "0.3.0",
            "commit": "0123456789abcdef0123456789abcdef01234567",
            "profile": "release",
            "artifacts": [{ "file": "jp.tar.xz", "target": "aarch64-apple-darwin" }],
        })
        .to_string(),
    )
    .unwrap();
    let mut init = init_message(root.path(), &["publish", "0.3.0"]);
    init.config = json!({ "plugins": { "command": { "release": { "options": {
        "repository": "https://github.com/acme/widgets"
    }}}}});
    init.paths.user_workspace = Some(root.path().join("storage"));

    let messages = command(&init, Some("token"));

    assert_eq!(
        exit(&messages).reason.as_deref(),
        Some(
            "changelog.d has no notes for 0.3.0; merge its notes branch, `release-notes/v0.3.0`, \
             into this checkout first."
        )
    );
}

#[test]
fn a_repository_that_is_not_on_github_is_refused() {
    let root = Utf8TempDir::new().unwrap();
    let mut init = init_message(root.path(), &["sync"]);
    init.config = json!({ "plugins": { "command": { "release": { "options": {
        "repository": "https://gitlab.com/acme/tools/widgets"
    }}}}});

    let messages = command(&init, Some("token"));

    assert_eq!(
        exit(&messages).reason.as_deref(),
        Some(
            "`https://gitlab.com/acme/tools/widgets` isn't the address of a repository on GitHub, \
             which reads as `https://github.com/<owner>/<name>`."
        )
    );
}

#[test]
fn a_tool_call_is_refused() {
    let root = Utf8TempDir::new().unwrap();
    let mut init = init_message(root.path(), &[]);
    init.tool = Some(serde_json::from_value(json!({ "name": "release_publish" })).unwrap());

    let messages = session(&HostToPlugin::Init(Box::new(init)));

    assert_eq!(
        exit(&messages).reason.as_deref(),
        Some("`jp release github` serves no tools.")
    );
}
