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
        "`jp rfd` serves no tool named `frobnicate`. It serves: abandon, draft, extend, list, \
         promote, rename, renumber, require, show, start, stop, supersede, track."
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
        message.starts_with("`list` can't reach the ticket directory: access denied: read on"),
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

fn with_policy(name: &str, arguments: Value, rules: Vec<FsRule>) -> ToolCall {
    let mut call = call(name, arguments);
    call.access = Some(
        serde_json::to_value(AccessPolicy {
            fs: rules,
            ..AccessPolicy::default()
        })
        .unwrap(),
    );

    call
}

/// The configuration a draft takes its author from.
fn by_jane() -> Value {
    json!({ "user": { "name": "Jane Doe <jane@example.com>" } })
}

fn with_template(root: &Utf8TempDir) {
    fs::write(
        root.path().join("docs/rfd/000-design-template.md"),
        "# RFD NNN: TITLE\n\n- **Status**: Draft\n- **Category**: Design\n- **Authors**: \
         AUTHOR\n- **Date**: DATE\n\n## Summary\n\nWhat.\n",
    )
    .unwrap();
}

#[test]
fn draft_writes_a_draft() {
    let root = workspace();
    with_template(&root);

    let out = content(run(
        &init(&root, by_jane()),
        &call(
            "draft",
            json!({ "category": "design", "title": "Tool Limits" }),
        ),
        &Options::default(),
    ));

    assert_eq!(out, "Created docs/rfd/drafts/D01-tool-limits.md\n");
    let draft = fs::read_to_string(root.path().join("docs/rfd/drafts/D01-tool-limits.md")).unwrap();
    assert!(
        draft.contains("- **Authors**: Jane Doe <jane@example.com>\n"),
        "{draft}"
    );
}

/// Without a ticket the title is the draft's only source of one; with a ticket,
/// the ticket's title serves.
#[test]
fn draft_needs_a_title_unless_it_has_a_ticket() {
    let root = workspace();
    with_template(&root);
    let draft = |arguments: Value| {
        run(
            &init(&root, by_jane()),
            &call("draft", arguments),
            &Options::default(),
        )
    };

    assert_eq!(
        error_message(draft(json!({ "category": "design" }))),
        "Missing argument `title`."
    );
    assert_eq!(
        content(draft(json!({ "category": "design", "ticket": "02wt0k1" }))),
        "Created docs/rfd/drafts/D01-ordered-things.md\ndocs/ticket/02wt0k1-track.md: promoted to \
         D01, closed as Done\n"
    );
    assert_eq!(
        error_message(draft(json!({ "category": "design", "ticket": "nope" }))),
        "`nope` is not a ticket id (try T-02wt0kx)."
    );
}

/// Drafting from a ticket writes to the ticket, which the policy has to allow.
#[test]
fn draft_from_a_ticket_has_to_be_allowed_to_write_it() {
    let root = workspace();
    with_template(&root);
    let rules = vec![
        FsRule::new("docs/rfd").with_read(true).with_write(true),
        FsRule::new("docs/ticket").with_read(true),
    ];

    let message = error_message(run(
        &init(&root, by_jane()),
        &with_policy(
            "draft",
            json!({ "category": "design", "ticket": "02wt0k1" }),
            rules,
        ),
        &Options::default(),
    ));

    assert!(
        message.starts_with("`draft` can't reach the ticket directory: access denied: update on"),
        "{message}"
    );
    assert!(
        !root
            .path()
            .join("docs/rfd/drafts/D01-ordered-things.md")
            .exists()
    );
}

/// Publishing a draft moves its file; accepting or implementing an RFD only
/// edits files.
/// The policy is asked for what the step does.
#[test]
fn promote_asks_for_what_the_step_does() {
    let root = workspace();
    let edit_only = || {
        vec![
            FsRule::new("docs/rfd").with_read(true).with_update(true),
            FsRule::new("docs/ticket").with_read(true).with_update(true),
        ]
    };

    let message = error_message(run(
        &init(&root, json!({})),
        &with_policy("promote", json!({ "id": "D05" }), edit_only()),
        &Options::default(),
    ));
    assert!(
        message.starts_with("`promote` can't reach the RFD directory: access denied: create on"),
        "{message}"
    );

    let out = content(run(
        &init(&root, json!({})),
        &with_policy("promote", json!({ "id": 42 }), edit_only()),
        &Options::default(),
    ));
    assert_eq!(
        out,
        "docs/rfd/042-ordered-things.md: Accepted -> Implemented\n"
    );
}

