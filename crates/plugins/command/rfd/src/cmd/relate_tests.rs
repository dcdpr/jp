use super::*;
use crate::cmd::tests::Workspace;

fn id(text: &str) -> RfdId {
    text.parse().unwrap()
}

fn workspace() -> Workspace {
    let workspace = Workspace::new();
    workspace.rfd("072-plugins.md", &[("Status", "Implemented")], "Body.");
    workspace.rfd("100-tickets.md", &[("Status", "Implemented")], "Body.");
    workspace.rfd("116-tooling.md", &[("Status", "Accepted")], "Body.");
    workspace.rfd("D05-sketch.md", &[("Status", "Draft")], "Body.");
    workspace.rfd("D06-other.md", &[("Status", "Draft")], "Body.");

    workspace
}

/// `extend` takes the RFD that extends first, the same order as `require`.
#[test]
fn extend_records_the_relationship_in_both_documents() {
    let workspace = workspace();

    let output = extend(&workspace.paths, id("116"), id("100")).unwrap();

    assert_eq!(
        output.text,
        "docs/rfd/116-tooling.md: Extends: RFD 100\ndocs/rfd/100-tickets.md: Extended by: RFD \
         116\n"
    );
    assert!(
        workspace
            .read("docs/rfd/116-tooling.md")
            .contains("- **Extends**: [RFD 100](100-tickets.md)\n")
    );
    assert!(
        workspace
            .read("docs/rfd/100-tickets.md")
            .contains("- **Extended by**: [RFD 116](116-tooling.md)\n")
    );
}

#[test]
fn require_records_the_relationship_in_both_documents() {
    let workspace = workspace();

    let output = require(&workspace.paths, id("116"), id("72")).unwrap();

    assert_eq!(
        output.text,
        "docs/rfd/116-tooling.md: Requires: RFD 072\ndocs/rfd/072-plugins.md: Required by: RFD \
         116\n"
    );
    assert!(
        workspace
            .read("docs/rfd/116-tooling.md")
            .contains("- **Requires**: [RFD 072](072-plugins.md)\n")
    );
    assert!(
        workspace
            .read("docs/rfd/072-plugins.md")
            .contains("- **Required by**: [RFD 116](116-tooling.md)\n")
    );
}

#[test]
fn a_relationship_already_recorded_is_reported() {
    let workspace = workspace();
    require(&workspace.paths, id("116"), id("072")).unwrap();
    let before = workspace.read("docs/rfd/116-tooling.md");

    let output = require(&workspace.paths, id("116"), id("072")).unwrap();

    assert_eq!(
        output.text,
        "docs/rfd/116-tooling.md: already lists RFD 072 under Requires\ndocs/rfd/072-plugins.md: \
         already lists RFD 116 under Required by\n"
    );
    assert_eq!(workspace.read("docs/rfd/116-tooling.md"), before);
}

/// A draft may depend on a published RFD, but the published one gets its
/// back-link only once the draft is promoted.
#[test]
fn a_draft_depending_on_a_published_rfd_leaves_it_alone() {
    let workspace = workspace();
    let published = workspace.read("docs/rfd/072-plugins.md");

    let output = require(&workspace.paths, id("D05"), id("072")).unwrap();

    assert_eq!(
        output.text,
        "docs/rfd/drafts/D05-sketch.md: Requires: RFD 072\ndocs/rfd/072-plugins.md: skipped \
         Required by: RFD D05 (suppressed: draft -> non-draft)\n"
    );
    assert!(
        workspace
            .read("docs/rfd/drafts/D05-sketch.md")
            .contains("- **Requires**: [RFD 072](../072-plugins.md)\n")
    );
    assert_eq!(workspace.read("docs/rfd/072-plugins.md"), published);
}

#[test]
fn drafts_link_to_each_other_both_ways() {
    let workspace = workspace();

    extend(&workspace.paths, id("D05"), id("D06")).unwrap();

    assert!(
        workspace
            .read("docs/rfd/drafts/D05-sketch.md")
            .contains("- **Extends**: [RFD D06](D06-other.md)\n")
    );
    assert!(
        workspace
            .read("docs/rfd/drafts/D06-other.md")
            .contains("- **Extended by**: [RFD D05](D05-sketch.md)\n")
    );
}

#[test]
fn a_published_rfd_never_links_to_a_draft() {
    let workspace = workspace();

    let error = require(&workspace.paths, id("116"), id("D05")).unwrap_err();

    assert_eq!(
        error,
        "Refused: published RFD 116 cannot link to draft RFD D05.\nPromote D05 first, or move the \
         relationship to a draft."
    );
}

/// Extension implies the dependency, so one target is listed under one of the
/// two only.
#[test]
fn a_target_is_not_listed_under_both_requires_and_extends() {
    let workspace = workspace();
    require(&workspace.paths, id("116"), id("100")).unwrap();

    let error = extend(&workspace.paths, id("116"), id("100")).unwrap_err();

    assert_eq!(
        error,
        "Refused: RFD 116 already lists RFD 100 under 'Requires'.\nExtension implies dependency; \
         don't list the same target under both. Drop one entry first."
    );
}

