use super::*;
use crate::cmd::tests::{Workspace, ticket};

fn id(text: &str) -> RfdId {
    text.parse().unwrap()
}

/// A draft about to be published, and the documents that know it by its draft
/// id: a published RFD it extends, a published RFD and a draft that mention it,
/// and a ticket that was promoted to it.
fn publishing() -> Workspace {
    let workspace = Workspace::new();
    workspace.rfd(
        "001-process.md",
        &[("Status", "Implemented"), ("Category", "Process")],
        "The process.",
    );
    workspace.rfd(
        "002-other.md",
        &[("Status", "Discussion")],
        "See [the sketch](drafts/D05-sketch.md) and RFD D05.",
    );
    workspace.rfd(
        "D05-sketch.md",
        &[
            ("Status", "Draft"),
            ("Category", "Design"),
            ("Extends", "[RFD 001](../001-process.md)"),
            ("Required by", "[RFD D07](D07-later.md)"),
        ],
        "This draft, D05, builds on [the process](../001-process.md).",
    );
    workspace.rfd(
        "D07-later.md",
        &[
            ("Status", "Draft"),
            ("Requires", "[RFD D05](D05-sketch.md)"),
        ],
        "Builds on D05.",
    );
    workspace.write(
        "docs/ticket/02wt0kx-sketch-it.md",
        &ticket(
            "Sketch It",
            "Done",
            &[("Promoted to", "D05")],
            "Now [the sketch](../rfd/drafts/D05-sketch.md), docs/rfd/drafts/D05-sketch.md.",
        ),
    );
    workspace.board(
        r#"{
  "planned": [
    {
      "milestone": null,
      "ids": [
        "D05"
      ]
    }
  ],
  "backlog": [
    "D07"
  ]
}
"#,
    );

    workspace
}

#[test]
fn a_draft_is_published_under_the_next_number() {
    let workspace = publishing();

    let output = promote(&workspace.paths, id("D05")).unwrap();

    assert!(!workspace.exists("docs/rfd/drafts/D05-sketch.md"));
    let text = workspace.read("docs/rfd/003-sketch.md");
    assert!(text.starts_with("# RFD 003: Sketch\n"), "{text}");
    assert!(text.contains("- **Status**: Discussion\n"), "{text}");
    assert!(output.warnings.is_empty(), "{:?}", output.warnings);
    assert!(
        output.text.ends_with(
            "docs/rfd/003-sketch.md: Draft -> Discussion (assigned 003)\nUpdated 4 \
             cross-reference(s) in RFDs and tickets.\n"
        ),
        "{}",
        output.text
    );
}

/// The file moves up out of `drafts/`, so its links to published RFDs lose a
/// `../`, and its mentions of its own draft id take the new number.
#[test]
fn the_published_file_links_from_its_new_directory() {
    let workspace = publishing();

    promote(&workspace.paths, id("D05")).unwrap();

    let text = workspace.read("docs/rfd/003-sketch.md");
    assert!(text.contains("This draft, 003, builds on"), "{text}");
    assert!(text.contains("001-process.md"), "{text}");
    assert!(!text.contains("../001-process.md"), "{text}");
}

/// A published RFD never lists a draft, so the draft-to-draft back-link goes,
/// and the RFD it extends gets the back-link a draft never gave it.
#[test]
fn draft_back_links_go_and_published_ones_are_filled_in() {
    let workspace = publishing();

    let output = promote(&workspace.paths, id("D05")).unwrap();

    let text = workspace.read("docs/rfd/003-sketch.md");
    assert!(!text.contains("Required by"), "{text}");
    assert!(
        workspace
            .read("docs/rfd/001-process.md")
            .contains("- **Extended by**: [RFD 003](003-sketch.md)\n")
    );
    assert!(
        output
            .text
            .contains("  backfilled Extended by: RFD 003 into docs/rfd/001-process.md\n"),
        "{}",
        output.text
    );
}

#[test]
fn references_to_the_draft_follow_it_in_every_document() {
    let workspace = publishing();

    let output = promote(&workspace.paths, id("D05")).unwrap();

    assert!(
        workspace
            .read("docs/rfd/002-other.md")
            .contains("See [the sketch](003-sketch.md) and RFD 003.")
    );
    let later = workspace.read("docs/rfd/drafts/D07-later.md");
    assert!(
        later.contains("- **Requires**: [RFD 003](../003-sketch.md)\n"),
        "{later}"
    );
    assert!(later.contains("Builds on 003."), "{later}");

    let ticket = workspace.read("docs/ticket/02wt0kx-sketch-it.md");
    assert!(ticket.contains("- **Promoted to**: 003\n"), "{ticket}");
    assert!(
        ticket.contains("Now [the sketch](../rfd/003-sketch.md), docs/rfd/003-sketch.md."),
        "{ticket}"
    );

    for file in [
        "docs/rfd/002-other.md",
        "docs/rfd/003-sketch.md",
        "docs/rfd/drafts/D07-later.md",
        "docs/ticket/02wt0kx-sketch-it.md",
    ] {
        assert!(
            output
                .text
                .contains(&format!("  updated D05 -> 003 references in {file}\n")),
            "{file}: {}",
            output.text
        );
    }
}

