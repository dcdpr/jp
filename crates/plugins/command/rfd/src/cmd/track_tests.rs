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

/// A ticket vocabulary that takes a tracking ticket: the link, open to any RFD
/// and held to one ticket per RFD, and a `type` with `tracking` among its
/// values.
fn vocabulary() -> Vocabulary {
    Vocabulary::parse(
        r#"{
            "tracking-rfd": {"description": "The RFD tracked", "values": [], "max": 1, "unique": "key-value"},
            "type": {"description": "The type of ticket", "values": ["bug", "feature", "tracking"]}
        }"#,
    )
    .unwrap()
}

/// The default link keys, with a tracking ticket also filed as `type=tracking`.
fn links() -> Links {
    Links {
        filed_with: vec!["type=tracking".to_owned()],
        ..Links::default()
    }
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
            &[("Label", "tracking-rfd=116"), ("Label", "type=tracking")],
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
        &links(),
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
- **Label**: tracking-rfd=116
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
        &links(),
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
        &links(),
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
        &links(),
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
        &links(),
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
        &links(),
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
        &links(),
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
        &links(),
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

/// A phase ticket names the RFD its work belongs to under the relevant key,
/// which tracks nothing.
#[test]
fn a_phase_ticket_is_not_the_tracking_ticket() {
    let workspace = workspace();
    workspace.write(
        "docs/ticket/02wt0k1-phase-one.md",
        &ticket("Phase one", "Todo", &[("Label", "relevant-rfd=116")], ""),
    );

    let tracked = track(
        &workspace.paths,
        &vocabulary(),
        &links(),
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
                &links(),
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
                &links(),
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
            &links(),
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
            &links(),
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
            &links(),
            id("116"),
            &filer(),
            &mut unread
        )
        .unwrap_err(),
        error
    );
    assert_eq!(
        stop(&workspace.paths, &links(), id("116")).unwrap_err(),
        error
    );
}

/// A board that defines labels but not the link can't take a tracking ticket,
/// and is told the entry to add, ready to paste.
#[test]
fn a_board_has_to_define_the_link_before_tracking() {
    let workspace = workspace();
    let without = Vocabulary::parse(
        r#"{"type": {"description": "The type", "values": ["bug", "tracking"]}}"#,
    )
    .unwrap();

    assert_eq!(
        track(
            &workspace.paths,
            &without,
            &links(),
            id("116"),
            &filer(),
            &mut unread
        )
        .unwrap_err(),
        "\
RFD tracking must be enabled first by adding the `tracking-rfd` label to the ticket configuration:

    [plugins.command.ticket.options.labels.tracking-rfd]
    description = \"The RFD whose implementation this ticket tracks.\"
    values = []
    max = 1
    unique = \"key-value\""
    );
    assert!(ticket_files(&workspace).is_empty());
}

/// A link listing fixed values can't name every RFD.
#[test]
fn the_link_has_to_take_any_value() {
    let workspace = workspace();

    for declared in [
        r#"{"tracking-rfd": {}, "type": {"values": ["tracking"]}}"#,
        r#"{"tracking-rfd": {"values": ["042"]}, "type": {"values": ["tracking"]}}"#,
    ] {
        let vocabulary = Vocabulary::parse(declared).unwrap();

        assert_eq!(
            track(
                &workspace.paths,
                &vocabulary,
                &links(),
                id("116"),
                &filer(),
                &mut unread
            )
            .unwrap_err(),
            "RFD tracking needs `tracking-rfd` to take any value: set `values = []` under \
             `plugins.command.ticket.options.labels.tracking-rfd`.",
            "{declared}"
        );
    }
    assert!(ticket_files(&workspace).is_empty());
}

/// A board that requires another key, or refuses a label the ticket is filed
/// with, is named as the reason.
#[test]
fn a_board_refusing_the_other_labels_is_named_as_the_reason() {
    let workspace = workspace();
    let vocabulary = Vocabulary::parse(
        r#"{
            "tracking-rfd": {"values": []},
            "type": {"values": ["tracking"], "min": 1, "max": 1},
            "area": {"values": ["cli"], "min": 1}
        }"#,
    )
    .unwrap();

    assert_eq!(
        track(
            &workspace.paths,
            &vocabulary,
            &links(),
            id("116"),
            &filer(),
            &mut unread
        )
        .unwrap_err(),
        "A tracking ticket is filed with `tracking-rfd=116` and `type=tracking`, and the ticket \
         board refuses them. `area` label missing: add one of area=cli."
    );
    assert!(ticket_files(&workspace).is_empty());
}

/// A board that defines no labels takes any, so it takes the link.
#[test]
fn a_board_without_labels_takes_the_link() {
    let workspace = workspace();

    let tracked = track(
        &workspace.paths,
        &Vocabulary::default(),
        &Links::default(),
        id("116"),
        &filer(),
        &mut |_| Ok(phases()),
    )
    .unwrap();

    let text = read_ticket(&workspace, tracked.ticket);
    assert!(text.contains("- **Label**: tracking-rfd=116\n"), "{text}");
    assert!(!text.contains("type=tracking"), "{text}");
}

/// A board that names its link its own way tracks under that name.
#[test]
fn the_link_key_is_the_configured_one() {
    let workspace = workspace();
    let links = Links {
        tracking: "tracks".to_owned(),
        relevant: "rfd".to_owned(),
        filed_with: vec![],
    };
    workspace.write(
        "docs/ticket/02wt0k1-old-style.md",
        &ticket("Old style", "Todo", &[("Label", "tracking-rfd=116")], ""),
    );

    let tracked = track(
        &workspace.paths,
        &Vocabulary::default(),
        &links,
        id("116"),
        &filer(),
        &mut |_| Ok(phases()),
    )
    .unwrap();

    // The ticket under the default key tracks nothing on this board.
    assert_ne!(tracked.ticket.to_string(), "T-02wt0k1");
    assert!(read_ticket(&workspace, tracked.ticket).contains("- **Label**: tracks=116\n"));
}

/// An interrupted read must not file a ticket without its plan.
#[test]
fn an_interrupted_plan_read_files_nothing() {
    let workspace = workspace();

    let error = track(
        &workspace.paths,
        &vocabulary(),
        &links(),
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
        &links(),
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
        &links(),
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
            &[("Label", "tracking-rfd=117")],
            "",
        ),
    );

    let output = stop(&workspace.paths, &links(), id("117")).unwrap();

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
        stop(&workspace.paths, &links(), id("117")).unwrap().text,
        "docs/ticket/02wt0k1-discussed.md: already Todo\n"
    );
}

#[test]
fn stop_without_a_ticket_files_nothing() {
    let workspace = workspace();

    let output = stop(&workspace.paths, &links(), id("116")).unwrap();

    assert_eq!(
        output.text,
        "RFD 116 has no tracking ticket; nothing to stop.\n"
    );
    assert!(ticket_files(&workspace).is_empty());
    assert_eq!(
        stop(&workspace.paths, &links(), id("099")).unwrap_err(),
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
            &[("Label", "tracking-rfd=RFD 116")],
            "Second.",
        ),
    );

    let tracked = track(
        &workspace.paths,
        &vocabulary(),
        &links(),
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
        &links(),
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

    let tracked = track(
        &paths,
        &vocabulary(),
        &links(),
        id("116"),
        &filer(),
        &mut |_| Ok(phases()),
    )
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
