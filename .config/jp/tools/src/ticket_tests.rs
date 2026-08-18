use std::path::{Path, PathBuf};

use camino_tempfile::Utf8TempDir;
use jp_tool::{Action, Outcome};
use serde_json::json;

use super::*;

const DATE: &str = "2026-08-05";
const STAMP: &str = "2026-08-05T14:03:11Z";

/// A ticket id the tests can name, since allocation draws unpredictable ones.
const FIXED_ID: &str = "T-02wt0kx";

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

/// File a ticket under [`FIXED_ID`], bypassing allocation so the test can
/// assert against the id it will read back.
fn write_ticket(dir: &Utf8TempDir, title: &str) -> TicketId {
    let id: TicketId = FIXED_ID.parse().unwrap();
    let tickets = dir.path().join(store::DEFAULT_DIR);
    std::fs::create_dir_all(&tickets).unwrap();
    std::fs::write(
        tickets.join(format!("{}slug.md", id.file_prefix())),
        render::ticket(title, Kind::Bug, HANDLE, DATE, None, "Something is wrong."),
    )
    .unwrap();

    id
}

/// Ids are generated, so tests follow the ones that were handed out.
fn ids(dir: &Utf8TempDir) -> Vec<TicketId> {
    store::list(&dir.path().join(store::DEFAULT_DIR))
        .unwrap()
        .into_iter()
        .filter_map(|entry| entry.ticket.as_ref().ok().map(|_| entry.id))
        .collect()
}

#[test]
fn create_reports_the_id_and_a_workspace_relative_path() {
    let dir = Utf8TempDir::new().unwrap();

    let out = create_ticket(&dir, "Tool call header misaligned");

    let id = ids(&dir)[0];
    let path = format!(
        "docs/ticket/{}tool-call-header-misaligned.md",
        id.file_prefix()
    );
    assert_eq!(out, format!("Created {id} at {path}"));
    assert!(dir.path().join(&path).exists());
}

