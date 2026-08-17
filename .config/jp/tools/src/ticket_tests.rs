use std::path::{Path, PathBuf};

use camino_tempfile::Utf8TempDir;
use jp_tool::{Action, Outcome};
use serde_json::json;

use super::*;

const DATE: &str = "2026-08-05";
const STAMP: &str = "2026-08-05T14:03:11Z";

/// The directory holding this module's tool declarations.
fn declarations() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../.jp/mcp/tools/ticket")
}

fn declaration(file: &str) -> toml::Value {
    let path = declarations().join(file);
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));

    toml::from_str(&content).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn advertised(file: &str, tool: &str, parameter: &str) -> Vec<String> {
    declaration(file)["conversation"]["tools"][tool]["parameters"][parameter]["enum"]
        .as_array()
        .unwrap_or_else(|| panic!("{tool}.{parameter} has no enum"))
        .iter()
        .map(|value| value.as_str().expect("string variant").to_owned())
        .collect()
}

/// Drive a tool through the public `run` entry point, exercising argument
/// parsing and dispatch.
fn run_tool(dir: &Utf8TempDir, name: &str, args: Value) -> ToolResult {
    dispatch(dir, Action::Run, name, args)
}

/// Drive a tool through the argument-formatting path JP takes before asking for
/// approval.
fn preview_tool(dir: &Utf8TempDir, name: &str, args: Value) -> ToolResult {
    dispatch(dir, Action::FormatArguments, name, args)
}

fn dispatch(dir: &Utf8TempDir, action: Action, name: &str, args: Value) -> ToolResult {
    let ctx = Context {
        root: dir.path().to_path_buf(),
        action,
        access: None,
        workspace_id: "test".into(),
        conversation_id: "test".into(),
    };
    let arguments = match args {
        Value::Object(map) => map,
        _ => serde_json::Map::new(),
    };

    run(ctx, Tool {
        name: name.to_owned(),
        arguments,
        answers: serde_json::Map::new(),
        options: serde_json::Map::new(),
    })
}

fn content(result: ToolResult) -> String {
    match result.expect("tool result") {
        Outcome::Success { content } => content,
        other => panic!("expected success, got: {other:?}"),
    }
}

/// A preview as a terminal shows it, with the ANSI styling removed.
fn strip_ansi(rendered: String) -> String {
    let bytes = strip_ansi_escapes::strip(rendered);
    String::from_utf8(bytes).expect("valid utf-8 after stripping ANSI")
}

fn error_message(result: ToolResult) -> String {
    match result.expect("tool result") {
        Outcome::Error { message, .. } => message,
        other => panic!("expected error, got: {other:?}"),
    }
}

fn create_ticket(dir: &Utf8TempDir, title: &str) -> String {
    content(run_tool(
        dir,
        "ticket_create",
        json!({
            "kind": "bug",
            "title": title,
            "body": "Something is wrong."
        }),
    ))
}

#[test]
fn create_reports_the_id_and_a_workspace_relative_path() {
    let dir = Utf8TempDir::new().unwrap();

    let out = create_ticket(&dir, "Tool call header misaligned");

    assert_eq!(
        out,
        "Created T0001 at docs/ticket/0001-tool-call-header-misaligned.md"
    );
    assert!(
        dir.path()
            .join("docs/ticket/0001-tool-call-header-misaligned.md")
            .exists()
    );
}

#[test]
fn create_preview_renders_the_file_that_will_be_written() {
    let out = strip_ansi(preview_create(
        TicketId::new(1),
        Kind::Bug,
        "Tool call header misaligned",
        Some("045"),
        Some("The header renders one column left of the body."),
        DATE,
    ));

    // The preview is quoted, so the rail shows where the ticket ends and the
    // conversation resumes. Written line by line because the blank lines carry
    // a trailing space that an editor would trim out of a block literal.
    assert_eq!(
        out,
        concat!(
            "> # T0001: Tool call header misaligned\n",
            "> \n",
            "> - **Status**: Todo\n",
            "> - **Kind**: Bug\n",
            "> - **Authors**: jp\n",
            "> - **Date**: 2026-08-05\n",
            "> - **Implements**: 045\n",
            "> \n",
            "> The header renders one column left of the body.\n",
        )
    );
}