#[test]
fn the_board_keeps_the_place_under_the_new_number() {
    let workspace = publishing();

    promote(&workspace.paths, id("D05")).unwrap();

    let board = workspace.read("docs/rfd/.priority.json");
    assert!(board.contains("\"003\""), "{board}");
    assert!(!board.contains("\"D05\""), "{board}");
    assert!(board.contains("\"D07\""), "{board}");
}

#[test]
fn a_draft_depending_on_a_draft_is_not_published() {
    let workspace = Workspace::new();
    workspace.rfd(
        "D03-first.md",
        &[("Status", "Draft"), ("Requires", "[RFD D04](D04-x.md)")],
        "Body.",
    );

    let error = promote(&workspace.paths, id("D03")).unwrap_err();

    assert_eq!(
        error,
        "Cannot promote: 'Requires' on D03-first.md contains draft references.\n  [RFD \
         D04](D04-x.md)\nPromote those drafts first, or remove the entries."
    );
    assert!(workspace.exists("docs/rfd/drafts/D03-first.md"));
}

/// The docs build rejects a draft id anywhere in a published RFD.
/// Its own id is rewritten, and the draft back-links are dropped, so neither
/// counts.
#[test]
fn a_draft_mentioning_another_draft_is_not_published() {
    let workspace = Workspace::new();
    workspace.rfd(
        "D03-first.md",
        &[("Status", "Draft"), ("Required by", "[RFD D09](D09-x.md)")],
        "D03 is this one. Unlike D04 and D06, it ships. D04 again. XD08 and D081 are not ids.",
    );

    let error = promote(&workspace.paths, id("D03")).unwrap_err();

    assert_eq!(
        error,
        "Cannot promote: D03-first.md references other drafts in its body:\n  RFD D04\n  RFD \
         D06\nPublished RFDs must not reference drafts (the docs build rejects DNN \
         tokens).\nPromote those drafts first, or reword the references."
    );
}

#[test]
fn a_broken_board_file_stops_the_promotion_before_it_writes() {
    let workspace = publishing();
    workspace.board("{ nope");

    let error = promote(&workspace.paths, id("D05")).unwrap_err();

    assert!(
        error
            .ends_with("so the priority board can't be updated. Fix it and run the command again."),
        "{error}"
    );
    assert!(workspace.exists("docs/rfd/drafts/D05-sketch.md"));
    assert!(!workspace.exists("docs/rfd/003-sketch.md"));
}

fn gated() -> Workspace {
    let workspace = Workspace::new();
    workspace.rfd(
        "020-gated.md",
        &[
            ("Status", "Discussion"),
            ("Requires", "[RFD 021](021-x.md), [RFD 099](099-gone.md)"),
            ("Extends", "[RFD 022](022-x.md)"),
        ],
        "Body.",
    );
    workspace.rfd("021-x.md", &[("Status", "Discussion")], "Body.");
    workspace.rfd("022-x.md", &[("Status", "Accepted")], "Body.");

    workspace
}

#[test]
fn acceptance_waits_for_what_it_requires_and_extends() {
    let workspace = gated();

    let error = promote(&workspace.paths, id("020")).unwrap_err();

    assert_eq!(
        error,
        "Cannot promote to Accepted: dependencies not satisfied:\n  RFD 021 (Discussion)\n  RFD \
         099 (not found)\nRequired: status is one of: Accepted, Implemented, Superseded"
    );
    assert!(
        workspace
            .read("docs/rfd/020-gated.md")
            .contains("- **Status**: Discussion\n")
    );
}

#[test]
fn an_rfd_whose_dependencies_are_accepted_is_accepted() {
    let workspace = gated();
    workspace.rfd("021-x.md", &[("Status", "Implemented")], "Body.");
    workspace.rfd("099-gone.md", &[("Status", "Superseded")], "Body.");

    let output = promote(&workspace.paths, id("020")).unwrap();

    assert_eq!(
        output.text,
        "docs/rfd/020-gated.md: Discussion -> Accepted\n"
    );
    assert!(
        workspace
            .read("docs/rfd/020-gated.md")
            .contains("- **Status**: Accepted\n")
    );
}

