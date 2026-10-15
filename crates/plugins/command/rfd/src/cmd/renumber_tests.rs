use super::*;
use crate::cmd::tests::{Workspace, ticket};

/// A draft, the documents that know it by its id, and a mention outside the
/// directories a renumber rewrites.
fn workspace() -> Workspace {
    let workspace = Workspace::new();
    workspace.rfd(
        "D05-sketch.md",
        &[("Status", "Draft")],
        "D05 sketches it; see [the other](D06-other.md).",
    );
    workspace.rfd(
        "D06-other.md",
        &[
            ("Status", "Draft"),
            ("Requires", "[RFD D05](D05-sketch.md)"),
        ],
        "After D05, not D056.",
    );
    workspace.rfd(
        "042-published.md",
        &[("Status", "Accepted")],
        "Mentions RFD 042 itself, and 042 as a bare number.",
    );
    workspace.write(
        "docs/ticket/02wt0kx-sketch.md",
        &ticket(
            "Sketch",
            "Done",
            &[("Promoted to", "D05")],
            "See [it](../rfd/drafts/D05-sketch.md).",
        ),
    );
    workspace.write("crates/notes.rs", "// RFD D05 decides this.\n");
    workspace.board(r#"{"planned": [{"milestone": null, "ids": ["042"]}], "backlog": ["D05"]}"#);

    workspace
}

#[test]
fn a_draft_moves_to_the_slot_given() {
    let workspace = workspace();

    let output = renumber(&workspace.paths, "D05", Some("D07"), true).unwrap();

    assert!(!workspace.exists("docs/rfd/drafts/D05-sketch.md"));
    assert_eq!(
        workspace.read("docs/rfd/drafts/D07-sketch.md"),
        "# RFD D07: Sketch\n\n- **Status**: Draft\n\n## Summary\n\nD07 sketches it; see [the \
         other](D06-other.md).\n"
    );
    assert_eq!(
        output.text,
        "  updated D05 -> D07 references in docs/rfd/drafts/D06-other.md\n  updated D05 -> D07 \
         references in docs/rfd/drafts/D07-sketch.md\n  updated D05 -> D07 references in \
         docs/ticket/02wt0kx-sketch.md\nD05-sketch.md -> docs/rfd/drafts/D07-sketch.md (D05 -> \
         D07)\nUpdated 3 file(s) with cross-references.\n"
    );
}

#[test]
fn references_to_a_draft_follow_it() {
    let workspace = workspace();

    renumber(&workspace.paths, "D05", Some("d7"), true).unwrap();

    let other = workspace.read("docs/rfd/drafts/D06-other.md");
    assert!(
        other.contains("- **Requires**: [RFD D07](D07-sketch.md)\n"),
        "{other}"
    );
    assert!(other.contains("After D07, not D056."), "{other}");

    let ticket = workspace.read("docs/ticket/02wt0kx-sketch.md");
    assert!(ticket.contains("- **Promoted to**: D07\n"), "{ticket}");
    assert!(
        ticket.contains("See [it](../rfd/drafts/D07-sketch.md)."),
        "{ticket}"
    );

    let board = workspace.read("docs/rfd/.priority.json");
    assert!(board.contains("\"D07\""), "{board}");
    assert!(!board.contains("\"D05\""), "{board}");
}

/// Mentions outside the RFDs and the tickets are reported, not rewritten.
#[test]
fn mentions_elsewhere_are_reported() {
    let workspace = workspace();

    let output = renumber(&workspace.paths, "D05", Some("D07"), true).unwrap();

    let warning = "Warning: references outside docs/rfd/ and docs/ticket/ still mention D05:\n  \
                   crates/notes.rs";
    assert_eq!(output.warnings, [warning]);
    assert_eq!(
        workspace.read("crates/notes.rs"),
        "// RFD D05 decides this.\n"
    );
}

#[test]
fn mentions_elsewhere_go_unchecked_when_the_workspace_is_out_of_reach() {
    let workspace = workspace();

    let output = renumber(&workspace.paths, "D05", Some("D07"), false).unwrap();

    let warning = "References outside docs/rfd/ and docs/ticket/ were not checked: the access \
                   policy doesn't allow reading the workspace.";
    assert_eq!(output.warnings, [warning]);
}

#[test]
fn without_a_target_the_next_free_id_is_taken() {
    let workspace = workspace();

    let output = renumber(&workspace.paths, "D06", None, true).unwrap();

    assert!(
        output
            .text
            .contains("D06-other.md -> docs/rfd/drafts/D01-other.md (D06 -> D01)\n"),
        "{}",
        output.text
    );

    let output = renumber(&workspace.paths, "042", None, true).unwrap();

    assert!(
        output
            .text
            .contains("042-published.md -> docs/rfd/001-published.md (042 -> 001)\n"),
        "{}",
        output.text
    );
}

/// A bare `042` is too common a token to rewrite; `RFD 042` and the file name
/// are not.
#[test]
fn a_published_rfd_rewrites_its_citations_and_file_name_only() {
    let workspace = workspace();
    workspace.write(
        "docs/rfd/050-later.md",
        "# RFD 050: Later\n\n- **Status**: Discussion\n- **Requires**: [RFD \
         042](042-published.md)\n\n## Summary\n\nBuilt on RFD 042, on 042.\n",
    );

    renumber(&workspace.paths, "42", Some("97"), true).unwrap();

    assert_eq!(
        workspace.read("docs/rfd/097-published.md"),
        "# RFD 097: Published\n\n- **Status**: Accepted\n\n## Summary\n\nMentions RFD 097 itself, \
         and 042 as a bare number.\n"
    );
    assert!(
        workspace
            .read("docs/rfd/050-later.md")
            .contains("[RFD 097](097-published.md)\n\n## Summary\n\nBuilt on RFD 097, on 042.\n")
    );
    let board = workspace.read("docs/rfd/.priority.json");
    assert!(board.contains("\"097\""), "{board}");
}

#[test]
fn a_target_has_to_be_a_free_id_in_the_same_space() {
    let workspace = workspace();

    for (rfd, target, error) in [
        (
            "D05",
            "042",
            "Target for a draft must be a draft slot (D01-D99), got '042'.",
        ),
        (
            "042",
            "D07",
            "Target for a published RFD must be a number, got 'D07'.",
        ),
        ("D05", "D00", "Target id must be greater than zero."),
        ("042", "000", "Target id must be greater than zero."),
        (
            "042",
            "1000",
            "Target for a published RFD must be at most 999, got '1000'.",
        ),
        ("D05", "D5", "RFD D05 already has that id; nothing to do."),
        ("D05", "D06", "Target id D06 is taken by D06-other.md."),
    ] {
        assert_eq!(
            renumber(&workspace.paths, rfd, Some(target), true).unwrap_err(),
            error,
            "{rfd} -> {target}"
        );
    }

    assert!(workspace.exists("docs/rfd/drafts/D05-sketch.md"));
}

/// After a merge two files can share an id.
/// A bare id names neither, so the file name picks one, and only what can only
/// mean that one is rewritten.
#[test]
fn a_file_name_picks_one_of_two_documents_sharing_an_id() {
    let workspace = workspace();
    workspace.rfd("D05-rival.md", &[("Status", "Draft")], "Also D05.");
    workspace.write(
        "docs/rfd/drafts/D08-links.md",
        "# RFD D08: Links\n\n- **Status**: Draft\n\n## Summary\n\nSee [rival](D05-rival.md) and \
         RFD D05.\n",
    );

    let error = renumber(&workspace.paths, "D05", Some("D07"), true).unwrap_err();
    assert!(
        error.starts_with("Ambiguous: 2 files share ID D05:"),
        "{error}"
    );

    let output = renumber(
        &workspace.paths,
        "docs/rfd/drafts/D05-rival",
        Some("D07"),
        true,
    )
    .unwrap();

    assert_eq!(
        workspace.read("docs/rfd/drafts/D07-rival.md"),
        "# RFD D07: Rival\n\n- **Status**: Draft\n\n## Summary\n\nAlso D07.\n"
    );
    assert!(
        workspace
            .read("docs/rfd/drafts/D08-links.md")
            .contains("See [rival](D07-rival.md) and RFD D05.\n")
    );
    assert!(
        workspace
            .read("docs/rfd/.priority.json")
            .contains("\"D05\"")
    );
    assert_eq!(
        output.warnings[0],
        "Warning: D05 is still held by:\n  docs/rfd/drafts/D05-sketch.md\nMentions of \"RFD D05\" \
         outside D07-rival.md, and a board entry for D05, were left alone: they name either \
         document. Check them by hand."
    );
}

#[test]
fn a_file_name_is_found_in_either_directory() {
    let workspace = workspace();

    renumber(&workspace.paths, "D05-sketch.md", Some("D07"), true).unwrap();
    renumber(&workspace.paths, "042-published", Some("097"), true).unwrap();

    assert!(workspace.exists("docs/rfd/drafts/D07-sketch.md"));
    assert!(workspace.exists("docs/rfd/097-published.md"));
}

#[test]
fn templates_and_misplaced_files_are_not_renumbered() {
    let workspace = workspace();
    workspace.write("docs/rfd/000-design-template.md", "# RFD NNN: TITLE\n");
    workspace.write("docs/rfd/D09-misplaced.md", "# RFD D09: Misplaced\n");

    assert_eq!(
        renumber(&workspace.paths, "000-design-template.md", None, true).unwrap_err(),
        "Refusing to renumber a template."
    );
    assert_eq!(
        renumber(&workspace.paths, "D09-misplaced.md", None, true).unwrap_err(),
        "docs/rfd/D09-misplaced.md is not in docs/rfd/drafts, where RFD D09 belongs."
    );
    assert_eq!(
        renumber(&workspace.paths, "no-such-file.md", None, true).unwrap_err(),
        "No RFD file matching 'no-such-file.md'."
    );
}