/// The previewed id is the one `create` goes on to use, and the preview itself
/// leaves the board untouched.
#[test]
fn create_preview_writes_nothing() {
    let dir = Utf8TempDir::new().unwrap();
    create_ticket(&dir, "Tool call header misaligned");

    let out = strip_ansi(content(preview_tool(
        &dir,
        "ticket_create",
        json!({
            "kind": "chore",
            "title": "Bump the deny list"
        }),
    )));

    assert!(out.starts_with("> # T0002: Bump the deny list\n"), "{out}");
    assert_eq!(
        std::fs::read_dir(dir.path().join("docs/ticket"))
            .unwrap()
            .count(),
        2,
        "the preview added a file: only the first ticket and the counter belong here"
    );
    assert_eq!(
        content(run_tool(
            &dir,
            "ticket_create",
            json!({ "kind": "chore", "title": "Bump the deny list" })
        )),
        "Created T0002 at docs/ticket/0002-bump-the-deny-list.md"
    );
}

#[test]
fn comment_preview_renders_the_block_under_the_ticket_it_lands_on() {
    let dir = Utf8TempDir::new().unwrap();
    create_ticket(&dir, "Tool call header misaligned");
    run_tool(
        &dir,
        "ticket_comment",
        json!({ "id": 1, "body": "Reproduced at 72 columns." }),
    )
    .unwrap();

    let out = strip_ansi(preview_comment(
        dir.path(),
        TicketId::new(1),
        Some(1),
        "The wrap calculation is off.",
        STAMP,
    ));

    assert_eq!(
        out,
        concat!(
            "> # T0001: Tool call header misaligned\n",
            "> \n",
            "> ──────────────────────────────────────────────────────────────────────────────\n",
            "> \n",
            "> - **From**: jp\n",
            "> - **Date**: 2026-08-05T14:03:11Z\n",
            "> - **Re**: T0001#1\n",
            "> \n",
            "> The wrap calculation is off.\n",
        )
    );
}

/// An id that isn't on the board is worth knowing before approving the call,
/// not after.
#[test]
fn comment_preview_warns_about_a_missing_ticket() {
    let dir = Utf8TempDir::new().unwrap();

    let out = strip_ansi(preview_comment(
        dir.path(),
        TicketId::new(9),
        None,
        "Reproduced at 72 columns.",
        STAMP,
    ));

    assert_eq!(
        out,
        concat!(
            "> # T0009\n",
            "> \n",
            "> \u{26a0} No T0009; this call will fail.\n",
            "> \n",
            "> ──────────────────────────────────────────────────────────────────────────────\n",
            "> \n",
            "> - **From**: jp\n",
            "> - **Date**: 2026-08-05T14:03:11Z\n",
            "> \n",
            "> Reproduced at 72 columns.\n",
        )
    );
}

#[test]
fn create_rejects_an_unknown_kind() {
    let dir = Utf8TempDir::new().unwrap();

    let out = error_message(run_tool(
        &dir,
        "ticket_create",
        json!({
            "kind": "task",
            "title": "Title"
        }),
    ));

    assert_eq!(out, "`kind` must be one of: bug, feature, chore.");
}

#[test]
fn create_rejects_an_empty_title() {
    let dir = Utf8TempDir::new().unwrap();

    let out = error_message(run_tool(
        &dir,
        "ticket_create",
        json!({
            "kind": "chore",
            "title": "   "
        }),
    ));

    assert_eq!(out, "`title` must not be empty.");
}

