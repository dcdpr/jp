use std::{cell::Cell, fs};

use jp_config::{
    PartialAppConfig,
    assignment::{AssignKeyValue as _, KvAssignment},
    assistant::tool_choice::ToolChoice,
    model::parameters::PartialReasoningConfig,
};
use serde_json::json;

use super::*;
use crate::cmd::tests::{Workspace, ticket};

/// A ticket vocabulary defining the tracking label.
fn vocabulary() -> Vocabulary {
    Vocabulary::parse(
        r#"{"type": {"description": "The type of ticket", "values": ["bug", "feature", "tracking"]}}"#,
    )
    .unwrap()
}

/// RFD 116 is Accepted and RFD 117 is in Discussion, beside an empty ticket
/// directory.
fn workspace() -> Workspace {
    let workspace = Workspace::new();
    fs::create_dir_all(&workspace.paths.tickets).unwrap();
    workspace.rfd(
        "116-shareable-tooling.md",
        &[("Status", "Accepted")],
        "Body.\n\n## Implementation Plan\n\n### Phase 1: Tickets\n",
    );
    workspace.rfd("117-discussed.md", &[("Status", "Discussion")], "Body.");

    workspace
}

fn filer() -> Filer {
    Filer {
        author: "Jane Doe <jane@example.com>".to_owned(),
        date: "2026-10-15".to_owned(),
    }
}

fn id(text: &str) -> RfdId {
    text.parse().unwrap()
}

fn phases() -> Plan {
    Plan::Phases(vec![
        Phase {
            title: "Configure tickets".to_owned(),
            summary: "Reads the options.".to_owned(),
        },
        Phase {
            title: "Read RFDs".to_owned(),
            summary: "Lists them,\n  on two lines.".to_owned(),
        },
    ])
}

/// A plan reader that must not be reached.
fn unread(_: &PlanRequest<'_>) -> Result<Plan, String> {
    panic!("the plan was read")
}

/// The ticket files in the ticket directory, by name.
fn ticket_files(workspace: &Workspace) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(&workspace.paths.tickets)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| Utf8Path::new(name).extension() == Some("md"))
        .collect();
    names.sort();

    names
}

/// The document `ticket` is filed as.
fn read_ticket(workspace: &Workspace, ticket: TicketId) -> String {
    let path = store::locate_ticket(&workspace.paths.tickets, ticket).unwrap();
    fs::read_to_string(path).unwrap()
}

/// RFD 116's tracking ticket, already filed as `T-02wt0k1` with `description`.
fn filed(workspace: &Workspace, status: &str, description: &str) -> String {
    workspace.write(
        "docs/ticket/02wt0k1-shareable-tooling.md",
        &ticket(
            "Shareable tooling",
            status,
            &[("Implements", "116"), ("Label", "type=tracking")],
            description,
        ),
    );

    workspace.read("docs/ticket/02wt0k1-shareable-tooling.md")
}

#[test]
fn track_files_a_ticket_holding_the_plan() {
    let workspace = workspace();
    let mut asked = vec![];

    let tracked = track(
        &workspace.paths,
        &vocabulary(),
        id("116"),
        &filer(),
        &mut |request| {
            asked.push((request.id, request.file.to_owned()));
            assert!(request.document.contains("### Phase 1: Tickets"));
            Ok(phases())
        },
    )
    .unwrap();

    assert_eq!(asked, [(
        id("116"),
        "docs/rfd/116-shareable-tooling.md".to_owned()
    )]);
    assert_eq!(
        read_ticket(&workspace, tracked.ticket),
        "\
# Shareable tooling

- **Status**: Todo
- **Authors**: Jane Doe <jane@example.com>
- **Date**: 2026-10-15
- **Implements**: 116
- **Label**: type=tracking

Tracking ticket for [RFD 116](../rfd/116-shareable-tooling.md).

## Implementation plan

- **Configure tickets**: Reads the options.
- **Read RFDs**: Lists them, on two lines.
"
    );

    let path = store::locate_ticket(&workspace.paths.tickets, tracked.ticket).unwrap();
    assert_eq!(tracked.report.notes, [format!(
        "Filed {} ({}), tracking RFD 116.",
        workspace.paths.show(&path),
        tracked.ticket
    )]);
    assert!(tracked.report.warnings.is_empty());
}

