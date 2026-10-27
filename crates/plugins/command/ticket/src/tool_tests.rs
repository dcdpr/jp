use std::sync::LazyLock;

use camino_tempfile::Utf8TempDir;
use jp_plugin::message::{OutputFormat, PathsInfo, WorkspaceInfo};
use jp_tool::FsRule;
use serde_json::json;

use super::*;

const DATE: &str = "2026-08-05";
const STAMP: &str = "2026-08-05T14:03:11Z";

/// A ticket id the tests can name, since allocation draws unpredictable ones.
const FIXED_ID: &str = "T-02wt0kx";

/// The workspace a call runs in, as the host describes it in `init`.
fn init(root: &Utf8TempDir) -> InitMessage {
    InitMessage {
        version: jp_plugin::PROTOCOL_VERSION,
        workspace: WorkspaceInfo {
            root: root.path().to_path_buf(),
            storage: root.path().join(".jp"),
            id: "test".to_owned(),
        },
        paths: PathsInfo::default(),
        config: json!({}),
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

/// The plugin's defaults, with the assistant named `jp` so the expected
/// documents read like JP's own tickets, and a label vocabulary so label writes
/// have something to check against.
fn options() -> Options {
    let Value::Object(options) = json!({
        "assistant": "jp",
        "labels": {
            "client": { "description": "The client.", "values": ["cli", "macos"] },
            "package": {
                "description": "The crate.",
                "values": ["jp_cli", "jp_config"],
                "retired": ["jp_legacy"]
            }
        }
    }) else {
        unreachable!("options are an object");
    };

    Options::read(&options).unwrap()
}

/// [`options`], for helpers that borrow it for as long as the test runs.
static OPTIONS: LazyLock<Options> = LazyLock::new(options);

/// Run one call the way the host does: the action, the arguments, the answers
/// so far, and the tool's own options.
fn dispatch(
    dir: &Utf8TempDir,
    action: ToolAction,
    name: &str,
    arguments: Value,
    answers: Value,
    tool_options: Value,
) -> Outcome {
    let call = ToolCall {
        action,
        name: name.to_owned(),
        arguments: object(arguments),
        answers: object(answers),
        options: object(tool_options),
        access: None,
        conversation: "test".to_owned(),
    };

    run(&init(dir), &call, &options())
}

fn run_tool(dir: &Utf8TempDir, name: &str, args: Value) -> Outcome {
    dispatch(dir, ToolAction::Run, name, args, json!({}), json!({}))
}

/// Run a tool the way JP runs it again once a question has been answered.
fn run_tool_with_answers(dir: &Utf8TempDir, name: &str, args: Value, answers: Value) -> Outcome {
    dispatch(dir, ToolAction::Run, name, args, answers, json!({}))
}

/// Run a tool with the options its definition passes alongside the arguments.
fn run_tool_with_options(dir: &Utf8TempDir, name: &str, args: Value, options: Value) -> Outcome {
    dispatch(dir, ToolAction::Run, name, args, json!({}), options)
}

/// Ask a tool to describe a call, as JP does before asking for approval.
fn preview_tool(dir: &Utf8TempDir, name: &str, args: Value) -> Outcome {
    dispatch(
        dir,
        ToolAction::FormatArguments,
        name,
        args,
        json!({}),
        json!({}),
    )
}

/// Run a call carrying an access policy.
fn run_with_access(dir: &Utf8TempDir, name: &str, args: Value, access: &AccessPolicy) -> Outcome {
    let call = ToolCall {
        action: ToolAction::Run,
        name: name.to_owned(),
        arguments: object(args),
        answers: Map::new(),
        options: Map::new(),
        access: Some(serde_json::to_value(access).unwrap()),
        conversation: "test".to_owned(),
    };

    run(&init(dir), &call, &options())
}

/// The ticket directory of a test workspace, for calling helpers directly.
fn tickets(dir: &Utf8TempDir) -> Tickets<'_> {
    Tickets {
        root: dir.path(),
        dir: dir.path().join(store::DEFAULT_DIR),
        author: "jp",
        options: &OPTIONS,
    }
}

fn content(outcome: Outcome) -> String {
    match outcome {
        Outcome::Success { content } => content,
        other => panic!("expected success, got: {other:?}"),
    }
}

/// A preview as a terminal shows it, with the ANSI styling removed.
fn strip_ansi(rendered: String) -> String {
    let bytes = strip_ansi_escapes::strip(rendered);
    String::from_utf8(bytes).expect("valid utf-8 after stripping ANSI")
}

fn error_message(outcome: Outcome) -> String {
    match outcome {
        Outcome::Error { message, .. } => message,
        other => panic!("expected error, got: {other:?}"),
    }
}

fn question(outcome: Outcome) -> Question {
    match outcome {
        Outcome::NeedsInput { question } => question,
        other => panic!("expected a question, got: {other:?}"),
    }
}

fn create_ticket(dir: &Utf8TempDir, title: &str) -> String {
    content(run_tool(
        dir,
        "create",
        json!({
            "title": title,
            "body": "Something is wrong."
        }),
    ))
}

/// A ticket with everything but the title fixed.
fn draft<'a>(title: &'a str, labels: &'a Labels) -> NewTicket<'a> {
    NewTicket {
        title,
        authors: "jp",
        date: DATE,
        implements: None,
        labels,
        description: "Something is wrong.",
    }
}

/// File a ticket under [`FIXED_ID`], bypassing allocation so the test can
/// assert against the id it will read back.
fn write_ticket(dir: &Utf8TempDir, title: &str) -> TicketId {
    write_ticket_as(dir, FIXED_ID, title)
}

/// File a ticket under `id`, so a test can lay several out in a known order.
fn write_ticket_as(dir: &Utf8TempDir, id: &str, title: &str) -> TicketId {
    let id: TicketId = id.parse().unwrap();
    let tickets = dir.path().join(store::DEFAULT_DIR);
    fs::create_dir_all(&tickets).unwrap();
    fs::write(
        tickets.join(format!("{}slug.md", id.file_prefix())),
        render::ticket(&draft(title, &Labels::default())),
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

/// The ticket directory and the author come from the plugin's options, which
/// the host resolves for the query making the call.
#[test]
fn create_writes_where_the_options_say_and_as_whom() {
    let dir = Utf8TempDir::new().unwrap();
    let call = ToolCall {
        action: ToolAction::Run,
        name: "create".to_owned(),
        arguments: object(json!({ "title": "Bump the deny list" })),
        answers: Map::new(),
        options: Map::new(),
        access: None,
        conversation: "test".to_owned(),
    };
    let options = Options {
        dir: Utf8PathBuf::from("work/items"),
        assistant: "helper".to_owned(),
        ..Options::default()
    };

    let out = content(run(&init(&dir), &call, &options));

    let entries = store::list(&dir.path().join("work/items")).unwrap();
    let [entry] = entries.as_slice() else {
        panic!("expected one ticket, got {entries:?}");
    };
    assert_eq!(
        out,
        format!(
            "Created {} at work/items/{}",
            entry.id,
            entry.path.file_name().unwrap()
        )
    );
    let ticket = entry.ticket.as_ref().unwrap();
    assert_eq!(ticket.metadata.authors, "helper");
}

#[test]
fn create_preview_renders_the_file_that_will_be_written() {
    let labels = Labels::default();
    let mut new = draft("Tool call header misaligned", &labels);
    new.implements = Some("045");
    new.description = "The header renders one column left of the body.";

    let out = strip_ansi(preview_create(&new));

    // The preview is quoted, so the rail shows where the ticket ends and the
    // conversation resumes. Written line by line because the blank lines carry
    // a trailing space that an editor would trim out of a block literal.
    assert_eq!(
        out,
        concat!(
            "> # Tool call header misaligned\n",
            "> \n",
            "> - **Status**: Todo\n",
            "> - **Authors**: jp\n",
            "> - **Date**: 2026-08-05\n",
            "> - **Implements**: 045\n",
            "> \n",
            "> The header renders one column left of the body.\n",
        )
    );
}

/// The preview shows the document that will be written, so a title still
/// waiting to be shortened has nothing to show yet: it asks the same question
/// the run would, and JP comes back once the answer is in.
#[test]
fn create_preview_asks_before_drawing_a_title_that_will_change() {
    let dir = Utf8TempDir::new().unwrap();

    let question = question(preview_tool(
        &dir,
        "create",
        json!({
            "title": "Make provider fixtures deterministic and sanitize recorded model output",
            "body": "Something is wrong."
        }),
    ));

    assert_eq!(question.id, "shorter_title");
}

/// The answered preview draws the ticket under the title that will be filed,
/// not the one the assistant first asked for.
#[test]
fn create_preview_draws_the_answered_title() {
    let dir = Utf8TempDir::new().unwrap();

    let out = strip_ansi(content(dispatch(
        &dir,
        ToolAction::FormatArguments,
        "create",
        json!({
            "title": "Make provider fixtures deterministic and sanitize recorded model output",
            "body": "Something is wrong."
        }),
        json!({ "shorter_title": "Sanitize recorded model output in fixtures" }),
        json!({}),
    )));

    assert!(
        out.starts_with("> # Sanitize recorded model output in fixtures\n"),
        "{out}"
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
        "create",
        json!({
            "title": "Bump the deny list"
        }),
    )));

    assert!(out.starts_with("> # Bump the deny list\n"), "{out}");
    assert_eq!(ids(&dir).len(), 1, "the preview filed a ticket");

    create_ticket(&dir, "Bump the deny list");
    assert_eq!(ids(&dir).len(), 2);
}

/// Only `create` and `comment` describe a call themselves; a definition that
/// styles another tool `parameters = "tool"` is told what to do instead.
#[test]
fn a_tool_without_a_preview_says_so() {
    let dir = Utf8TempDir::new().unwrap();
    let id = write_ticket(&dir, "Tool call header misaligned");

    let message = error_message(preview_tool(
        &dir,
        "edit",
        json!({ "id": FIXED_ID, "status": "done" }),
    ));

    assert_eq!(
        message,
        "`edit` has no preview of its own; style its parameters as `function_call` or `json` \
         rather than `tool`."
    );
    let source = fs::read_to_string(store::locate_ticket(&tickets(&dir).dir, id).unwrap()).unwrap();
    assert!(source.contains("- **Status**: Todo"), "{source}");
}

#[test]
fn comment_preview_renders_the_block_under_the_ticket_it_lands_on() {
    let dir = Utf8TempDir::new().unwrap();
    let id = write_ticket(&dir, "Tool call header misaligned");
    // The reply target has to exist, or the preview rejects the call.
    content(run_tool(
        &dir,
        "comment",
        json!({ "id": FIXED_ID, "body": "Reproduced at 72 columns." }),
    ));

    let out = strip_ansi(content(
        preview_comment(
            &tickets(&dir),
            id,
            Some(1),
            "The wrap calculation is off.",
            STAMP,
        )
        .unwrap(),
    ));

    assert_eq!(
        out,
        concat!(
            "> # T-02wt0kx: Tool call header misaligned\n",
            "> \n",
            "> ──────────────────────────────────────────────────────────────────────────────\n",
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

    let out = error_message(
        preview_comment(
            &tickets(&dir),
            FIXED_ID.parse().unwrap(),
            None,
            "Reproduced at 72 columns.",
            STAMP,
        )
        .unwrap(),
    );

    assert_eq!(out, "No T-02wt0kx.");
}

/// The preview knows the comment count, which the assistant cannot see, so a
/// reply to a comment that isn't there fails here rather than at the write.
#[test]
fn comment_preview_rejects_a_missing_reply_target() {
    let dir = Utf8TempDir::new().unwrap();
    let id = write_ticket(&dir, "Tool call header misaligned");

    let out = error_message(
        preview_comment(
            &tickets(&dir),
            id,
            Some(3),
            "The wrap calculation is off.",
            STAMP,
        )
        .unwrap(),
    );

    assert_eq!(out, "No comment #3 on T-02wt0kx.");
}

/// Two files claiming one id is a different problem from a missing ticket, and
/// points at a different fix.
#[test]
fn comment_preview_reports_a_duplicated_id() {
    let dir = Utf8TempDir::new().unwrap();
    let id = write_ticket(&dir, "Tool call header misaligned");
    fs::write(
        dir.path()
            .join(store::DEFAULT_DIR)
            .join(format!("{}other.md", id.file_prefix())),
        render::ticket(&draft("Other", &Labels::default())),
    )
    .unwrap();

    let error =
        preview_comment(&tickets(&dir), id, None, "Reproduced at 72 columns.", STAMP).unwrap_err();

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
    let dir_path = dir.path().join(store::DEFAULT_DIR);
    fs::create_dir_all(&dir_path).unwrap();
    fs::write(
        dir_path.join(format!("{}slug.md", id.file_prefix())),
        "# Tool call header misaligned\n\n- **Status**: Todo\n- **Date**: 2026-08-05\n\nSomething \
         is wrong.\n",
    )
    .unwrap();

    let out = strip_ansi(content(
        preview_comment(&tickets(&dir), id, None, "Reproduced at 72 columns.", STAMP).unwrap(),
    ));

    assert_eq!(
        out.lines().next(),
        Some("> # T-02wt0kx: Tool call header misaligned")
    );

    // The write the preview promised really does land.
    assert_eq!(
        content(run_tool(
            &dir,
            "comment",
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
        error_message(preview_tool(&dir, "create", json!({ "title": "   " }))),
        "`title` must not be empty."
    );
    assert_eq!(
        error_message(preview_tool(
            &dir,
            "comment",
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
    content(run_tool(
        &dir,
        "create",
        json!({
            "title": "Wrapping is wrong",
            "body": "The first sentence. The second one, which the model wrote on the same line."
        }),
    ));

    let id = ids(&dir)[0];
    let source = fs::read_to_string(dir.path().join(format!(
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
fn create_rejects_an_empty_title() {
    let dir = Utf8TempDir::new().unwrap();

    let out = error_message(run_tool(
        &dir,
        "create",
        json!({
            "title": "   "
        }),
    ));

    assert_eq!(out, "`title` must not be empty.");
}

/// The host validates arguments against the schema, but a definition can mark
/// one optional by mistake; the tool still names what it needs.
#[test]
fn a_missing_argument_is_named() {
    let dir = Utf8TempDir::new().unwrap();

    assert_eq!(
        error_message(run_tool(&dir, "create", json!({}))),
        "Missing argument `title`."
    );
    assert_eq!(
        error_message(run_tool(&dir, "show", json!({}))),
        "Missing argument `id`."
    );
}

/// A title past the slug budget loses its tail in the filename, so it is put
/// back as a question instead of being filed under a truncated name.
#[test]
fn create_asks_for_a_shorter_title() {
    let dir = Utf8TempDir::new().unwrap();

    let question = question(run_tool(
        &dir,
        "create",
        json!({
            "title": "Make provider fixtures deterministic and sanitize recorded model output",
            "body": "Something is wrong."
        }),
    ));

    assert_eq!(question.id, "shorter_title");
    assert_eq!(question.text, "Shorter title, at most 60 characters?");
    // The title the formatter saw, which is the edited one when the user
    // changed the arguments at the approval prompt: the assistant's history
    // only has the title it first sent.
    assert_eq!(
        question.pre_amble.as_deref(),
        Some(
            "The title is 71 characters: Make provider fixtures deterministic and sanitize \
             recorded model output"
        )
    );
    // The title as given, so a prompt opens with it ready to edit rather
    // than empty.
    assert_eq!(
        question.default,
        Some(json!(
            "Make provider fixtures deterministic and sanitize recorded model output"
        ))
    );
    assert!(
        ids(&dir).is_empty(),
        "the ticket was filed before the answer"
    );
}

/// The answer replaces the title, and the rest of the call is the one that was
/// already made: the body does not travel a second time.
#[test]
fn a_shorter_title_answer_files_the_ticket() {
    let dir = Utf8TempDir::new().unwrap();

    let out = content(run_tool_with_answers(
        &dir,
        "create",
        json!({
            "title": "Make provider fixtures deterministic and sanitize recorded model output",
            "body": "Something is wrong."
        }),
        json!({ "shorter_title": "Sanitize recorded model output in fixtures" }),
    ));

    assert!(
        out.ends_with("-sanitize-recorded-model-output-in-fixtures.md"),
        "{out}"
    );

    let id = ids(&dir).pop().expect("one ticket");
    let ticket = content(run_tool(
        &dir,
        "show",
        json!({ "id": id.to_string(), "description": true }),
    ));
    assert!(
        ticket.starts_with(&format!(
            "```markdown\n# {id}: Sanitize recorded model output in fixtures\n"
        )),
        "{ticket}"
    );
    assert!(ticket.contains("Something is wrong."), "{ticket}");
}

/// The question is asked once.
/// A replacement that doesn't fit either ends the call, rather than being asked
/// again under an id that already has an answer.
#[test]
fn a_shorter_title_answer_that_still_does_not_fit_ends_the_call() {
    let dir = Utf8TempDir::new().unwrap();

    let out = error_message(run_tool_with_answers(
        &dir,
        "create",
        json!({ "title": "A title that does not fit" }),
        json!({
            "shorter_title":
                "Make provider fixtures deterministic and sanitize recorded model output"
        }),
    ));

    assert_eq!(
        out,
        "The shorter title is 71 characters, and a ticket takes at most 60: Make provider \
         fixtures deterministic and sanitize recorded model output"
    );
    assert!(ids(&dir).is_empty(), "a refused title filed a ticket");
}

#[test]
fn an_empty_shorter_title_answer_ends_the_call() {
    let dir = Utf8TempDir::new().unwrap();

    let out = error_message(run_tool_with_answers(
        &dir,
        "create",
        json!({ "title": "A title that does not fit" }),
        json!({ "shorter_title": "  " }),
    ));

    assert_eq!(out, "The shorter title was empty.");
}

/// The boundary is inclusive: a title that fits exactly is filed under a slug
/// that carries all of it.
#[test]
fn create_accepts_a_title_that_fills_the_slug() {
    let dir = Utf8TempDir::new().unwrap();

    let out = create_ticket(
        &dir,
        "Sanitize recorded model output in the provider test fixtures",
    );

    assert!(
        out.ends_with("-sanitize-recorded-model-output-in-the-provider-test-fixtures.md"),
        "{out}"
    );
}

#[test]
fn the_title_limit_is_configurable() {
    let dir = Utf8TempDir::new().unwrap();

    let question = question(run_tool_with_options(
        &dir,
        "create",
        json!({ "title": "Bump the deny list" }),
        json!({ "max_title_length": 10 }),
    ));

    assert_eq!(question.text, "Shorter title, at most 10 characters?");
}

#[test]
fn comments_are_attributed_to_the_assistant_and_numbered() {
    let dir = Utf8TempDir::new().unwrap();
    create_ticket(&dir, "Tool call header misaligned");

    let id = ids(&dir)[0];

    let first = content(run_tool(
        &dir,
        "comment",
        json!({
            "id": id.as_str(),
            "body": "Reproduced at 72 columns."
        }),
    ));
    let second = content(run_tool(
        &dir,
        "comment",
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
        "show",
        json!({ "id": id.to_string(), "comments": true }),
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

    content(run_tool(
        &dir,
        "comment",
        json!({
            "id": id.to_string(),
            "body": "The first sentence. The second one, which the model wrote on the same line."
        }),
    ));

    let source = fs::read_to_string(dir.path().join(format!(
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
fn a_reply_to_a_missing_comment_is_reported_to_the_model() {
    let dir = Utf8TempDir::new().unwrap();
    create_ticket(&dir, "Tool call header misaligned");

    let id = ids(&dir)[0];

    let out = error_message(run_tool(
        &dir,
        "comment",
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
        error_message(run_tool(
            &dir,
            "edit",
            json!({ "id": "nope", "status": "done" })
        )),
        "`nope` is not a ticket id (try T-02wt0kx)."
    );
    assert_eq!(
        error_message(run_tool(&dir, "edit", json!({ "id": 7, "status": "done" }))),
        "`7` is not a ticket id."
    );
    assert_eq!(
        error_message(run_tool(
            &dir,
            "edit",
            json!({ "id": "T-zzzzzzz", "status": "done" })
        )),
        "No T-zzzzzzz."
    );
}

/// Moving a ticket between columns, closing it included, is one more thing
/// `edit` changes.
#[test]
fn edit_moves_a_ticket_between_columns() {
    let dir = Utf8TempDir::new().unwrap();
    create_ticket(&dir, "Tool call header misaligned");
    let id = ids(&dir)[0];

    assert_eq!(
        content(run_tool(
            &dir,
            "edit",
            json!({ "id": id.to_string(), "status": "in progress" })
        )),
        format!("Edited {id}:\n- status: Todo -> In Progress\n")
    );
    assert_eq!(
        content(run_tool(
            &dir,
            "edit",
            json!({ "id": id.to_string(), "status": "done" })
        )),
        format!("Edited {id}:\n- status: In Progress -> Done\n")
    );
    assert_eq!(
        content(run_tool(
            &dir,
            "edit",
            json!({ "id": id.to_string(), "status": "done" })
        )),
        format!("Edited {id}:\n- status: already Done\n")
    );
    assert_eq!(
        error_message(run_tool(
            &dir,
            "edit",
            json!({ "id": id.to_string(), "status": "blocked" })
        )),
        "`status` must be one of: todo, in progress, done."
    );
}

/// Several changes land in one call, and the result names each.
#[test]
fn edit_changes_several_things_at_once() {
    let dir = Utf8TempDir::new().unwrap();
    let id = write_ticket(&dir, "Wrapping is wrong");
    let prefix = id.file_prefix();

    let out = content(run_tool(
        &dir,
        "edit",
        json!({
            "id": FIXED_ID,
            "title": "The wrap calculation is off",
            "description": "Off by one at 80 columns.",
            "status": "in progress",
            "labels": ["client=cli"]
        }),
    ));

    assert_eq!(
        out,
        format!(
            "Edited {id}:\n- labels: client=cli\n- status: Todo -> In Progress\n- description \
             replaced\n- title changed, moved docs/ticket/{prefix}slug.md -> \
             docs/ticket/{prefix}the-wrap-calculation-is-off.md\n"
        )
    );
    let entries = store::list(&tickets(&dir).dir).unwrap();
    let ticket = entries[0].ticket.as_ref().unwrap();
    assert_eq!(ticket.title, "The wrap calculation is off");
    assert_eq!(ticket.description, "Off by one at 80 columns.");
    assert_eq!(ticket.metadata.status, Status::InProgress);
    assert_eq!(ticket.metadata.labels.to_tokens(), ["client=cli"]);
}

/// A call that changes nothing is a call that forgot something.
#[test]
fn edit_needs_something_to_change() {
    let dir = Utf8TempDir::new().unwrap();
    write_ticket(&dir, "Wrapping is wrong");

    assert_eq!(
        error_message(run_tool(&dir, "edit", json!({ "id": FIXED_ID }))),
        "Name what to change: `title`, `description`, `status`, or `labels`."
    );
    assert_eq!(
        error_message(run_tool(
            &dir,
            "edit",
            json!({ "id": FIXED_ID, "labels": null })
        )),
        "Name what to change: `title`, `description`, `status`, or `labels`."
    );
}

/// A refused label stops the whole call, so nothing else it asked for lands.
#[test]
fn a_refused_label_changes_nothing() {
    let dir = Utf8TempDir::new().unwrap();
    let id = write_ticket(&dir, "Wrapping is wrong");
    let path = store::locate_ticket(&tickets(&dir).dir, id).unwrap();
    let before = fs::read_to_string(&path).unwrap();

    let message = error_message(run_tool(
        &dir,
        "edit",
        json!({ "id": FIXED_ID, "status": "done", "labels": ["client=clii"] }),
    ));

    assert!(
        message.starts_with("`client=clii` is not a known label."),
        "{message}"
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), before);
}

#[test]
fn edit_replaces_the_description_and_keeps_the_rest() {
    let dir = Utf8TempDir::new().unwrap();
    create_ticket(&dir, "Wrapping is wrong");
    let id = ids(&dir)[0];
    content(run_tool(
        &dir,
        "comment",
        json!({ "id": id.to_string(), "body": "Reproduced at 72 columns." }),
    ));

    let out = content(run_tool(
        &dir,
        "edit",
        json!({
            "id": id.to_string(),
            "description": "The first sentence. The second one, which the model wrote on the same line."
        }),
    ));

    assert_eq!(out, format!("Edited {id}:\n- description replaced\n"));
    let entries = store::list(&tickets(&dir).dir).unwrap();
    let ticket = entries[0].ticket.as_ref().unwrap();
    assert_eq!(ticket.title, "Wrapping is wrong");
    assert_eq!(
        ticket.description,
        "The first sentence.\nThe second one, which the model wrote on the same line."
    );
    assert_eq!(ticket.comments.len(), 1);
}

/// The write replaces the old description outright, so an empty one is more
/// likely a slip than a request.
#[test]
fn edit_refuses_an_empty_description() {
    let dir = Utf8TempDir::new().unwrap();
    create_ticket(&dir, "Wrapping is wrong");
    let id = ids(&dir)[0];

    assert_eq!(
        error_message(run_tool(
            &dir,
            "edit",
            json!({ "id": id.to_string(), "description": "  " })
        )),
        "`description` must not be empty."
    );
}

/// A new title reports both paths: the assistant is holding the old one, and
/// the file it names is gone.
#[test]
fn a_new_title_moves_the_file_and_reports_both_paths() {
    let dir = Utf8TempDir::new().unwrap();
    create_ticket(&dir, "Wrapping is wrong");
    let id = ids(&dir)[0];
    let prefix = id.file_prefix();

    let out = content(run_tool(
        &dir,
        "edit",
        json!({ "id": id.to_string(), "title": "The wrap calculation is off" }),
    ));

    assert_eq!(
        out,
        format!(
            "Edited {id}:\n- title changed, moved docs/ticket/{prefix}wrapping-is-wrong.md -> \
             docs/ticket/{prefix}the-wrap-calculation-is-off.md\n"
        )
    );
    assert!(
        !dir.path()
            .join(format!("docs/ticket/{prefix}wrapping-is-wrong.md"))
            .exists()
    );

    let shown = content(run_tool(
        &dir,
        "show",
        json!({ "id": id.to_string(), "description": true }),
    ));
    assert!(
        shown.contains(&format!("# {id}: The wrap calculation is off")),
        "{shown}"
    );
    assert!(shown.contains("Something is wrong."), "{shown}");
}

/// A retitle the slug does not notice still says where the ticket is, without
/// claiming a move that did not happen.
#[test]
fn a_title_within_one_slug_reports_the_unchanged_path() {
    let dir = Utf8TempDir::new().unwrap();
    create_ticket(&dir, "Wrapping is wrong");
    let id = ids(&dir)[0];

    let out = content(run_tool(
        &dir,
        "edit",
        json!({ "id": id.to_string(), "title": "Wrapping is wrong!" }),
    ));

    assert_eq!(
        out,
        format!(
            "Edited {id}:\n- title changed, still at docs/ticket/{}wrapping-is-wrong.md\n",
            id.file_prefix()
        )
    );
}

#[test]
fn edit_rejects_an_empty_title() {
    let dir = Utf8TempDir::new().unwrap();
    create_ticket(&dir, "Wrapping is wrong");
    let id = ids(&dir)[0];

    assert_eq!(
        error_message(run_tool(
            &dir,
            "edit",
            json!({ "id": id.to_string(), "title": "   " })
        )),
        "`title` must not be empty."
    );
}

/// Retitling moves the file, so only an edit that names a title needs the
/// policy to let it create and delete files there.
#[test]
fn only_a_new_title_needs_to_move_files() {
    let dir = Utf8TempDir::new().unwrap();
    write_ticket(&dir, "Wrapping is wrong");
    let update_only = AccessPolicy {
        fs: vec![FsRule::new("").with_read(true).with_update(true)],
        ..AccessPolicy::default()
    };

    let message = error_message(run_with_access(
        &dir,
        "edit",
        json!({ "id": FIXED_ID, "title": "Something else" }),
        &update_only,
    ));
    assert!(
        message.starts_with("`edit` can't reach the ticket directory: access denied: create on"),
        "{message}"
    );

    let out = content(run_with_access(
        &dir,
        "edit",
        json!({ "id": FIXED_ID, "status": "done" }),
        &update_only,
    ));
    assert_eq!(out, format!("Edited {FIXED_ID}:\n- status: Todo -> Done\n"));
}

#[test]
fn list_shows_one_line_per_ticket_and_filters() {
    let dir = Utf8TempDir::new().unwrap();
    create_ticket(&dir, "Still open");
    create_ticket(&dir, "Finished");
    let [open, finished] = ids(&dir)[..] else {
        panic!("expected two tickets")
    };
    content(run_tool(
        &dir,
        "edit",
        json!({ "id": finished.to_string(), "status": "done" }),
    ));

    let all = content(run_tool(&dir, "list", json!({})));
    assert_eq!(
        all,
        format!("{open} Todo         Still open\n{finished} Done         Finished\n")
    );

    let todo = content(run_tool(&dir, "list", json!({ "status": "todo" })));
    assert_eq!(todo, format!("{open} Todo         Still open\n"));
}

/// The first Todo line is the next thing to work on: each column comes in the
/// order the board ranks it, Todo first, rather than by id.
#[test]
fn list_follows_the_board_order() {
    let dir = Utf8TempDir::new().unwrap();
    let first = write_ticket_as(&dir, "T-02wt0k1", "Filed first");
    let second = write_ticket_as(&dir, "T-02wt0k2", "Filed second");
    let started = write_ticket_as(&dir, "T-02wt0k3", "Started");
    let tickets = dir.path().join(store::DEFAULT_DIR);
    store::set_status(&tickets, started, Status::InProgress).unwrap();
    board::rank(&tickets, second, board::Place::Before, first).unwrap();

    let out = content(run_tool(&dir, "list", json!({})));

    assert_eq!(
        out,
        "T-02wt0k2 Todo         Filed second\nT-02wt0k1 Todo         Filed first\nT-02wt0k3 In \
         Progress  Started\n"
    );
}

/// A query matches the title, the description, or a comment, whatever its case,
/// and leaves out a ticket that mentions it nowhere.
#[test]
fn list_finds_a_query_in_the_title_description_and_comments() {
    let dir = Utf8TempDir::new().unwrap();
    write_ticket_as(&dir, "T-02wt0k1", "Wrap is off by one");
    write_ticket_as(&dir, "T-02wt0k2", "Header misaligned");
    write_ticket_as(&dir, "T-02wt0k3", "Colour is wrong");
    write_ticket_as(&dir, "T-02wt0k4", "Unrelated");
    content(run_tool(
        &dir,
        "edit",
        json!({ "id": "T-02wt0k2", "description": "The wrap width ignores the gutter." }),
    ));
    content(run_tool(
        &dir,
        "comment",
        json!({ "id": "T-02wt0k3", "body": "Same WRAP issue at 72 columns." }),
    ));

    let out = content(run_tool(&dir, "list", json!({ "query": "Wrap" })));

    assert_eq!(
        out,
        "T-02wt0k1 Todo         Wrap is off by one\nT-02wt0k2 Todo         Header \
         misaligned\nT-02wt0k3 Todo         Colour is wrong (1 comment)\n"
    );
}

/// A long listing comes a page at a time, and each page but the last says which
/// one to ask for next.
#[test]
fn list_pages_through_a_long_listing() {
    let dir = Utf8TempDir::new().unwrap();
    write_ticket_as(&dir, "T-02wt0k1", "First");
    write_ticket_as(&dir, "T-02wt0k2", "Second");
    write_ticket_as(&dir, "T-02wt0k3", "Third");
    let page = |page: usize| {
        run_tool_with_options(
            &dir,
            "list",
            json!({ "page": page }),
            json!({ "page_size": 2 }),
        )
    };

    assert_eq!(
        content(page(1)),
        "T-02wt0k1 Todo         First\nT-02wt0k2 Todo         Second\n\nPage 1 of 2 (3 tickets). \
         Pass `page: 2` for more.\n"
    );
    assert_eq!(
        content(page(2)),
        "T-02wt0k3 Todo         Third\n\nPage 2 of 2 (3 tickets).\n"
    );
    assert_eq!(error_message(page(3)), "Page 3 is past the last page, 2.");
}

#[test]
fn list_pages_count_from_one() {
    let dir = Utf8TempDir::new().unwrap();

    assert_eq!(
        error_message(run_tool(&dir, "list", json!({ "page": 0 }))),
        "`page` counts from 1."
    );
}

/// An order file that can't be read costs the order, not the listing.
#[test]
fn an_unreadable_board_order_lists_each_column_by_id() {
    let dir = Utf8TempDir::new().unwrap();
    write_ticket_as(&dir, "T-02wt0k1", "Filed first");
    write_ticket_as(&dir, "T-02wt0k2", "Filed second");
    fs::write(dir.path().join("docs/ticket/.board.json"), "not an order").unwrap();

    let out = content(run_tool(&dir, "list", json!({})));

    assert!(
        out.starts_with(
            "T-02wt0k1 Todo         Filed first\nT-02wt0k2 Todo         Filed second\n\nCould not \
             read:\n- docs/ticket/.board.json: "
        ),
        "{out}"
    );
    assert!(out.ends_with(" (columns are listed by id)\n"), "{out}");
}

#[test]
fn list_names_the_files_it_could_not_read() {
    let dir = Utf8TempDir::new().unwrap();
    create_ticket(&dir, "Readable");
    let id = ids(&dir)[0];
    // Sorts after the readable ticket, so listing order is stable.
    fs::write(
        dir.path().join("docs/ticket/zzzzzzz-mangled.md"),
        "no heading here\n",
    )
    .unwrap();

    let out = content(run_tool(&dir, "list", json!({})));

    assert!(out.starts_with(&format!("{id} Todo")), "{out}");
    assert!(
        out.contains("- docs/ticket/zzzzzzz-mangled.md: Ticket does not open with"),
        "{out}"
    );
}

/// By default a ticket is its metadata.
/// What was left out is named at the end with the argument that shows it, so
/// the assistant knows there is more without having to ask.
///
/// Fenced as markdown: the terminal renderer keys syntax highlighting off the
/// leading fence.
#[test]
fn show_prints_the_metadata_and_names_what_it_left_out() {
    let dir = Utf8TempDir::new().unwrap();
    write_ticket(&dir, "Tool call header misaligned");
    for body in ["Reproduced at 72 columns.", "Not at 80."] {
        content(run_tool(
            &dir,
            "comment",
            json!({ "id": FIXED_ID, "body": body }),
        ));
    }

    let out = content(run_tool(&dir, "show", json!({ "id": FIXED_ID })));

    assert_eq!(
        out,
        r"```markdown
# T-02wt0kx: Tool call header misaligned

- **Path**: docs/ticket/02wt0kx-slug.md
- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-08-05

Not shown: the description (`description: true`) and 2 comments (`comments: true`).
```"
    );
}

/// Asked for everything on a ticket nobody has commented on, `show` says there
/// is no discussion rather than leaving the reader to wonder.
#[test]
fn show_prints_everything_asked_for() {
    let dir = Utf8TempDir::new().unwrap();
    write_ticket(&dir, "Tool call header misaligned");

    let out = content(run_tool(
        &dir,
        "show",
        json!({ "id": FIXED_ID, "description": true, "comments": true }),
    ));

    assert_eq!(
        out,
        r"```markdown
# T-02wt0kx: Tool call header misaligned

- **Path**: docs/ticket/02wt0kx-slug.md
- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-08-05

Something is wrong.

No comments yet.
```"
    );
}

#[test]
fn show_adds_the_comments_when_asked() {
    let dir = Utf8TempDir::new().unwrap();
    write_ticket(&dir, "Tool call header misaligned");
    content(run_tool(
        &dir,
        "comment",
        json!({ "id": FIXED_ID, "body": "Reproduced at 72 columns." }),
    ));

    let out = content(run_tool(
        &dir,
        "show",
        json!({ "id": FIXED_ID, "comments": true }),
    ));

    assert!(
        out.contains("\n## Comments (1)\n\n### T-02wt0kx#1 — jp at "),
        "{out}"
    );
    assert!(!out.contains("Something is wrong."), "{out}");
    assert!(
        out.ends_with(
            "Reproduced at 72 columns.\n\nNot shown: the description (`description: true`).\n```"
        ),
        "{out}"
    );
}

#[test]
fn show_reports_a_missing_ticket() {
    let dir = Utf8TempDir::new().unwrap();

    assert_eq!(
        error_message(run_tool(&dir, "show", json!({ "id": "T-zzzzzzz" }))),
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

/// A definition whose `source` names a tool the plugin doesn't serve is a
/// configuration mistake; the outcome lists what it does serve.
#[test]
fn an_unknown_tool_names_the_ones_served() {
    let dir = Utf8TempDir::new().unwrap();

    assert_eq!(
        error_message(run_tool(&dir, "frobnicate", json!({}))),
        "`jp ticket` serves no tool named `frobnicate`. It serves: comment, create, edit, list, \
         show."
    );
}

/// Every name in `TOOLS` is one the plugin answers, and every one of them has
/// an access requirement.
#[test]
fn every_served_tool_is_dispatched() {
    let dir = Utf8TempDir::new().unwrap();

    for name in TOOLS {
        let call = ToolCall {
            action: ToolAction::Run,
            name: (*name).to_owned(),
            arguments: Map::new(),
            answers: Map::new(),
            options: Map::new(),
            access: None,
            conversation: "test".to_owned(),
        };
        assert!(capabilities(&call).is_some(), "{name} has no capabilities");

        if let Outcome::Error { message, .. } = run_tool(&dir, name, json!({})) {
            assert!(
                !message.contains("serves no tool named"),
                "{name} is listed but not dispatched"
            );
        }
    }
}

#[test]
fn create_writes_labels_and_list_shows_them() {
    let dir = Utf8TempDir::new().unwrap();

    content(run_tool(
        &dir,
        "create",
        json!({
            "title": "Flag parsing drops the last value",
            "labels": ["package=jp_config", "client=cli"]
        }),
    ));

    let id = ids(&dir)[0];
    let listed = content(run_tool(&dir, "list", json!({})));

    assert_eq!(
        listed,
        format!(
            "{id} Todo         Flag parsing drops the last value [client=cli, package=jp_config]\n"
        )
    );
}

#[test]
fn list_narrows_to_tickets_carrying_every_label() {
    let dir = Utf8TempDir::new().unwrap();

    for (title, labels) in [
        ("Both", json!(["client=cli", "package=jp_cli"])),
        ("One", json!(["client=cli"])),
        ("None", json!([])),
    ] {
        content(run_tool(
            &dir,
            "create",
            json!({ "title": title, "labels": labels }),
        ));
    }

    let listed = content(run_tool(
        &dir,
        "list",
        json!({ "labels": ["package=jp_cli", "client=cli"] }),
    ));

    assert_eq!(listed.lines().count(), 1, "{listed}");
    assert!(
        listed.contains("Both [client=cli, package=jp_cli]"),
        "{listed}"
    );
}

/// The whole set is written, so a retried call lands the same way twice and a
/// label left off the call is a label removed.
#[test]
fn labels_replace_the_whole_set() {
    let dir = Utf8TempDir::new().unwrap();
    let id = write_ticket(&dir, "Tool call header misaligned");

    let out = content(run_tool(
        &dir,
        "edit",
        json!({ "id": FIXED_ID, "labels": ["package=jp_config", "client=cli"] }),
    ));
    assert_eq!(
        out,
        format!("Edited {id}:\n- labels: client=cli, package=jp_config\n")
    );

    let out = content(run_tool(
        &dir,
        "edit",
        json!({ "id": FIXED_ID, "labels": ["client=cli"] }),
    ));
    assert_eq!(out, format!("Edited {id}:\n- labels: client=cli\n"));

    let out = content(run_tool(
        &dir,
        "edit",
        json!({ "id": FIXED_ID, "labels": [] }),
    ));
    assert_eq!(out, format!("Edited {id}:\n- labels cleared\n"));

    let shown = content(run_tool(&dir, "show", json!({ "id": FIXED_ID })));
    assert!(!shown.contains("Labels"), "{shown}");
}

/// A null `labels` is the shape a model reaches for when it means "leave them
/// alone", so it leaves them alone; `[]` is what clears them.
#[test]
fn a_null_label_list_leaves_the_labels_alone() {
    let dir = Utf8TempDir::new().unwrap();
    let id = write_ticket(&dir, "Tool call header misaligned");

    content(run_tool(
        &dir,
        "edit",
        json!({ "id": FIXED_ID, "labels": ["client=cli", "package=jp_config"] }),
    ));

    let out = content(run_tool(
        &dir,
        "edit",
        json!({ "id": FIXED_ID, "labels": null, "status": "in progress" }),
    ));

    assert_eq!(
        out,
        format!("Edited {id}:\n- status: Todo -> In Progress\n")
    );
    let entries = store::list(&tickets(&dir).dir).unwrap();
    let ticket = entries[0].ticket.as_ref().unwrap();
    assert_eq!(ticket.metadata.labels.to_tokens(), [
        "client=cli",
        "package=jp_config"
    ]);
}

/// A typo names the vocabulary, so the assistant can fix the call rather than
/// landing a label nothing groups by.
#[test]
fn an_unknown_label_is_refused_with_the_known_set() {
    let dir = Utf8TempDir::new().unwrap();

    let message = error_message(run_tool(
        &dir,
        "create",
        json!({ "title": "Typo", "labels": ["client=clii"] }),
    ));

    assert_eq!(
        message,
        "`client=clii` is not a known label. Labels you can add: client=cli, client=macos, \
         package=jp_cli, package=jp_config."
    );
    assert!(ids(&dir).is_empty(), "a ticket was filed anyway");
}

/// The case the active/retired split exists for: an old ticket carries a label
/// the board has since retired, and adding a new one must not force the retired
/// one off first.
#[test]
fn a_retired_label_already_on_a_ticket_can_be_kept() {
    let dir = Utf8TempDir::new().unwrap();

    // Written by hand: `jp_legacy` can no longer be applied through the tool,
    // which is exactly the situation an old ticket is in.
    let id = write_ticket(&dir, "Old ticket");
    let path = store::locate_ticket(&tickets(&dir).dir, id).unwrap();
    let source = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        render::set_repeated_metadata(&source, "Label", &["package=jp_legacy".to_owned()]).unwrap(),
    )
    .unwrap();

    let out = content(run_tool(
        &dir,
        "edit",
        json!({ "id": FIXED_ID, "labels": ["package=jp_legacy", "client=cli"] }),
    ));

    assert_eq!(
        out,
        format!("Edited {id}:\n- labels: client=cli, package=jp_legacy\n")
    );
}

#[test]
fn a_retired_label_cannot_be_added_fresh() {
    let dir = Utf8TempDir::new().unwrap();
    write_ticket(&dir, "Fresh");

    let message = error_message(run_tool(
        &dir,
        "edit",
        json!({ "id": FIXED_ID, "labels": ["package=jp_legacy"] }),
    ));

    assert_eq!(
        message,
        "`package=jp_legacy` is retired and can only stay on an item that already carries it. \
         Labels you can add: client=cli, client=macos, package=jp_cli, package=jp_config."
    );
}

/// The refusal has to land on the preview too: an unattended formatter that
/// errors fails the call before the user is asked to approve one that cannot
/// land.
#[test]
fn an_unknown_label_is_refused_before_the_preview() {
    let dir = Utf8TempDir::new().unwrap();

    let message = error_message(preview_tool(
        &dir,
        "create",
        json!({ "title": "Typo", "labels": ["client=clii"] }),
    ));

    assert_eq!(
        message,
        "`client=clii` is not a known label. Labels you can add: client=cli, client=macos, \
         package=jp_cli, package=jp_config."
    );
}

#[test]
fn labelling_a_missing_ticket_says_so() {
    let dir = Utf8TempDir::new().unwrap();

    let message = error_message(run_tool(
        &dir,
        "edit",
        json!({ "id": FIXED_ID, "labels": ["client=cli"] }),
    ));

    assert_eq!(message, format!("No {FIXED_ID}."));
}

/// The host compiles the call's access grants and the plugin enforces them: a
/// policy that only lets the tool read refuses a write before anything lands.
#[test]
fn a_write_the_access_policy_denies_is_refused() {
    let dir = Utf8TempDir::new().unwrap();
    let read_only = AccessPolicy {
        fs: vec![FsRule::new("").with_read(true)],
        ..AccessPolicy::default()
    };

    let message = error_message(run_with_access(
        &dir,
        "create",
        json!({ "title": "Bump the deny list" }),
        &read_only,
    ));

    assert!(
        message.starts_with("`create` can't reach the ticket directory: access denied: create on"),
        "{message}"
    );
    assert!(ids(&dir).is_empty(), "a denied write filed a ticket");

    // Reading is what the policy grants, so a read goes through.
    assert_eq!(
        content(run_with_access(&dir, "list", json!({}), &read_only)),
        "No tickets match.\n"
    );
}

/// A tool reaches only the workspace, policy or not, so a ticket directory
/// configured outside it is refused.
#[test]
fn a_ticket_directory_outside_the_workspace_is_refused() {
    let dir = Utf8TempDir::new().unwrap();
    let elsewhere = Utf8TempDir::new().unwrap();
    let call = ToolCall {
        action: ToolAction::Run,
        name: "list".to_owned(),
        arguments: Map::new(),
        answers: Map::new(),
        options: Map::new(),
        access: None,
        conversation: "test".to_owned(),
    };
    let options = Options {
        dir: elsewhere.path().to_path_buf(),
        ..Options::default()
    };

    let message = error_message(run(&init(&dir), &call, &options));

    assert_eq!(
        message,
        format!(
            "The ticket directory {} is outside the workspace, which is all a tool reaches.",
            elsewhere.path()
        )
    );
}

/// A policy the plugin can't read refuses the call rather than reading as
/// unrestricted.
#[test]
fn an_unreadable_access_policy_refuses_the_call() {
    let dir = Utf8TempDir::new().unwrap();
    let call = ToolCall {
        action: ToolAction::Run,
        name: "list".to_owned(),
        arguments: Map::new(),
        answers: Map::new(),
        options: Map::new(),
        access: Some(json!({ "fs": "everything" })),
        conversation: "test".to_owned(),
    };

    let message = error_message(run(&init(&dir), &call, &options()));

    assert!(
        message.starts_with("The access policy for this call can't be read: "),
        "{message}"
    );
}