#[test]
fn comments_are_attributed_to_the_assistant_and_numbered() {
    let dir = Utf8TempDir::new().unwrap();
    create_ticket(&dir, "Tool call header misaligned");

    let first = content(run_tool(
        &dir,
        "ticket_comment",
        json!({
            "id": 1,
            "body": "Reproduced at 72 columns."
        }),
    ));
    let second = content(run_tool(
        &dir,
        "ticket_comment",
        json!({
            "id": "T0001",
            "re": 1,
            "body": "The wrap calculation is off."
        }),
    ));

    assert_eq!(first, "Added T0001#1");
    assert_eq!(second, "Added T0001#2");

    let shown = content(run_tool(&dir, "ticket_show", json!({ "id": "1" })));
    assert!(shown.contains("### T0001#1 — jp at "), "{shown}");
    assert!(
        shown.contains("### T0001#2 — jp at ") && shown.contains("replying to T0001#1"),
        "{shown}"
    );
}

#[test]
fn a_reply_to_a_missing_comment_is_reported_to_the_model() {
    let dir = Utf8TempDir::new().unwrap();
    create_ticket(&dir, "Tool call header misaligned");

    let out = error_message(run_tool(
        &dir,
        "ticket_comment",
        json!({
            "id": 1,
            "re": 3,
            "body": "Reply."
        }),
    ));

    assert_eq!(out, "No T0001, or no comment #3 on it.");
}

#[test]
fn an_unusable_id_is_reported_to_the_model() {
    let dir = Utf8TempDir::new().unwrap();

    assert_eq!(
        error_message(run_tool(&dir, "ticket_close", json!({ "id": "nope" }))),
        "`nope` is not a ticket id (try 42, 042, or T0042)."
    );
    assert_eq!(
        error_message(run_tool(&dir, "ticket_close", json!({ "id": 7 }))),
        "No T0007."
    );
}

#[test]
fn close_reports_the_transition_once() {
    let dir = Utf8TempDir::new().unwrap();
    create_ticket(&dir, "Tool call header misaligned");

    assert_eq!(
        content(run_tool(&dir, "ticket_close", json!({ "id": 1 }))),
        "T0001: Todo -> Done"
    );
    assert_eq!(
        content(run_tool(&dir, "ticket_close", json!({ "id": 1 }))),
        "T0001 was already Done."
    );
}

#[test]
fn list_shows_one_line_per_ticket_and_filters() {
    let dir = Utf8TempDir::new().unwrap();
    create_ticket(&dir, "Still open");
    create_ticket(&dir, "Finished");
    run_tool(&dir, "ticket_close", json!({ "id": 2 })).unwrap();

    let all = content(run_tool(&dir, "ticket_list", json!({})));
    assert_eq!(
        all,
        "T0001  Todo         Bug      Still open\nT0002  Done         Bug      Finished\n"
    );

    let todo = content(run_tool(&dir, "ticket_list", json!({ "status": "todo" })));
    assert_eq!(todo, "T0001  Todo         Bug      Still open\n");

    let chores = content(run_tool(&dir, "ticket_list", json!({ "kind": "chore" })));
    assert_eq!(chores, "No tickets match.\n");
}

#[test]
fn list_names_the_files_it_could_not_read() {
    let dir = Utf8TempDir::new().unwrap();
    create_ticket(&dir, "Readable");
    std::fs::write(
        dir.path().join("docs/ticket/0009-mangled.md"),
        "no heading here\n",
    )
    .unwrap();

    let out = content(run_tool(&dir, "ticket_list", json!({})));

    assert!(out.starts_with("T0001  Todo"), "{out}");
    assert!(
        out.contains("- docs/ticket/0009-mangled.md: Ticket does not open with"),
        "{out}"
    );
}

