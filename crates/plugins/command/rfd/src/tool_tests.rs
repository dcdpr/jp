use std::fs;

use camino_tempfile::Utf8TempDir;
use jp_plugin::message::{OutputFormat, PathsInfo, WorkspaceInfo};
use jp_tool::FsRule;
use serde_json::json;

use super::*;

/// A workspace with one planned RFD, in development, and one draft.
fn workspace() -> Utf8TempDir {
    let root = Utf8TempDir::new().unwrap();
    let rfds = root.path().join("docs/rfd");
    let tickets = root.path().join("docs/ticket");
    fs::create_dir_all(rfds.join("drafts")).unwrap();
    fs::create_dir_all(&tickets).unwrap();

    fs::write(
        rfds.join("042-ordered-things.md"),
        "# RFD 042: Ordered Things\n\n- **Status**: Accepted\n- **Category**: Design\n- \
         **Summary**: Keeps things in order.\n",
    )
    .unwrap();
    fs::write(
        rfds.join("drafts/D05-sketch.md"),
        "# RFD D05: Sketch\n\n- **Status**: Draft\n- **Category**: Guide\n",
    )
    .unwrap();
    fs::write(
        rfds.join(".priority.json"),
        r#"{"planned": [{"milestone": null, "ids": ["042"]}], "backlog": ["D05"]}"#,
    )
    .unwrap();
    fs::write(
        tickets.join("02wt0k1-track.md"),
        "# Ordered things\n\n- **Status**: In Progress\n- **Kind**: Feature\n- **Authors**: jp\n- \
         **Date**: 2026-08-05\n- **Implements**: 042\n- **Label**: type=tracking\n",
    )
    .unwrap();

    root
}

/// The workspace a call runs in, as the host describes it in `init`.
fn init(root: &Utf8TempDir, config: Value) -> InitMessage {
    InitMessage {
        version: jp_plugin::PROTOCOL_VERSION,
        workspace: WorkspaceInfo {
            root: root.path().to_path_buf(),
            storage: root.path().join(".jp"),
            id: "test".to_owned(),
        },
        paths: PathsInfo::default(),
        config,
        options: Map::new(),
        args: vec![],
        log_level: 0,
        output_format: OutputFormat::Text,
        tool: None,
    }
}

fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

fn call(name: &str, arguments: Value) -> ToolCall {
    ToolCall {
        action: ToolAction::Run,
        name: name.to_owned(),
        arguments: object(arguments),
        answers: Map::new(),
        options: Map::new(),
        access: None,
        conversation: "test".to_owned(),
    }
}

fn run_tool(root: &Utf8TempDir, name: &str, arguments: Value) -> Outcome {
    run(
        &init(root, json!({})),
        &call(name, arguments),
        &Options::default(),
    )
}

fn content(outcome: Outcome) -> String {
    match outcome {
        Outcome::Success { content } => content,
        other => panic!("expected success, got: {other:?}"),
    }
}

fn error_message(outcome: Outcome) -> String {
    match outcome {
        Outcome::Error { message, .. } => message,
        other => panic!("expected error, got: {other:?}"),
    }
}

#[test]
fn list_prints_the_planned_rfds() {
    let root = workspace();

    assert_eq!(
        content(run_tool(&root, "list", json!({}))),
        concat!(
            "--planned (1), --backlog (1), --all (2), --full\n",
            "\n",
            "Planned (priority order, * in development)\n",
            "\n",
            "  1  *  042  Accepted    Ordered Things\n",
        )
    );
}

#[test]
fn list_takes_the_sections_and_category_the_command_does() {
    let root = workspace();

    let out = content(run_tool(
        &root,
        "list",
        json!({ "all": true, "full": true, "category": "guide" }),
    ));

    assert_eq!(
        out,
        concat!(
            "--planned (0), --backlog (1), --all (1)\n",
            "\n",
            "Backlog (not yet ranked)\n",
            "\n",
            "        D05  Draft       Sketch\n",
        )
    );
}

#[test]
fn show_prints_one_rfd_as_markdown() {
    let root = workspace();

    let out = content(run_tool(&root, "show", json!({ "id": "42" })));

    assert_eq!(
        out,
        concat!(
            "```markdown\n",
            "# RFD 042: Ordered Things\n",
            "\n",
            "- **File**: docs/rfd/042-ordered-things.md\n",
            "- **Status**: Accepted\n",
            "- **Category**: Design\n",
            "- **Board**: planned, rank 1\n",
            "- **In development**: yes\n",
            "\n",
            "Keeps things in order.\n",
            "```",
        )
    );
}

/// A model passes `42` as a number as readily as a string, and a draft by its
/// slot.
#[test]
fn show_takes_an_id_as_a_number_or_a_draft_slot() {
    let root = workspace();

    assert!(content(run_tool(&root, "show", json!({ "id": 42 }))).contains("# RFD 042:"));
    assert!(content(run_tool(&root, "show", json!({ "id": "d5" }))).contains("# RFD D05:"));
}