#[test]
fn create_preview_renders_the_file_that_will_be_written() {
    let out = strip_ansi(preview_create(
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
            "> # Tool call header misaligned\n",
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

/// A preview leaves the board exactly as it found it: no file, and no id drawn
/// that the ticket it previews won't carry.
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

    assert!(out.starts_with("> # Bump the deny list\n"), "{out}");
    assert_eq!(ids(&dir).len(), 1, "the preview filed a ticket");

    create_ticket(&dir, "Bump the deny list");
    assert_eq!(ids(&dir).len(), 2);
}

#[test]
fn comment_preview_renders_the_block_under_the_ticket_it_lands_on() {
    let dir = Utf8TempDir::new().unwrap();
    let id = write_ticket(&dir, "Tool call header misaligned");
    // The reply target has to exist, or the preview rejects the call.
    run_tool(
        &dir,
        "ticket_comment",
        json!({ "id": FIXED_ID, "body": "Reproduced at 72 columns." }),
    )
    .unwrap();

    let out = strip_ansi(content(preview_comment(
        dir.path(),
        id,
        Some(1),
        "The wrap calculation is off.",
        STAMP,
    )));

    assert_eq!(
        out,
        concat!(
            "> # T-02wt0kx: Tool call header misaligned\n",
            "> \n",
            "> ────────────────────────────────────────────────────────────────────────────────\n",
            "> \n",
            "> - **From**: jp\n",
            "> - **Date**: 2026-08-05T14:03:11Z\n",
            "> - **Re**: #1\n",
            "> \n",
            "> The wrap calculation is off.\n",
        )
    );
}

/// A call that cannot land is answered before the user is asked about it: the
/// preview fails, and JP turns that into a tool failure ahead of the approval
/// prompt so the assistant can correct the id.
#[test]
fn comment_preview_rejects_a_missing_ticket() {
    let dir = Utf8TempDir::new().unwrap();

    let out = error_message(preview_comment(
        dir.path(),
        FIXED_ID.parse().unwrap(),
        None,
        "Reproduced at 72 columns.",
        STAMP,
    ));

    assert_eq!(out, "No T-02wt0kx.");
}

/// The preview knows the comment count, which the assistant cannot see, so a
/// reply to a comment that isn't there fails here rather than at the write.
#[test]
fn comment_preview_rejects_a_missing_reply_target() {
    let dir = Utf8TempDir::new().unwrap();
    let id = write_ticket(&dir, "Tool call header misaligned");

    let out = error_message(preview_comment(
        dir.path(),
        id,
        Some(3),
        "The wrap calculation is off.",
        STAMP,
    ));

    assert_eq!(out, "No comment #3 on T-02wt0kx.");
}

/// Two files claiming one id is a different problem from a missing ticket, and
/// points at a different fix.
#[test]
fn comment_preview_reports_a_duplicated_id() {
    let dir = Utf8TempDir::new().unwrap();
    let id = write_ticket(&dir, "Tool call header misaligned");
    std::fs::write(
        dir.path()
            .join(store::DEFAULT_DIR)
            .join(format!("{}other.md", id.file_prefix())),
        render::ticket("Other", Kind::Bug, HANDLE, DATE, None, "Something else."),
    )
    .unwrap();

    let error =
        preview_comment(dir.path(), id, None, "Reproduced at 72 columns.", STAMP).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("is claimed by more than one file: "),
        "{error}"
    );
}

/// A hand-edited ticket that lost a metadata field still takes a comment: the
/// append never reads the header, so the preview must not demand one either.
#[test]
fn comment_preview_tolerates_a_malformed_header() {
    let dir = Utf8TempDir::new().unwrap();
    let id: TicketId = FIXED_ID.parse().unwrap();
    let tickets = dir.path().join(store::DEFAULT_DIR);
    std::fs::create_dir_all(&tickets).unwrap();
    std::fs::write(
        tickets.join(format!("{}slug.md", id.file_prefix())),
        "# Tool call header misaligned\n\n- **Status**: Todo\n- **Kind**: Bug\n- **Date**: \
         2026-08-05\n\nSomething is wrong.\n",
    )
    .unwrap();

    let out = strip_ansi(content(preview_comment(
        dir.path(),
        id,
        None,
        "Reproduced at 72 columns.",
        STAMP,
    )));

    assert_eq!(
        out.lines().next(),
        Some("> # T-02wt0kx: Tool call header misaligned")
    );

    // The write the preview promised really does land.
    assert_eq!(
        content(run_tool(
            &dir,
            "ticket_comment",
            json!({ "id": FIXED_ID, "body": "Reproduced at 72 columns." })
        )),
        "Added T-02wt0kx#1"
    );
}

/// Arguments execution rejects are rejected by the preview too, so the
/// assistant is corrected before the call reaches the user.
#[test]
fn preview_rejects_the_arguments_execution_would_reject() {
    let dir = Utf8TempDir::new().unwrap();

    assert_eq!(
        error_message(preview_tool(
            &dir,
            "ticket_create",
            json!({ "kind": "chore", "title": "   " })
        )),
        "`title` must not be empty."
    );
    assert_eq!(
        error_message(preview_tool(
            &dir,
            "ticket_comment",
            json!({ "id": FIXED_ID, "body": "  \n " })
        )),
        "`body` must not be empty."
    );
}

/// Markdown in this repository is laid out by `comfort`, and a body arrives
/// from the model as one long line per paragraph.
#[test]
fn create_reflows_the_body_with_comfort() {
    let dir = Utf8TempDir::new().unwrap();
    run_tool(
        &dir,
        "ticket_create",
        json!({
            "kind": "bug",
            "title": "Wrapping is wrong",
            "body": "The first sentence. The second one, which the model wrote on the same line."
        }),
    )
    .unwrap();

    let id = ids(&dir)[0];
    let source = std::fs::read_to_string(dir.path().join(format!(
        "docs/ticket/{}wrapping-is-wrong.md",
        id.file_prefix()
    )))
    .unwrap();

    assert!(
        source.ends_with(
            "\nThe first sentence.\nThe second one, which the model wrote on the same line.\n"
        ),
        "{source}"
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

    let id = ids(&dir)[0];

    let first = content(run_tool(
        &dir,
        "ticket_comment",
        json!({
            "id": id.as_str(),
            "body": "Reproduced at 72 columns."
        }),
    ));
    let second = content(run_tool(
        &dir,
        "ticket_comment",
        json!({
            "id": id.to_string(),
            "re": 1,
            "body": "The wrap calculation is off."
        }),
    ));

    assert_eq!(first, format!("Added {id}#1"));
    assert_eq!(second, format!("Added {id}#2"));

    let shown = content(run_tool(
        &dir,
        "ticket_show",
        json!({ "id": id.to_string() }),
    ));
    assert!(shown.contains(&format!("### {id}#1 — jp at ")), "{shown}");
    assert!(
        shown.contains(&format!("### {id}#2 — jp at "))
            && shown.contains(&format!("replying to {id}#1")),
        "{shown}"
    );
}

/// A comment body is reflowed for the same reason a description is: the file it
/// lands in is one CI checks.
#[test]
fn comment_reflows_the_body_with_comfort() {
    let dir = Utf8TempDir::new().unwrap();
    create_ticket(&dir, "Wrapping is wrong");
    let id = ids(&dir)[0];

    run_tool(
        &dir,
        "ticket_comment",
        json!({
            "id": id.to_string(),
            "body": "The first sentence. The second one, which the model wrote on the same line."
        }),
    )
    .unwrap();

    let source = std::fs::read_to_string(
        dir.path()
            .join(format!("docs/ticket/{}wrapping-is-wrong.md", id.file_prefix())),
    )
    .unwrap();

    assert!(
        source.ends_with(
            "\nThe first sentence.\nThe second one, which the model wrote on the same line.\n"
        ),
        "{source}"
    );
}

#[test]
fn a_reply_to_a_missing_comment_is_reported_to_the_model() {
    let dir = Utf8TempDir::new().unwrap();
    create_ticket(&dir, "Tool call header misaligned");

    let id = ids(&dir)[0];

    let out = error_message(run_tool(
        &dir,
        "ticket_comment",
        json!({
            "id": id.to_string(),
            "re": 3,
            "body": "Reply."
        }),
    ));

    assert_eq!(out, format!("No {id}, or no comment #3 on it."));
}

#[test]
fn an_unusable_id_is_reported_to_the_model() {
    let dir = Utf8TempDir::new().unwrap();

    assert_eq!(
        error_message(run_tool(&dir, "ticket_close", json!({ "id": "nope" }))),
        "`nope` is not a ticket id (try T-02wt0kx)."
    );
    assert_eq!(
        error_message(run_tool(&dir, "ticket_close", json!({ "id": 7 }))),
        "`7` is not a ticket id."
    );
    assert_eq!(
        error_message(run_tool(&dir, "ticket_close", json!({ "id": "T-zzzzzzz" }))),
        "No T-zzzzzzz."
    );
}

#[test]
fn close_reports_the_transition_once() {
    let dir = Utf8TempDir::new().unwrap();
    create_ticket(&dir, "Tool call header misaligned");

    let id = ids(&dir)[0];

    assert_eq!(
        content(run_tool(
            &dir,
            "ticket_close",
            json!({ "id": id.to_string() })
        )),
        format!("{id}: Todo -> Done")
    );
    assert_eq!(
        content(run_tool(
            &dir,
            "ticket_close",
            json!({ "id": id.to_string() })
        )),
        format!("{id} was already Done.")
    );
}

#[test]
fn list_shows_one_line_per_ticket_and_filters() {
    let dir = Utf8TempDir::new().unwrap();
    create_ticket(&dir, "Still open");
    create_ticket(&dir, "Finished");
    let [open, finished] = ids(&dir)[..] else {
        panic!("expected two tickets")
    };
    run_tool(&dir, "ticket_close", json!({ "id": finished.to_string() })).unwrap();

    let all = content(run_tool(&dir, "ticket_list", json!({})));
    assert_eq!(
        all,
        format!(
            "{open} Todo         Bug      Still open\n{finished} Done         Bug      Finished\n"
        )
    );

    let todo = content(run_tool(&dir, "ticket_list", json!({ "status": "todo" })));
    assert_eq!(todo, format!("{open} Todo         Bug      Still open\n"));

    let chores = content(run_tool(&dir, "ticket_list", json!({ "kind": "chore" })));
    assert_eq!(chores, "No tickets match.\n");
}

#[test]
fn list_names_the_files_it_could_not_read() {
    let dir = Utf8TempDir::new().unwrap();
    create_ticket(&dir, "Readable");
    let id = ids(&dir)[0];
    // Sorts after the readable ticket, so listing order is stable.
    std::fs::write(
        dir.path().join("docs/ticket/zzzzzzz-mangled.md"),
        "no heading here\n",
    )
    .unwrap();

    let out = content(run_tool(&dir, "ticket_list", json!({})));

    assert!(out.starts_with(&format!("{id} Todo")), "{out}");
    assert!(
        out.contains("- docs/ticket/zzzzzzz-mangled.md: Ticket does not open with"),
        "{out}"
    );
}

#[test]
fn show_renders_metadata_and_the_description() {
    let dir = Utf8TempDir::new().unwrap();
    create_ticket(&dir, "Tool call header misaligned");

    let id = ids(&dir)[0];
    let out = content(run_tool(
        &dir,
        "ticket_show",
        json!({ "id": id.to_string() }),
    ));

    assert!(
        out.starts_with(&format!("# {id}: Tool call header misaligned\n")),
        "{out}"
    );
    assert!(
        out.contains(&format!(
            "- **Path**: docs/ticket/{}tool-call-header-misaligned.md",
            id.file_prefix()
        )),
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
        error_message(run_tool(&dir, "ticket_show", json!({ "id": "T-zzzzzzz" }))),
        "No T-zzzzzzz."
    );
}

#[test]
fn ids_are_read_from_every_accepted_spelling() {
    let expected: TicketId = "T-02wt0kx".parse().unwrap();

    assert_eq!(id_arg(&json!("T-02wt0kx")), Ok(expected));
    assert_eq!(id_arg(&json!("T02wt0kx")), Ok(expected));
    assert_eq!(id_arg(&json!("02wt0kx")), Ok(expected));

    // Ids carry letters, so a bare number can never be one.
    assert!(id_arg(&json!(42)).is_err());
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