/// Without a plan to list, the marker stands in for it, and the next `track`
/// reads it again.
#[test]
fn a_plan_that_cannot_be_read_leaves_the_marker() {
    let workspace = workspace();

    let tracked = track(
        &workspace.paths,
        &vocabulary(),
        id("116"),
        &filer(),
        &mut |_| Ok(Plan::Unavailable("the provider is down".to_owned())),
    )
    .unwrap();

    let description = "\
Tracking ticket for [RFD 116](../rfd/116-shareable-tooling.md).

<!-- rfd-plan: pending -->
";
    assert!(read_ticket(&workspace, tracked.ticket).ends_with(description));

    let warning = "The Implementation Plan of RFD 116 could not be read: the provider is down. \
                   The tracking ticket holds `<!-- rfd-plan: pending -->` in its place until `jp \
                   rfd track 116` reads it.";
    assert_eq!(tracked.report.warnings, [warning]);
}

#[test]
fn an_rfd_without_a_plan_leaves_the_marker() {
    let workspace = workspace();

    let tracked = track(
        &workspace.paths,
        &vocabulary(),
        id("116"),
        &filer(),
        &mut |_| Ok(Plan::Phases(vec![])),
    )
    .unwrap();

    assert!(read_ticket(&workspace, tracked.ticket).contains(PENDING));

    let warning = "No Implementation Plan was found in RFD 116. The tracking ticket holds `<!-- \
                   rfd-plan: pending -->` in its place until `jp rfd track 116` reads it.";
    assert_eq!(tracked.report.warnings, [warning]);
}

/// The description is the project's to edit once the plan is in, so a ticket
/// without the marker is neither read for nor rewritten.
#[test]
fn an_existing_tracking_ticket_is_reused() {
    let workspace = workspace();
    let before = filed(&workspace, "Todo", "Edited by hand.");

    let tracked = track(
        &workspace.paths,
        &vocabulary(),
        id("116"),
        &filer(),
        &mut unread,
    )
    .unwrap();

    assert_eq!(tracked.ticket.to_string(), "T-02wt0k1");
    let note = "docs/ticket/02wt0k1-shareable-tooling.md (T-02wt0k1) already tracks RFD 116.";
    assert_eq!(tracked.report.notes, [note]);
    assert_eq!(
        workspace.read("docs/ticket/02wt0k1-shareable-tooling.md"),
        before
    );
    assert_eq!(ticket_files(&workspace), ["02wt0k1-shareable-tooling.md"]);
}

#[test]
fn a_pending_plan_is_read_into_the_existing_ticket() {
    let workspace = workspace();
    filed(
        &workspace,
        "In Progress",
        &format!("Tracking ticket for [RFD 116](../rfd/116-shareable-tooling.md).\n\n{PENDING}"),
    );

    let tracked = track(
        &workspace.paths,
        &vocabulary(),
        id("116"),
        &filer(),
        &mut |_| Ok(phases()),
    )
    .unwrap();

    let note = "Read the Implementation Plan of RFD 116 into \
                docs/ticket/02wt0k1-shareable-tooling.md (T-02wt0k1).";
    assert_eq!(tracked.report.notes, [note]);

    let text = workspace.read("docs/ticket/02wt0k1-shareable-tooling.md");
    assert!(text.contains("- **Status**: In Progress\n"), "{text}");
    let description = "\
Tracking ticket for [RFD 116](../rfd/116-shareable-tooling.md).

## Implementation plan

- **Configure tickets**: Reads the options.
- **Read RFDs**: Lists them, on two lines.
";
    assert!(text.ends_with(description), "{text}");
}