#[test]
fn show_names_what_is_wrong_with_an_id() {
    let root = workspace();

    assert_eq!(
        error_message(run_tool(&root, "show", json!({}))),
        "Missing argument `id`."
    );
    assert_eq!(
        error_message(run_tool(&root, "show", json!({ "id": "nope" }))),
        "Invalid RFD id 'nope'. Use a number (41) or draft ID (D01)."
    );
    assert_eq!(
        error_message(run_tool(&root, "show", json!({ "id": true }))),
        "`true` is not an RFD id."
    );
    assert_eq!(
        error_message(run_tool(&root, "show", json!({ "id": "043" }))),
        "No RFD found with number 043."
    );
}

#[test]
fn an_unknown_tool_names_the_ones_served() {
    let root = workspace();

    assert_eq!(
        error_message(run_tool(&root, "frobnicate", json!({}))),
        "`jp rfd` serves no tool named `frobnicate`. It serves: list, show."
    );
}

#[test]
fn every_served_tool_is_dispatched() {
    let root = workspace();

    for name in TOOLS {
        if let Outcome::Error { message, .. } = run_tool(&root, name, json!({ "id": "042" })) {
            assert!(
                !message.contains("serves no tool named"),
                "{name} is listed but not dispatched"
            );
        }
    }
}

#[test]
fn a_tool_without_a_preview_says_so() {
    let root = workspace();
    let mut call = call("list", json!({}));
    call.action = ToolAction::FormatArguments;

    assert_eq!(
        error_message(run(&init(&root, json!({})), &call, &Options::default())),
        "`list` has no preview of its own; style its parameters as `function_call` or `json` \
         rather than `tool`."
    );
}

/// The host compiles the call's access grants and the plugin enforces them:
/// reading the RFDs has to be allowed, and so does reading the tickets, which
/// say what is in development.
#[test]
fn the_access_policy_has_to_allow_reading() {
    let root = workspace();
    let rfds_only = AccessPolicy {
        fs: vec![FsRule::new("docs/rfd").with_read(true)],
        ..AccessPolicy::default()
    };
    let everything = AccessPolicy {
        fs: vec![FsRule::new("").with_read(true)],
        ..AccessPolicy::default()
    };
    let with_access = |policy: &AccessPolicy| {
        let mut call = call("list", json!({}));
        call.access = Some(serde_json::to_value(policy).unwrap());
        run(&init(&root, json!({})), &call, &Options::default())
    };

    let message = error_message(with_access(&rfds_only));
    assert!(
        message.starts_with("`list` can't read the ticket directory: access denied: read on"),
        "{message}"
    );
    assert!(content(with_access(&everything)).contains("042"));
}

/// Without a ticket directory there is nothing to read there, so the policy
/// isn't asked about one.
#[test]
fn a_missing_ticket_directory_needs_no_access() {
    let root = workspace();
    fs::remove_dir_all(root.path().join("docs/ticket")).unwrap();
    let mut call = call("list", json!({}));
    call.access = Some(
        serde_json::to_value(AccessPolicy {
            fs: vec![FsRule::new("docs/rfd").with_read(true)],
            ..AccessPolicy::default()
        })
        .unwrap(),
    );

    let out = content(run(&init(&root, json!({})), &call, &Options::default()));

    // Nothing reads as in development without tickets.
    assert!(out.contains("  1     042"), "{out}");
}

#[test]
fn an_unreadable_access_policy_refuses_the_call() {
    let root = workspace();
    let mut call = call("list", json!({}));
    call.access = Some(json!({ "fs": "everything" }));

    let message = error_message(run(&init(&root, json!({})), &call, &Options::default()));

    assert!(
        message.starts_with("The access policy for this call can't be read: "),
        "{message}"
    );
}

/// A tool reaches only the workspace, whichever directory points outside it.
#[test]
fn a_directory_outside_the_workspace_is_refused() {
    let root = workspace();
    let elsewhere = Utf8TempDir::new().unwrap();

    let outside_rfds = Options {
        dir: elsewhere.path().to_path_buf(),
        ..Options::default()
    };
    assert_eq!(
        error_message(run(
            &init(&root, json!({})),
            &call("list", json!({})),
            &outside_rfds
        )),
        format!(
            "The RFD directory {} is outside the workspace, which is all a tool reaches.",
            elsewhere.path()
        )
    );

    let outside_tickets = json!({
        "plugins": { "command": { "ticket": { "options": { "dir": elsewhere.path() } } } }
    });
    assert_eq!(
        error_message(run(
            &init(&root, outside_tickets),
            &call("list", json!({})),
            &Options::default()
        )),
        format!(
            "The ticket directory {} is outside the workspace, which is all a tool reaches.",
            elsewhere.path()
        )
    );
}

/// The ticket directory follows `jp ticket`'s configuration.
#[test]
fn the_ticket_directory_is_the_one_jp_ticket_uses() {
    let root = workspace();
    fs::rename(root.path().join("docs/ticket"), root.path().join("work")).unwrap();
    let config = json!({
        "plugins": { "command": { "ticket": { "options": { "dir": "work" } } } }
    });

    let out = content(run(
        &init(&root, config),
        &call("list", json!({})),
        &Options::default(),
    ));

    assert!(out.contains("  1  *  042"), "{out}");
}