/// `Requires` and `Extends` are one graph for the promotion gate, so a loop
/// through either is refused.
#[test]
fn a_link_closing_a_cycle_is_refused() {
    let workspace = workspace();
    require(&workspace.paths, id("116"), id("072")).unwrap();
    extend(&workspace.paths, id("072"), id("100")).unwrap();

    let error = extend(&workspace.paths, id("100"), id("116")).unwrap_err();

    assert_eq!(
        error,
        "Refused: cycle detected. Adding RFD 100 -> RFD 116 on 'Extends' would close a loop in \
         the Requires/Extends graph."
    );
    assert_eq!(
        require(&workspace.paths, id("116"), id("116")).unwrap_err(),
        "Refused: cycle detected. Adding RFD 116 -> RFD 116 on 'Requires' would close a loop in \
         the Requires/Extends graph."
    );
}

#[test]
fn a_withdrawn_or_replaced_rfd_is_not_extended() {
    let workspace = workspace();
    workspace.rfd("033-old.md", &[("Status", "Superseded")], "Body.");

    let error = extend(&workspace.paths, id("116"), id("033")).unwrap_err();

    assert_eq!(
        error,
        "Cannot extend RFD 033 (status: 'Superseded').\nAbandoned and Superseded RFDs cannot be \
         extended."
    );
}

#[test]
fn an_rfd_that_does_not_exist_is_an_error() {
    let workspace = workspace();

    assert_eq!(
        require(&workspace.paths, id("116"), id("099")).unwrap_err(),
        "No RFD found with number 099."
    );
}

fn superseding() -> Workspace {
    let workspace = Workspace::new();
    workspace.rfd(
        "033-old.md",
        &[
            ("Status", "Accepted"),
            ("Date", "2026-03-07"),
            ("Summary", "The old way."),
        ],
        "Body.",
    );
    workspace.rfd("034-new.md", &[("Status", "Discussion")], "Body.");
    workspace.board(r#"{"planned": [{"milestone": null, "ids": ["033", "034"]}], "backlog": []}"#);

    workspace
}

#[test]
fn supersede_records_the_replacement_in_both_documents() {
    let workspace = superseding();

    let output = supersede(&workspace.paths, id("033"), id("034")).unwrap();

    assert_eq!(
        output.text,
        "docs/rfd/033-old.md: Superseded by RFD 034\ndocs/rfd/034-new.md: Supersedes RFD 033\n"
    );
    assert_eq!(
        workspace.read("docs/rfd/033-old.md"),
        "# RFD 033: Old\n\n- **Status**: Superseded\n- **Date**: 2026-03-07\n- **Superseded by**: \
         [RFD 034](034-new.md)\n- **Summary**: The old way.\n\n## Summary\n\nBody.\n"
    );
    assert!(
        workspace
            .read("docs/rfd/034-new.md")
            .contains("- **Supersedes**: [RFD 033](033-old.md)\n")
    );
}

/// The replacement carries the work, and whether it belongs on the board is for
/// whoever ranks it.
#[test]
fn a_superseded_rfd_leaves_the_board() {
    let workspace = superseding();

    supersede(&workspace.paths, id("033"), id("034")).unwrap();

    let board = workspace.read("docs/rfd/.priority.json");
    assert!(!board.contains("\"033\""), "{board}");
    assert!(board.contains("\"034\""), "{board}");
}

#[test]
fn only_an_accepted_or_implemented_rfd_is_superseded() {
    let workspace = superseding();
    workspace.rfd("035-x.md", &[("Status", "Discussion")], "Body.");

    assert_eq!(
        supersede(&workspace.paths, id("035"), id("034")).unwrap_err(),
        "Cannot supersede from 'Discussion'.\nOnly Accepted or Implemented RFDs can be superseded."
    );
}

/// RFD 001: a draft replaced before it is published is deleted or abandoned.
#[test]
fn drafts_take_no_part_in_superseding() {
    let workspace = superseding();
    workspace.rfd("D05-sketch.md", &[("Status", "Draft")], "Body.");

    assert_eq!(
        supersede(&workspace.paths, id("D05"), id("034")).unwrap_err(),
        "Cannot supersede RFD D05: a draft is deleted or abandoned, not superseded."
    );
    assert_eq!(
        supersede(&workspace.paths, id("033"), id("D05")).unwrap_err(),
        "Cannot supersede RFD 033 with RFD D05: promote the draft first."
    );
    assert_eq!(
        supersede(&workspace.paths, id("033"), id("033")).unwrap_err(),
        "RFD 033 cannot supersede itself."
    );
}

#[test]
fn a_retired_tracking_issue_is_a_reminder() {
    let workspace = superseding();
    workspace.write(
        "docs/rfd/033-old.md",
        "# RFD 033: Old\n\n- **Status**: Implemented\n- **Tracking Issue**: [\\#437]\n\n## \
         Summary\n\n[\\#437]: https://github.com/dcdpr/jp/issues/437\n",
    );

    let output = supersede(&workspace.paths, id("033"), id("034")).unwrap();

    assert!(
        output.text.ends_with(
            "Remember to close the superseded tracking issue: \
             https://github.com/dcdpr/jp/issues/437\n"
        ),
        "{}",
        output.text
    );
}