#[test]
fn a_plan_still_pending_leaves_the_ticket_as_it_was() {
    let workspace = workspace();
    let before = filed(
        &workspace,
        "Todo",
        "Tracking ticket.\n\n<!-- rfd-plan: pending -->",
    );

    let tracked = track(
        &workspace.paths,
        &vocabulary(),
        id("116"),
        &filer(),
        &mut |_| Ok(Plan::Unavailable("no provider".to_owned())),
    )
    .unwrap();

    assert_eq!(
        workspace.read("docs/ticket/02wt0k1-shareable-tooling.md"),
        before
    );
    assert_eq!(tracked.report.notes, Vec::<String>::new());
    assert_eq!(tracked.report.warnings.len(), 1);
}

/// Text written around the marker is the project's, and stays.
#[test]
fn only_the_marker_is_replaced() {
    let workspace = workspace();
    filed(
        &workspace,
        "Todo",
        &format!("Tracking ticket.\n\n{PENDING}\n\nNotes added by hand."),
    );

    track(
        &workspace.paths,
        &vocabulary(),
        id("116"),
        &filer(),
        &mut |_| Ok(phases()),
    )
    .unwrap();

    let text = workspace.read("docs/ticket/02wt0k1-shareable-tooling.md");
    let description = "\
Tracking ticket.

## Implementation plan

- **Configure tickets**: Reads the options.
- **Read RFDs**: Lists them, on two lines.

Notes added by hand.
";
    assert!(text.ends_with(description), "{text}");
}

/// Reading the plan takes a while; a ticket filled in the meantime, by another
/// run or by hand, is not filled again.
#[test]
fn a_ticket_filled_while_the_plan_was_read_is_left_alone() {
    let workspace = workspace();
    filed(
        &workspace,
        "Todo",
        &format!("Tracking ticket.\n\n{PENDING}"),
    );
    let path = workspace
        .paths
        .root
        .join("docs/ticket/02wt0k1-shareable-tooling.md");

    let tracked = track(
        &workspace.paths,
        &vocabulary(),
        id("116"),
        &filer(),
        &mut |_| {
            let text = fs::read_to_string(&path).unwrap();
            fs::write(&path, text.replace(PENDING, "Filed by another run.")).unwrap();
            Ok(phases())
        },
    )
    .unwrap();

    assert!(
        workspace
            .read("docs/ticket/02wt0k1-shareable-tooling.md")
            .ends_with("Tracking ticket.\n\nFiled by another run.\n")
    );
    let note = "docs/ticket/02wt0k1-shareable-tooling.md (T-02wt0k1) already holds the \
                Implementation Plan of RFD 116.";
    assert_eq!(tracked.report.notes, [note]);
}

/// A plan left for later is a note, not a warning: nothing went wrong.
#[test]
fn a_deferred_plan_files_the_ticket_with_the_marker() {
    let workspace = workspace();

    let tracked = track(
        &workspace.paths,
        &vocabulary(),
        id("116"),
        &filer(),
        &mut |_| Ok(Plan::Deferred),
    )
    .unwrap();

    assert!(read_ticket(&workspace, tracked.ticket).ends_with(&format!("{PENDING}\n")));
    assert!(tracked.report.warnings.is_empty());
    assert_eq!(
        tracked.report.notes[0],
        "The tracking ticket holds `<!-- rfd-plan: pending -->` in place of the Implementation \
         Plan of RFD 116 until `jp rfd track 116` reads it."
    );
}

/// Phase tickets name the RFD they belong to without the tracking label.
#[test]
fn a_phase_ticket_is_not_the_tracking_ticket() {
    let workspace = workspace();
    workspace.write(
        "docs/ticket/02wt0k1-phase-one.md",
        &ticket("Phase one", "Todo", &[("Implements", "116")], ""),
    );

    let tracked = track(
        &workspace.paths,
        &vocabulary(),
        id("116"),
        &filer(),
        &mut |_| Ok(phases()),
    )
    .unwrap();

    assert_ne!(tracked.ticket.to_string(), "T-02wt0k1");
    assert_eq!(ticket_files(&workspace).len(), 2);
}

