use std::fs;

use super::*;
use crate::cmd::tests::{Workspace, ticket};

const TEMPLATE: &str = "\
<!-- A note for the author. -->

# RFD NNN: TITLE

- **Status**: Draft
- **Category**: Design
- **Authors**: AUTHOR
- **Date**: DATE

## Summary

One to three sentences.

## Motivation

Why.
";

const TICKET: &str = "T-02wt0kx";

fn categories() -> Vec<String> {
    ["design", "decision", "guide", "process"]
        .map(ToOwned::to_owned)
        .to_vec()
}

fn workspace() -> Workspace {
    let workspace = Workspace::new();
    workspace.write("docs/rfd/000-design-template.md", TEMPLATE);
    workspace.write(
        "docs/rfd/000-decision-template.md",
        &TEMPLATE.replace("Design", "Decision"),
    );

    workspace
}

fn request<'a>(category: &'a str, title: &'a str) -> Draft<'a> {
    Draft {
        category,
        title,
        author: "Jane Doe <jane@example.com>",
        date: "2026-10-15",
    }
}

#[test]
fn a_draft_is_its_template_filled_in() {
    let workspace = workspace();

    let output = draft(
        &workspace.paths,
        &categories(),
        &request("design", "My Feature"),
    )
    .unwrap();

    assert_eq!(output.text, "Created docs/rfd/drafts/D01-my-feature.md\n");
    assert_eq!(output.warnings, [] as [String; 0]);
    assert_eq!(
        workspace.read("docs/rfd/drafts/D01-my-feature.md"),
        "\
<!-- A note for the author. -->

# RFD D01: My Feature

- **Status**: Draft
- **Category**: Design
- **Authors**: Jane Doe <jane@example.com>
- **Date**: 2026-10-15

## Summary

One to three sentences.

## Motivation

Why.
"
    );
}

#[test]
fn a_draft_takes_the_first_free_slot() {
    let workspace = workspace();
    workspace.write("docs/rfd/drafts/D01-taken.md", "");
    workspace.write("docs/rfd/drafts/D03-taken.md", "");

    let output = draft(&workspace.paths, &categories(), &request("design", "Next")).unwrap();

    assert_eq!(output.text, "Created docs/rfd/drafts/D02-next.md\n");
}

/// Skip any slot still cited in another document, because promoting a draft
/// rewrites every citation of its ID and would redirect stale citations to the
/// new RFD.
#[test]
fn a_draft_skips_a_slot_another_document_still_cites() {
    let workspace = workspace();
    workspace.write("docs/rfd/drafts/D01-taken.md", "# RFD D01: Taken\n");
    workspace.write(
        "docs/ticket/02wt0kx-old.md",
        "See [RFD D02] and [the sketch](../rfd/drafts/D03-sketch.md).\n\nNot D056, xD04, or \
         D04_x.\n",
    );
    workspace.write("docs/rfd/042-published.md", "Built on RFD D02.\n");

    let output = draft(&workspace.paths, &categories(), &request("design", "Next")).unwrap();

    assert_eq!(output.text, "Created docs/rfd/drafts/D04-next.md\n");
    assert_eq!(output.warnings, ["Warning: draft ids cited with no draft \
                                  file to match; their slots are skipped \
                                  until the citations are fixed:\n  D02 \
                                  in docs/rfd/042-published.md\n  D02 in \
                                  docs/ticket/02wt0kx-old.md\n  D03 in \
                                  docs/ticket/02wt0kx-old.md"]);
}

/// Promotion rewrites `RFD D02` even when followed by a letter or `_`, so
/// non-standalone `RFD Dnn` citations also reserve their slots.
#[test]
fn a_draft_skips_a_slot_cited_inside_a_word() {
    let workspace = workspace();
    workspace.write("docs/rfd/drafts/D01-taken.md", "# RFD D01: Taken\n");
    workspace.write(
        "docs/ticket/02wt0kx-old.md",
        "See _RFD D02_ and the RFD D03s, but not RFD D045.\n",
    );

    let output = draft(&workspace.paths, &categories(), &request("design", "Next")).unwrap();

    assert_eq!(output.text, "Created docs/rfd/drafts/D04-next.md\n");
    assert_eq!(output.warnings, ["Warning: draft ids cited with no draft \
                                  file to match; their slots are skipped \
                                  until the citations are fixed:\n  D02 \
                                  in docs/ticket/02wt0kx-old.md\n  D03 \
                                  in docs/ticket/02wt0kx-old.md"]);
}