#[test]
fn promote_publishes_a_draft() {
    let root = workspace();

    let out = content(run_tool(&root, "promote", json!({ "id": "D05" })));

    assert_eq!(
        out,
        "docs/rfd/001-sketch.md: Draft -> Discussion (assigned 001)\n"
    );
    assert!(root.path().join("docs/rfd/001-sketch.md").exists());
}

/// The relationship tools take the RFD that holds the relationship as `id`, the
/// way the commands take it first.
#[test]
fn relationship_tools_take_the_holding_rfd_as_id() {
    let root = workspace();

    assert_eq!(
        content(run_tool(
            &root,
            "extend",
            json!({ "id": "D05", "target": "042" })
        )),
        "docs/rfd/drafts/D05-sketch.md: Extends: RFD 042\ndocs/rfd/042-ordered-things.md: skipped \
         Extended by: RFD D05 (suppressed: draft -> non-draft)\n"
    );
    assert_eq!(
        error_message(run_tool(&root, "require", json!({ "id": "042" }))),
        "Missing argument `target`."
    );
    assert_eq!(
        error_message(run_tool(
            &root,
            "supersede",
            json!({ "id": "042", "target": "043" })
        )),
        "Missing argument `by`."
    );
}

/// What a command leaves for someone to check reaches the assistant with what
/// it changed.
#[test]
fn warnings_follow_the_result() {
    let root = workspace();
    fs::write(
        root.path().join("docs/rfd/043-depended-on.md"),
        "# RFD 043: Depended On\n\n- **Status**: Accepted\n- **Required by**: [RFD \
         050](050-x.md)\n",
    )
    .unwrap();

    let out = content(run_tool(
        &root,
        "abandon",
        json!({ "id": "043", "reason": "Not needed." }),
    ));

    assert_eq!(
        out,
        "docs/rfd/043-depended-on.md: Abandoned (Accepted -> Abandoned)\nWarning: the following \
         RFDs depend on this one (Required by):\n  RFD 050\nTheir dependency on RFD 043 is now \
         broken; review and update.\n"
    );
}

#[test]
fn rename_and_renumber_move_the_file() {
    let root = workspace();

    let renamed = content(run_tool(
        &root,
        "rename",
        json!({ "id": "042", "title": "Kept in Order" }),
    ));
    assert!(
        renamed.contains("042-ordered-things.md -> docs/rfd/042-kept-in-order.md\n"),
        "{renamed}"
    );

    let renumbered = content(run_tool(
        &root,
        "renumber",
        json!({ "id": "042", "new": 97 }),
    ));
    assert!(
        renumbered.contains("042-kept-in-order.md -> docs/rfd/097-kept-in-order.md (042 -> 097)\n"),
        "{renumbered}"
    );
}

/// A call kept to the RFDs and the tickets can't search the rest of the
/// workspace for references, and says so.
#[test]
fn renumber_without_the_workspace_says_what_went_unchecked() {
    let root = workspace();
    let rules = vec![
        FsRule::new("docs/rfd").with_read(true).with_write(true),
        FsRule::new("docs/ticket").with_read(true).with_write(true),
    ];

    let out = content(run(
        &init(&root, json!({})),
        &with_policy("renumber", json!({ "id": "D05", "new": "D07" }), rules),
        &Options::default(),
    ));

    assert!(
        out.ends_with(
            "References outside docs/rfd/ and docs/ticket/ were not checked: the access policy \
             doesn't allow reading the workspace.\n"
        ),
        "{out}"
    );
    assert!(root.path().join("docs/rfd/drafts/D07-sketch.md").exists());
}