/// Implementation begins at Accepted, and so does a tracking ticket.
#[test]
fn only_an_accepted_or_implemented_rfd_is_tracked() {
    let workspace = workspace();
    workspace.rfd("D05-sketch.md", &[("Status", "Draft")], "Body.");

    for (rfd, error) in [
        (
            "117",
            "Cannot track: 117-discussed.md is 'Discussion'.\nOnly Accepted or Implemented RFDs \
             carry a tracking ticket.",
        ),
        (
            "D05",
            "Cannot track: D05-sketch.md is 'Draft'.\nOnly Accepted or Implemented RFDs carry a \
             tracking ticket.",
        ),
    ] {
        assert_eq!(
            track(
                &workspace.paths,
                &vocabulary(),
                id(rfd),
                &filer(),
                &mut unread
            )
            .unwrap_err(),
            error
        );
        assert_eq!(
            start(
                &workspace.paths,
                &vocabulary(),
                id(rfd),
                &filer(),
                &mut unread
            )
            .unwrap_err(),
            error
        );
    }
    assert!(ticket_files(&workspace).is_empty());

    workspace.rfd("030-built.md", &[("Status", "Implemented")], "Body.");
    assert!(
        track(
            &workspace.paths,
            &vocabulary(),
            id("030"),
            &filer(),
            &mut |_| Ok(phases())
        )
        .is_ok()
    );
}

#[test]
fn tracking_needs_a_ticket_directory() {
    let workspace = workspace();
    fs::remove_dir_all(&workspace.paths.tickets).unwrap();
    let error = "Tracking needs tickets, and there is no ticket directory at docs/ticket.";

    assert_eq!(
        track(
            &workspace.paths,
            &vocabulary(),
            id("116"),
            &filer(),
            &mut unread
        )
        .unwrap_err(),
        error
    );
    assert_eq!(
        start(
            &workspace.paths,
            &vocabulary(),
            id("116"),
            &filer(),
            &mut unread
        )
        .unwrap_err(),
        error
    );
    assert_eq!(stop(&workspace.paths, id("116")).unwrap_err(), error);
}