/// Promotion rewrites a draft's priority board entry to its permanent number,
/// so a stale draft entry on the board also reserves its slot.
#[test]
fn a_draft_skips_a_slot_the_board_still_ranks() {
    let workspace = workspace();
    workspace.board(
        r#"{ "planned": [{ "milestone": null, "ids": ["D01", "042"] }], "backlog": ["D02"] }"#,
    );

    let output = draft(&workspace.paths, &categories(), &request("design", "Next")).unwrap();

    assert_eq!(output.text, "Created docs/rfd/drafts/D03-next.md\n");
    assert_eq!(output.warnings, ["Warning: draft ids cited with no draft \
                                  file to match; their slots are skipped \
                                  until the citations are fixed:\n  D01 \
                                  in docs/rfd/.priority.json\n  D02 in \
                                  docs/rfd/.priority.json"]);
}

/// The category is written as the metadata spells it, whatever the template
/// says, and a category typed in capitals still names one.
#[test]
fn the_category_is_set_from_the_request() {
    let workspace = workspace();
    workspace.write(
        "docs/rfd/000-guide-template.md",
        &TEMPLATE.replace("Design", "Whatever"),
    );

    draft(&workspace.paths, &categories(), &request("Guide", "How To")).unwrap();

    let text = workspace.read("docs/rfd/drafts/D01-how-to.md");
    assert!(text.contains("- **Category**: Guide\n"), "{text}");
}

/// A title is text, not a pattern: the words the template's placeholders use
/// stay in it, and `/` and `&` reach the heading as typed.
#[test]
fn the_title_is_written_as_given() {
    let workspace = workspace();

    draft(
        &workspace.paths,
        &categories(),
        &request("design", "AUTHOR & DATE / Rules"),
    )
    .unwrap();

    let text = workspace.read("docs/rfd/drafts/D01-author--date--rules.md");
    assert!(
        text.contains("\n# RFD D01: AUTHOR & DATE / Rules\n"),
        "{text}"
    );
    assert!(
        text.contains("- **Authors**: Jane Doe <jane@example.com>\n"),
        "{text}"
    );
}

#[test]
fn an_unknown_category_is_refused() {
    let workspace = workspace();

    let error = draft(&workspace.paths, &categories(), &request("essay", "Title")).unwrap_err();

    assert_eq!(
        error,
        "Unknown category 'essay'. Use 'design', 'decision', 'guide', or 'process'."
    );
}

#[test]
fn a_category_without_a_template_points_at_init() {
    let workspace = workspace();

    let error = draft(
        &workspace.paths,
        &categories(),
        &request("process", "Title"),
    )
    .unwrap_err();

    assert_eq!(
        error,
        "There is no template for 'process' at docs/rfd/000-process-template.md. `jp rfd init` \
         writes one for each configured category."
    );
}

#[test]
fn a_title_without_a_slug_is_refused() {
    let workspace = workspace();

    let error = draft(&workspace.paths, &categories(), &request("design", "???")).unwrap_err();

    assert_eq!(error, "Title '???' produces an empty filename slug.");
    assert!(!workspace.exists("docs/rfd/drafts/D01-.md"));
}

#[test]
fn a_template_without_a_heading_is_refused() {
    let workspace = workspace();
    workspace.write("docs/rfd/000-design-template.md", "- **Status**: Draft\n");

    let error = draft(&workspace.paths, &categories(), &request("design", "Title")).unwrap_err();

    assert_eq!(
        error,
        "docs/rfd/000-design-template.md: the template has no `# RFD NNN: TITLE` heading to fill \
         in."
    );
}