/// The workspace with RFD 043 Accepted and no tracking ticket yet.
fn trackable() -> Utf8TempDir {
    let root = workspace();
    fs::write(
        root.path().join("docs/rfd/043-more-order.md"),
        "# RFD 043: More Order\n\n- **Status**: Accepted\n",
    )
    .unwrap();

    root
}

/// The tracking ticket RFD 043 got, as written.
fn tracking_ticket(root: &Utf8TempDir) -> String {
    let name = fs::read_dir(root.path().join("docs/ticket"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .find(|name| name.ends_with("-more-order.md"))
        .expect("a tracking ticket for RFD 043");

    fs::read_to_string(root.path().join("docs/ticket").join(name)).unwrap()
}

/// The config whose ticket vocabulary defines the tracking label.
fn with_labels() -> Value {
    json!({ "plugins": { "command": { "ticket": { "options": {
        "labels": { "type": { "description": "The type", "values": ["tracking"] } }
    } } } } })
}

/// [`with_labels`], also naming who `jp ticket` attributes a tool's tickets to.
fn with_assistant() -> Value {
    let mut config = with_labels();
    config["plugins"]["command"]["ticket"]["options"]["assistant"] = json!("jp");

    config
}

/// A tool call can't have the plan read, so the ticket is filed with the marker
/// in its place, attributed to the assistant `jp ticket` names.
#[test]
fn track_files_the_ticket_with_the_plan_pending() {
    let root = trackable();

    let out = content(run(
        &init(&root, with_assistant()),
        &call("track", json!({ "id": 43 })),
        &Options::default(),
    ));

    assert!(out.starts_with("Filed docs/ticket/"), "{out}");
    assert!(
        out.ends_with(
            "The Implementation Plan of RFD 043 could not be read: a tool call can't ask the host \
             to run a query. The tracking ticket holds `<!-- rfd-plan: pending -->` in its place \
             until `jp rfd track 043` reads it.\n"
        ),
        "{out}"
    );
    let ticket = tracking_ticket(&root);
    assert!(ticket.contains("- **Authors**: jp\n"), "{ticket}");
    assert!(ticket.contains("- **Implements**: 043\n"), "{ticket}");
    assert!(ticket.ends_with("<!-- rfd-plan: pending -->\n"), "{ticket}");
}

#[test]
fn start_and_stop_move_the_tracking_ticket() {
    let root = trackable();

    let started = content(run(
        &init(&root, with_labels()),
        &call("start", json!({ "id": "043" })),
        &Options::default(),
    ));
    assert!(started.contains(": Todo -> In Progress\n"), "{started}");
    let ticket = tracking_ticket(&root);
    assert!(ticket.contains("- **Status**: In Progress\n"), "{ticket}");
    assert!(ticket.contains("- **Authors**: assistant\n"), "{ticket}");

    let stopped = content(run_tool(&root, "stop", json!({ "id": "043" })));
    assert!(stopped.ends_with(": In Progress -> Todo\n"), "{stopped}");
}

/// Filing a ticket writes in the ticket directory, which the policy has to
/// allow.
#[test]
fn track_has_to_be_allowed_to_write_tickets() {
    let root = trackable();
    let rules = vec![
        FsRule::new("docs/rfd").with_read(true),
        FsRule::new("docs/ticket").with_read(true),
    ];

    let message = error_message(run(
        &init(&root, json!({})),
        &with_policy("track", json!({ "id": "043" }), rules),
        &Options::default(),
    ));

    assert!(
        message.starts_with("`track` can't reach the ticket directory: access denied: create on"),
        "{message}"
    );
}

/// A tool call has no terminal to offer the tracking ticket on, so it says how
/// to file it instead.
#[test]
fn promoting_to_accepted_names_the_tracking_ticket_it_skipped() {
    let root = trackable();
    fs::write(
        root.path().join("docs/rfd/044-discussed.md"),
        "# RFD 044: Discussed\n\n- **Status**: Discussion\n",
    )
    .unwrap();

    let out = content(run_tool(&root, "promote", json!({ "id": "044" })));

    assert_eq!(
        out,
        "docs/rfd/044-discussed.md: Discussion -> Accepted\nSkipped the tracking ticket, which a \
         tool call doesn't offer. File it with `jp rfd track 044` when the work starts.\n"
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