#[test]
fn implementation_waits_for_what_it_requires_to_be_built() {
    let workspace = Workspace::new();
    workspace.rfd(
        "010-done.md",
        &[("Status", "Accepted"), ("Extends", "[RFD 011](011-x.md)")],
        "Body.",
    );
    workspace.rfd("011-x.md", &[("Status", "Accepted")], "Body.");

    let error = promote(&workspace.paths, id("010")).unwrap_err();

    assert_eq!(
        error,
        "Cannot promote to Implemented: dependencies not satisfied:\n  RFD 011 \
         (Accepted)\nRequired: status is one of: Implemented, Superseded"
    );
}

fn implementing() -> Workspace {
    let workspace = Workspace::new();
    workspace.rfd(
        "001-process.md",
        &[
            ("Status", "Implemented"),
            ("Required by", "[RFD 010](010-done.md), [RFD 012](012-x.md)"),
        ],
        "Body.",
    );
    workspace.rfd(
        "010-done.md",
        &[
            ("Status", "Accepted"),
            ("Requires", "[RFD 001](001-process.md)"),
            ("Required by", "[RFD 011](011-next.md)"),
        ],
        "Body.",
    );
    workspace.rfd(
        "011-next.md",
        &[
            ("Status", "Discussion"),
            (
                "Requires",
                "[RFD 010](010-done.md), [RFD 001](001-process.md)",
            ),
        ],
        "Body.",
    );
    workspace.rfd(
        "D02-draft.md",
        &[
            ("Status", "Draft"),
            ("Requires", "[RFD 010](../010-done.md)"),
        ],
        "Body.",
    );
    workspace
        .board(r#"{"planned": [{"milestone": null, "ids": ["010", "011"]}], "backlog": ["D02"]}"#);

    workspace
}

/// A built dependency gates nothing, so the links go in both directions: this
/// RFD's own, and those of every RFD that required it, drafts included, which
/// its `Required by` never listed.
#[test]
fn implementation_drops_the_requires_links_in_both_directions() {
    let workspace = implementing();

    let output = promote(&workspace.paths, id("010")).unwrap();

    let done = workspace.read("docs/rfd/010-done.md");
    assert!(done.contains("- **Status**: Implemented\n"), "{done}");
    assert!(!done.contains("Requires"), "{done}");
    assert!(!done.contains("Required by"), "{done}");

    assert!(
        workspace
            .read("docs/rfd/001-process.md")
            .contains("- **Required by**: [RFD 012](012-x.md)\n")
    );
    assert!(
        workspace
            .read("docs/rfd/011-next.md")
            .contains("- **Requires**: [RFD 001](001-process.md)\n")
    );
    assert!(
        !workspace
            .read("docs/rfd/drafts/D02-draft.md")
            .contains("Requires")
    );

    assert_eq!(
        output.text,
        "  stripped Required by: RFD 010 from docs/rfd/001-process.md\n  stripped Requires: RFD \
         010 from docs/rfd/011-next.md\n  stripped Requires: RFD 010 from \
         docs/rfd/drafts/D02-draft.md\ndocs/rfd/010-done.md: Accepted -> Implemented\n"
    );
}

#[test]
fn an_implemented_rfd_leaves_the_board() {
    let workspace = implementing();

    promote(&workspace.paths, id("010")).unwrap();

    let board = workspace.read("docs/rfd/.priority.json");
    assert!(!board.contains("\"010\""), "{board}");
    assert!(board.contains("\"011\""), "{board}");
}

#[test]
fn an_rfd_past_accepted_is_not_promoted() {
    let workspace = Workspace::new();
    workspace.rfd("030-x.md", &[("Status", "Implemented")], "Body.");
    workspace.rfd("031-x.md", &[("Status", "Abandoned")], "Body.");

    assert_eq!(
        promote(&workspace.paths, id("030")).unwrap_err(),
        "Cannot promote from 'Implemented'.\nPromotable statuses: Draft, Discussion, Accepted."
    );
    assert_eq!(
        promote(&workspace.paths, id("031")).unwrap_err(),
        "Cannot promote from 'Abandoned'.\nPromotable statuses: Draft, Discussion, Accepted."
    );
}

/// The promoted file is laid out the way CI's markdown check wants it, so the
/// promotion leaves nothing for `fmt-markdown-ci` to report.
#[test]
fn the_promoted_file_is_reflowed() {
    let workspace = Workspace::new();
    workspace.rfd(
        "040-x.md",
        &[("Status", "Discussion")],
        "One sentence. Another on the same line, linking [RFD 001](001-process.md).",
    );

    promote(&workspace.paths, id("040")).unwrap();

    let text = workspace.read("docs/rfd/040-x.md");
    assert!(
        text.contains("One sentence.\nAnother on the same line, linking [RFD 001].\n"),
        "{text}"
    );
    assert!(text.ends_with("[RFD 001]: 001-process.md\n"), "{text}");
}