fn ticket_file(workspace: &Workspace, extra: &[(&str, &str)]) -> String {
    workspace.write(
        "docs/ticket/02wt0kx-ship-the-widget.md",
        &ticket(
            "Ship the Widget",
            "Todo",
            extra,
            "The widget needs shipping.\n\nIn two steps.",
        ),
    );

    "docs/ticket/02wt0kx-ship-the-widget.md".to_owned()
}

#[test]
fn a_draft_from_a_ticket_carries_its_title_and_description() {
    let workspace = workspace();
    let ticket_path = ticket_file(&workspace, &[]);

    let output = from_ticket(
        &workspace.paths,
        &categories(),
        TICKET.parse().unwrap(),
        &request("design", ""),
    )
    .unwrap();

    assert_eq!(
        output.text,
        "Created docs/rfd/drafts/D01-ship-the-widget.md\ndocs/ticket/02wt0kx-ship-the-widget.md: \
         promoted to D01, closed as Done\n"
    );
    assert_eq!(
        workspace.read("docs/rfd/drafts/D01-ship-the-widget.md"),
        "\
<!-- A note for the author. -->

# RFD D01: Ship the Widget

- **Status**: Draft
- **Category**: Design
- **Authors**: Jane Doe <jane@example.com>
- **Date**: 2026-10-15

## Summary

The widget needs shipping.

In two steps.

Promoted from ticket T-02wt0kx.

## Motivation

Why.
"
    );

    let ticket = workspace.read(&ticket_path);
    assert!(ticket.contains("- **Status**: Done\n"), "{ticket}");
    assert!(ticket.contains("- **Promoted to**: D01\n"), "{ticket}");
}

#[test]
fn a_title_given_with_a_ticket_replaces_the_tickets() {
    let workspace = workspace();
    ticket_file(&workspace, &[]);

    let output = from_ticket(
        &workspace.paths,
        &categories(),
        TICKET.parse().unwrap(),
        &request("decision", "Widget Shipping Policy"),
    )
    .unwrap();

    assert!(
        output
            .text
            .starts_with("Created docs/rfd/drafts/D01-widget-shipping-policy.md\n"),
        "{}",
        output.text
    );
    let text = workspace.read("docs/rfd/drafts/D01-widget-shipping-policy.md");
    assert!(
        text.contains("# RFD D01: Widget Shipping Policy\n"),
        "{text}"
    );
    assert!(text.contains("- **Category**: Decision\n"), "{text}");
}

/// Running it again after it finished reports where the ticket went.
#[test]
fn a_ticket_already_promoted_to_an_existing_rfd_is_reported() {
    let workspace = workspace();
    ticket_file(&workspace, &[("Promoted to", "042")]);
    workspace.rfd("042-the-widget.md", &[("Status", "Discussion")], "Body.");

    let output = from_ticket(
        &workspace.paths,
        &categories(),
        TICKET.parse().unwrap(),
        &request("design", ""),
    )
    .unwrap();

    assert_eq!(
        output.text,
        "T-02wt0kx is already promoted to 042 (docs/rfd/042-the-widget.md).\n"
    );
    assert!(!workspace.exists("docs/rfd/drafts/D01-ship-the-widget.md"));
}

#[test]
fn a_ticket_naming_a_missing_rfd_is_seeded_again() {
    let workspace = workspace();
    let ticket_path = ticket_file(&workspace, &[("Promoted to", "D07")]);

    let output = from_ticket(
        &workspace.paths,
        &categories(),
        TICKET.parse().unwrap(),
        &request("design", ""),
    )
    .unwrap();

    assert_eq!(output.warnings, [
        "T-02wt0kx names D07, but no such RFD exists; seeding a new draft."
    ]);
    assert!(workspace.exists("docs/rfd/drafts/D01-ship-the-widget.md"));
    assert!(
        workspace
            .read(&ticket_path)
            .contains("- **Promoted to**: D01\n")
    );
}