#[test]
fn show_renders_metadata_and_the_description() {
    let dir = Utf8TempDir::new().unwrap();
    create_ticket(&dir, "Tool call header misaligned");

    let out = content(run_tool(&dir, "ticket_show", json!({ "id": 1 })));

    assert!(
        out.starts_with("# T0001: Tool call header misaligned\n"),
        "{out}"
    );
    assert!(
        out.contains("- **Path**: docs/ticket/0001-tool-call-header-misaligned.md"),
        "{out}"
    );
    assert!(out.contains("- **Status**: Todo"), "{out}");
    assert!(out.contains("- **Authors**: jp"), "{out}");
    assert!(out.contains("Something is wrong."), "{out}");
    assert!(out.contains("No comments yet."), "{out}");
}

#[test]
fn show_reports_a_missing_ticket() {
    let dir = Utf8TempDir::new().unwrap();

    assert_eq!(
        error_message(run_tool(&dir, "ticket_show", json!({ "id": 4 }))),
        "No T0004."
    );
}

#[test]
fn ids_are_read_from_strings_and_numbers() {
    assert_eq!(id_arg(&json!("T0042")), Ok(TicketId::new(42)));
    assert_eq!(id_arg(&json!("042")), Ok(TicketId::new(42)));
    assert_eq!(id_arg(&json!(42)), Ok(TicketId::new(42)));
    assert!(id_arg(&json!(0)).is_err());
    assert!(id_arg(&json!(-1)).is_err());
    assert!(id_arg(&json!(null)).is_err());
}

#[test]
fn an_unknown_ticket_tool_is_an_error() {
    let dir = Utf8TempDir::new().unwrap();

    assert!(run_tool(&dir, "ticket_frobnicate", json!({})).is_err());
}

/// Every tool declared under `.jp/mcp/tools/ticket/` must be one this module
/// dispatches.
///
/// A declared tool that `run` doesn't know is an "Unknown tool" at conversation
/// time, and a file that doesn't parse takes the whole workspace config with
/// it.
#[test]
fn every_declared_tool_is_dispatched() {
    let dir = Utf8TempDir::new().unwrap();

    let mut declared: Vec<String> = vec![];
    for entry in std::fs::read_dir(declarations()).expect("ticket tool declarations") {
        let file = entry.unwrap().file_name().to_string_lossy().into_owned();
        let value = declaration(&file);
        let tools = value["conversation"]["tools"]
            .as_table()
            .unwrap_or_else(|| panic!("{file} declares no tools"));

        declared.extend(tools.keys().cloned());
    }
    declared.sort();

    assert_eq!(declared, [
        "ticket_close",
        "ticket_comment",
        "ticket_create",
        "ticket_list",
        "ticket_show",
    ]);

    for name in &declared {
        if let Err(error) = run_tool(&dir, name, json!({})) {
            assert!(
                !error.to_string().starts_with("Unknown tool"),
                "{name} is declared but not dispatched"
            );
        }
    }
}

/// The values the tools advertise must be values the parser accepts, in both
/// directions: nothing advertised that is rejected, nothing accepted that is
/// hidden.
#[test]
fn advertised_values_match_the_parser() {
    for (file, tool) in [
        ("create.toml", "ticket_create"),
        ("list.toml", "ticket_list"),
    ] {
        let kinds = advertised(file, tool, "kind");
        for value in &kinds {
            value
                .parse::<Kind>()
                .unwrap_or_else(|_| panic!("{tool} advertises `{value}`, which is not a kind"));
        }
        for kind in [Kind::Bug, Kind::Feature, Kind::Chore] {
            assert!(
                kinds.iter().any(|value| value.parse() == Ok(kind)),
                "{tool} does not advertise {kind}"
            );
        }
    }

    let statuses = advertised("list.toml", "ticket_list", "status");
    for value in &statuses {
        value
            .parse::<Status>()
            .unwrap_or_else(|_| panic!("ticket_list advertises `{value}`, which is not a status"));
    }
    for status in [Status::Todo, Status::InProgress, Status::Done] {
        assert!(
            statuses.iter().any(|value| value.parse() == Ok(status)),
            "ticket_list does not advertise {status}"
        );
    }
}