#[test]
fn a_vocabulary_has_to_define_the_tracking_label() {
    let workspace = workspace();

    let without =
        Vocabulary::parse(r#"{"type": {"description": "The type", "values": ["bug"]}}"#).unwrap();
    assert_eq!(
        track(&workspace.paths, &without, id("116"), &filer(), &mut unread).unwrap_err(),
        "Tracking label missing. Add `\"tracking\"` to \
         `plugins.command.ticket.options.labels.type.values`."
    );

    let retired = Vocabulary::parse(
        r#"{"type": {"description": "The type", "values": ["bug"], "retired": ["tracking"]}}"#,
    )
    .unwrap();
    assert_eq!(
        track(&workspace.paths, &retired, id("116"), &filer(), &mut unread).unwrap_err(),
        "Tracking label retired. Move `\"tracking\"` from \
         `plugins.command.ticket.options.labels.type.retired` to \
         `plugins.command.ticket.options.labels.type.values`."
    );

    assert!(ticket_files(&workspace).is_empty());
}

/// A board that defines no labels takes any, so it takes the tracking label.
#[test]
fn a_board_without_labels_takes_the_tracking_label() {
    let workspace = workspace();

    let tracked = track(
        &workspace.paths,
        &Vocabulary::default(),
        id("116"),
        &filer(),
        &mut |_| Ok(phases()),
    )
    .unwrap();

    assert!(
        read_ticket(&workspace, tracked.ticket).contains("- **Label**: type=tracking\n"),
        "{}",
        read_ticket(&workspace, tracked.ticket)
    );
}

/// An interrupted read must not file a ticket without its plan.
#[test]
fn an_interrupted_plan_read_files_nothing() {
    let workspace = workspace();

    let error = track(
        &workspace.paths,
        &vocabulary(),
        id("116"),
        &filer(),
        &mut |_| Err("Interrupted.".to_owned()),
    )
    .unwrap_err();

    assert_eq!(error, "Interrupted.");
    assert!(ticket_files(&workspace).is_empty());
}

#[test]
fn start_files_the_ticket_and_moves_it_to_in_progress() {
    let workspace = workspace();

    let output = start(
        &workspace.paths,
        &vocabulary(),
        id("116"),
        &filer(),
        &mut |_| Ok(phases()),
    )
    .unwrap();

    let [name] = ticket_files(&workspace).try_into().unwrap();
    assert_eq!(
        output.text,
        format!("docs/ticket/{name}: Todo -> In Progress\n")
    );
    assert!(
        output.notes[0].starts_with("Filed docs/ticket/"),
        "{:?}",
        output.notes
    );
    assert!(
        workspace
            .read(&format!("docs/ticket/{name}"))
            .contains("- **Status**: In Progress\n")
    );

    let read = Cell::new(false);
    let again = start(
        &workspace.paths,
        &vocabulary(),
        id("116"),
        &filer(),
        &mut |_| {
            read.set(true);
            Ok(phases())
        },
    )
    .unwrap();
    assert_eq!(
        again.text,
        format!("docs/ticket/{name}: already In Progress\n")
    );
    assert!(
        !read.get(),
        "a ticket with its plan in is not read for again"
    );
}

/// A mark left behind by an RFD that moved back to Discussion has to be
/// clearable, so `stop` takes any status.
#[test]
fn stop_moves_the_ticket_back_to_todo_whatever_the_rfd_says() {
    let workspace = workspace();
    workspace.write(
        "docs/ticket/02wt0k1-discussed.md",
        &ticket(
            "Discussed",
            "In Progress",
            &[("Implements", "117"), ("Label", "type=tracking")],
            "",
        ),
    );

    let output = stop(&workspace.paths, id("117")).unwrap();

    assert_eq!(
        output.text,
        "docs/ticket/02wt0k1-discussed.md: In Progress -> Todo\n"
    );
    assert!(
        workspace
            .read("docs/ticket/02wt0k1-discussed.md")
            .contains("- **Status**: Todo\n")
    );

    assert_eq!(
        stop(&workspace.paths, id("117")).unwrap().text,
        "docs/ticket/02wt0k1-discussed.md: already Todo\n"
    );
}

#[test]
fn stop_without_a_ticket_files_nothing() {
    let workspace = workspace();

    let output = stop(&workspace.paths, id("116")).unwrap();

    assert_eq!(
        output.text,
        "RFD 116 has no tracking ticket; nothing to stop.\n"
    );
    assert!(ticket_files(&workspace).is_empty());
    assert_eq!(
        stop(&workspace.paths, id("099")).unwrap_err(),
        "No RFD found with number 099."
    );
}

#[test]
fn a_second_tracking_ticket_is_reported_and_the_first_used() {
    let workspace = workspace();
    filed(&workspace, "Todo", "First.");
    workspace.write(
        "docs/ticket/02wt0k2-shareable-tooling-again.md",
        &ticket(
            "Again",
            "Todo",
            &[("Implements", "RFD 116"), ("Label", "type=tracking")],
            "Second.",
        ),
    );

    let tracked = track(
        &workspace.paths,
        &vocabulary(),
        id("116"),
        &filer(),
        &mut unread,
    )
    .unwrap();

    assert_eq!(tracked.ticket.to_string(), "T-02wt0k1");
    let warning = "\
Warning: RFD 116 has more than one tracking ticket; using T-02wt0k1:
  docs/ticket/02wt0k1-shareable-tooling.md
  docs/ticket/02wt0k2-shareable-tooling-again.md";
    assert_eq!(tracked.report.warnings, [warning]);
}

/// A tracking ticket that can't be read can't be found either, so the one filed
/// now may duplicate it.
#[test]
fn an_unreadable_ticket_is_reported_when_filing() {
    let workspace = workspace();
    workspace.write("docs/ticket/02wt0k9-broken.md", "Not a ticket.\n");

    let tracked = track(
        &workspace.paths,
        &vocabulary(),
        id("116"),
        &filer(),
        &mut |_| Ok(phases()),
    )
    .unwrap();

    assert_eq!(tracked.report.warnings, [format!(
        "Warning: these tickets could not be read; if one of them tracks RFD 116, {} duplicates \
         it:\n  docs/ticket/02wt0k9-broken.md",
        tracked.ticket
    )]);
}

/// A ticket directory beside the docs, not in them, still links to the RFD.
#[test]
fn the_link_to_the_rfd_is_relative_to_the_ticket_directory() {
    let workspace = workspace();
    let paths = Paths::new(
        &workspace.paths.root,
        Utf8Path::new("docs/rfd"),
        Utf8Path::new("work/tickets"),
    );
    fs::create_dir_all(&paths.tickets).unwrap();

    let tracked = track(&paths, &vocabulary(), id("116"), &filer(), &mut |_| {
        Ok(phases())
    })
    .unwrap();

    let path = store::locate_ticket(&paths.tickets, tracked.ticket).unwrap();
    assert!(
        fs::read_to_string(path)
            .unwrap()
            .contains("Tracking ticket for [RFD 116](../../docs/rfd/116-shareable-tooling.md).")
    );
}

/// The plan is read in a conversation that is kept out of the workspace and
/// expires, with no reasoning and no tool calls.
#[test]
fn the_plan_query_asks_for_structured_phases_in_a_throwaway_conversation() {
    let query = plan_query(&PlanRequest {
        id: id("116"),
        file: "docs/rfd/116-x.md",
        document: "# RFD 116: X\n\nBody.\n",
    });

    assert!(query.new);
    assert_eq!(query.expires_in.as_deref(), Some("5m"));
    assert_eq!(
        query.title.as_deref(),
        Some("Implementation Plan of RFD 116")
    );
    assert_eq!(query.cfg, [
        "conversation.start_local=true",
        "assistant.model.parameters.reasoning=off",
        "assistant.tool_choice=none",
    ]);
    let content = format!(
        "{PLAN_PROMPT}

<document path=\"docs/rfd/116-x.md\">
# RFD 116: X

Body.
</document>
"
    );
    assert_eq!(query.content, content);

    let schema = Value::Object(query.schema.unwrap());
    assert_eq!(schema["required"], json!(["phases"]));
    assert_eq!(
        schema["properties"]["phases"]["items"]["required"],
        json!(["title", "summary"])
    );
}

/// `jp` applies each of the plan query's `cfg` strings the way it applies
/// `--cfg`, so a key it doesn't know fails every plan read.
#[test]
fn the_plan_query_configuration_names_keys_jp_knows() {
    let mut partial = PartialAppConfig::default();
    for arg in PLAN_CFG {
        let assignment: KvAssignment = arg.parse().unwrap();
        partial.assign(assignment).unwrap();
    }

    assert_eq!(partial.conversation.start_local, Some(true));
    assert_eq!(
        partial.assistant.model.parameters.reasoning,
        Some(PartialReasoningConfig::Off)
    );
    assert_eq!(partial.assistant.tool_choice, Some(ToolChoice::None));
}

#[test]
fn the_plan_is_read_from_the_structured_response() {
    assert_eq!(
        plan_from(Some(json!({
            "phases": [{ "title": "Configure tickets", "summary": "Reads the options." }]
        }))),
        Plan::Phases(vec![Phase {
            title: "Configure tickets".to_owned(),
            summary: "Reads the options.".to_owned(),
        }])
    );
    assert_eq!(
        plan_from(None),
        Plan::Unavailable("the response carried no structured data".to_owned())
    );

    let Plan::Unavailable(reason) = plan_from(Some(json!({ "steps": [] }))) else {
        panic!("a response without phases has no plan");
    };
    assert!(
        reason.starts_with("the response didn't match the schema: "),
        "{reason}"
    );
}