/// Only the seed ticket's `Promoted to` field (which the command overwrites) is
/// omitted from the warning; dangling citations in the ticket's description are
/// still reported.
#[test]
fn a_seed_tickets_own_citations_are_reported() {
    let workspace = workspace();
    workspace.write(
        "docs/ticket/02wt0kx-ship-the-widget.md",
        &ticket(
            "Ship the Widget",
            "Todo",
            &[("Promoted to", "D07")],
            "Builds on RFD D09.",
        ),
    );

    let output = from_ticket(
        &workspace.paths,
        &categories(),
        TICKET.parse().unwrap(),
        &request("design", ""),
    )
    .unwrap();

    assert_eq!(output.warnings, [
        "T-02wt0kx names D07, but no such RFD exists; seeding a new draft.",
        "Warning: draft ids cited with no draft file to match; their slots are skipped until the \
         citations are fixed:\n  D09 in docs/ticket/02wt0kx-ship-the-widget.md",
    ]);
    assert!(workspace.exists("docs/rfd/drafts/D01-ship-the-widget.md"));
}

/// A run interrupted after writing the draft and before closing the ticket left
/// the draft behind; the next run links it rather than seeding a second one.
#[test]
fn a_draft_seeded_by_an_interrupted_run_is_linked() {
    let workspace = workspace();
    let ticket_path = ticket_file(&workspace, &[]);
    workspace.write(
        "docs/rfd/drafts/D04-ship-the-widget.md",
        "# RFD D04: Ship the Widget\n\n- **Status**: Draft\n\n## Summary\n\nPromoted from ticket \
         T-02wt0kx.\n",
    );

    let output = from_ticket(
        &workspace.paths,
        &categories(),
        TICKET.parse().unwrap(),
        &request("design", ""),
    )
    .unwrap();

    assert_eq!(
        output.text,
        "T-02wt0kx already seeded docs/rfd/drafts/D04-ship-the-widget.md; linking \
         it.\ndocs/ticket/02wt0kx-ship-the-widget.md: promoted to D04, closed as Done\n"
    );
    assert!(!workspace.exists("docs/rfd/drafts/D01-ship-the-widget.md"));
    assert!(
        workspace
            .read(&ticket_path)
            .contains("- **Promoted to**: D04\n")
    );
}

#[test]
fn drafting_from_a_ticket_needs_a_ticket_directory() {
    let workspace = workspace();
    fs::remove_dir_all(&workspace.paths.tickets).unwrap();

    let error = from_ticket(
        &workspace.paths,
        &categories(),
        TICKET.parse().unwrap(),
        &request("design", ""),
    )
    .unwrap_err();

    assert_eq!(
        error,
        "Drafting from a ticket needs tickets, and there is no ticket directory at docs/ticket."
    );
}

#[test]
fn a_ticket_that_does_not_exist_is_an_error() {
    let workspace = workspace();

    let error = from_ticket(
        &workspace.paths,
        &categories(),
        TICKET.parse().unwrap(),
        &request("design", ""),
    )
    .unwrap_err();

    assert!(error.contains("T-02wt0kx"), "{error}");
    assert!(!workspace.exists("docs/rfd/drafts/D01-ship-the-widget.md"));
}

/// A template without a `## Summary` still gets the ticket's description, in a
/// section of its own.
#[test]
fn a_template_without_a_summary_gets_one() {
    let workspace = workspace();
    workspace.write(
        "docs/rfd/000-design-template.md",
        "# RFD NNN: TITLE\n\n- **Status**: Draft\n- **Category**: Design\n\n## Context\n\nWhy.\n",
    );
    ticket_file(&workspace, &[]);

    from_ticket(
        &workspace.paths,
        &categories(),
        TICKET.parse().unwrap(),
        &request("design", ""),
    )
    .unwrap();

    assert_eq!(
        workspace.read("docs/rfd/drafts/D01-ship-the-widget.md"),
        "# RFD D01: Ship the Widget\n\n- **Status**: Draft\n- **Category**: Design\n\n## \
         Summary\n\nThe widget needs shipping.\n\nIn two steps.\n\nPromoted from ticket \
         T-02wt0kx.\n\n## Context\n\nWhy.\n"
    );
}
