use super::*;
use crate::cmd::tests::{Workspace, ticket};

fn id(text: &str) -> RfdId {
    text.parse().unwrap()
}

/// An RFD, and links to its file from an RFD, a ticket, another page of the
/// docs, and code.
fn workspace() -> Workspace {
    let workspace = Workspace::new();
    workspace.rfd("042-ordered-things.md", &[("Status", "Accepted")], "Body.");
    workspace.write(
        "docs/rfd/050-later.md",
        "# RFD 050: Later\n\n- **Status**: Draft\n\n## Summary\n\nSee [RFD \
         042](042-ordered-things.md).\n",
    );
    workspace.write(
        "docs/ticket/02wt0kx-x.md",
        &ticket(
            "X",
            "Todo",
            &[],
            "Per [the RFD](../rfd/042-ordered-things.md).",
        ),
    );
    workspace.write(
        "docs/architecture.md",
        "Ordering is [RFD 042](rfd/042-ordered-things).\n",
    );
    workspace.write("crates/x.rs", "// docs/rfd/042-ordered-things.md\n");

    workspace
}

#[test]
fn a_new_title_moves_the_file_and_its_links() {
    let workspace = workspace();

    let output = rename(&workspace.paths, id("042"), "Kept in Order", true).unwrap();

    assert!(!workspace.exists("docs/rfd/042-ordered-things.md"));
    assert!(
        workspace
            .read("docs/rfd/042-kept-in-order.md")
            .starts_with("# RFD 042: Kept in Order\n\n- **Status**: Accepted\n")
    );
    assert!(
        workspace
            .read("docs/rfd/050-later.md")
            .contains("See [RFD 042](042-kept-in-order.md).")
    );
    assert!(
        workspace
            .read("docs/ticket/02wt0kx-x.md")
            .contains("Per [the RFD](../rfd/042-kept-in-order.md).")
    );
    assert_eq!(
        output.text,
        "  updated link targets in docs/architecture.md\n  updated link targets in \
         docs/rfd/050-later.md\n  updated link targets in \
         docs/ticket/02wt0kx-x.md\n042-ordered-things.md -> \
         docs/rfd/042-kept-in-order.md\nUpdated 3 file(s) with link targets.\n"
    );
}

/// A link without `.md` reaches the same page, so it moves too.
/// Links outside the docs tree are named, not rewritten.
#[test]
fn links_without_md_are_rewritten_and_links_outside_the_docs_reported() {
    let workspace = workspace();

    let output = rename(&workspace.paths, id("042"), "Kept in Order", true).unwrap();

    assert_eq!(
        workspace.read("docs/architecture.md"),
        "Ordering is [RFD 042](rfd/042-kept-in-order).\n"
    );
    let warning = "Warning: references outside docs/ still name 042-ordered-things:\n  crates/x.rs";
    assert_eq!(output.warnings, [warning]);
    assert_eq!(
        workspace.read("crates/x.rs"),
        "// docs/rfd/042-ordered-things.md\n"
    );
}

#[test]
fn links_elsewhere_go_unchecked_when_the_workspace_is_out_of_reach() {
    let workspace = workspace();

    let output = rename(&workspace.paths, id("042"), "Kept in Order", false).unwrap();

    let warning = "References outside docs/ were not checked: the access policy doesn't allow \
                   reading the workspace.";
    assert_eq!(output.warnings, [warning]);
}

/// A longer slug that starts with the old one names another file.
#[test]
fn a_longer_slug_is_left_alone() {
    let workspace = workspace();
    workspace.write(
        "docs/notes.md",
        "[A](rfd/042-ordered-things-revisited.md)\n",
    );

    rename(&workspace.paths, id("042"), "Kept in Order", true).unwrap();

    assert_eq!(
        workspace.read("docs/notes.md"),
        "[A](rfd/042-ordered-things-revisited.md)\n"
    );
}

#[test]
fn a_title_with_the_same_slug_changes_the_heading_only() {
    let workspace = workspace();
    let later = workspace.read("docs/rfd/050-later.md");

    let output = rename(&workspace.paths, id("042"), "Ordered Things!", true).unwrap();

    assert_eq!(
        output.text,
        "042-ordered-things.md -> docs/rfd/042-ordered-things.md\n"
    );
    assert!(
        workspace
            .read("docs/rfd/042-ordered-things.md")
            .starts_with("# RFD 042: Ordered Things!\n")
    );
    assert_eq!(workspace.read("docs/rfd/050-later.md"), later);
}

#[test]
fn a_draft_is_renamed_in_the_drafts_directory() {
    let workspace = workspace();
    workspace.rfd("D05-sketch.md", &[("Status", "Draft")], "Body.");

    rename(&workspace.paths, id("D05"), "Rough Sketch", true).unwrap();

    assert!(workspace.exists("docs/rfd/drafts/D05-rough-sketch.md"));
    assert!(!workspace.exists("docs/rfd/drafts/D05-sketch.md"));
}

/// Tickets kept outside the docs tree still link to RFD files.
#[test]
fn links_in_a_ticket_directory_outside_the_docs_are_rewritten() {
    let workspace = Workspace::new();
    let paths = Paths::new(
        &workspace.paths.root,
        Utf8Path::new("docs/rfd"),
        Utf8Path::new("tickets"),
    );
    workspace.rfd("042-ordered-things.md", &[("Status", "Accepted")], "Body.");
    workspace.write(
        "tickets/02wt0kx-x.md",
        "Per [the RFD](../docs/rfd/042-ordered-things.md).\n",
    );

    let output = rename(&paths, id("042"), "Kept in Order", true).unwrap();

    assert_eq!(
        workspace.read("tickets/02wt0kx-x.md"),
        "Per [the RFD](../docs/rfd/042-kept-in-order.md).\n"
    );
    assert!(output.warnings.is_empty(), "{:?}", output.warnings);
}

#[test]
fn a_title_without_a_slug_or_a_document_without_a_heading_is_refused() {
    let workspace = workspace();
    workspace.write("docs/rfd/043-bare.md", "- **Status**: Draft\n");

    assert_eq!(
        rename(&workspace.paths, id("042"), "!!!", true).unwrap_err(),
        "Title '!!!' produces an empty filename slug."
    );
    assert_eq!(
        rename(&workspace.paths, id("043"), "Titled", true).unwrap_err(),
        "No '# RFD 043: ...' heading in docs/rfd/043-bare.md."
    );
    assert!(workspace.exists("docs/rfd/043-bare.md"));
}
