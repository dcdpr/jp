use super::*;
use crate::cmd::tests::Workspace;

fn id(text: &str) -> RfdId {
    text.parse().unwrap()
}

#[test]
fn an_abandoned_rfd_opens_with_the_reason() {
    let workspace = Workspace::new();
    workspace.rfd(
        "042-idea.md",
        &[("Status", "Discussion"), ("Summary", "An idea.")],
        "Body.",
    );

    let output = abandon(
        &workspace.paths,
        id("042"),
        "Solved differently, by RFD 043.",
    )
    .unwrap();

    assert_eq!(
        output.text,
        "docs/rfd/042-idea.md: Abandoned (Discussion -> Abandoned)\n"
    );
    assert!(output.warnings.is_empty());
    assert_eq!(
        workspace.read("docs/rfd/042-idea.md"),
        "\
# RFD 042: Idea

- **Status**: Abandoned
- **Summary**: An idea.

> [!IMPORTANT]
> This RFD is **Abandoned**.
>
> Solved differently, by RFD 043.
>
> The original text below is preserved for historical context.

## Summary

Body.
"
    );
}

/// RFD 001: a draft the author won't pursue can be abandoned when the reason is
/// worth keeping.
#[test]
fn a_draft_can_be_abandoned() {
    let workspace = Workspace::new();
    workspace.rfd("D05-sketch.md", &[("Status", "Draft")], "Body.");
    workspace.board(r#"{"planned": [{"milestone": null, "ids": []}], "backlog": ["D05", "D06"]}"#);

    abandon(&workspace.paths, id("D05"), "Not needed.").unwrap();

    assert!(
        workspace
            .read("docs/rfd/drafts/D05-sketch.md")
            .contains("- **Status**: Abandoned\n")
    );
    let board = workspace.read("docs/rfd/.priority.json");
    assert!(!board.contains("\"D05\""), "{board}");
    assert!(board.contains("\"D06\""), "{board}");
}

#[test]
fn a_reason_on_several_lines_stays_in_the_notice() {
    let workspace = Workspace::new();
    workspace.rfd("042-idea.md", &[("Status", "Accepted")], "Body.");

    abandon(
        &workspace.paths,
        id("042"),
        "Split in two:\n\n- RFD 043\n- RFD 044",
    )
    .unwrap();

    assert!(workspace.read("docs/rfd/042-idea.md").contains(
        "> This RFD is **Abandoned**.\n>\n> Split in two:\n>\n> - RFD 043\n> - RFD 044\n>\n> The \
         original"
    ));
}

/// Nothing cascades: the RFDs that depended on this one need someone to decide
/// what they depend on now.
#[test]
fn the_rfds_that_depended_on_it_are_named() {
    let workspace = Workspace::new();
    workspace.rfd(
        "042-idea.md",
        &[
            ("Status", "Accepted"),
            ("Required by", "[RFD 050](050-x.md), [RFD 051](051-y.md)"),
        ],
        "Body.",
    );

    let output = abandon(&workspace.paths, id("042"), "Dropped.").unwrap();

    let warning = "Warning: the following RFDs depend on this one (Required by):\n  RFD 050\n  \
                   RFD 051\nTheir dependency on RFD 042 is now broken; review and update.";
    assert_eq!(output.warnings, [warning]);
}

#[test]
fn a_retired_tracking_issue_is_a_reminder() {
    let workspace = Workspace::new();
    workspace.rfd(
        "042-idea.md",
        &[
            ("Status", "Accepted"),
            (
                "Tracking Issue",
                "[#437](https://github.com/dcdpr/jp/issues/437)",
            ),
        ],
        "Body.",
    );

    let output = abandon(&workspace.paths, id("042"), "Dropped.").unwrap();

    assert_eq!(
        output.text,
        "docs/rfd/042-idea.md: Abandoned (Accepted -> Abandoned)\nRemember to close the tracking \
         issue: https://github.com/dcdpr/jp/issues/437\n"
    );
}

#[test]
fn a_finished_rfd_is_not_abandoned() {
    let workspace = Workspace::new();
    for (name, status) in [
        ("042-a.md", "Implemented"),
        ("043-b.md", "Superseded"),
        ("044-c.md", "Abandoned"),
    ] {
        workspace.rfd(name, &[("Status", status)], "Body.");
    }

    for (rfd, status) in [
        ("042", "Implemented"),
        ("043", "Superseded"),
        ("044", "Abandoned"),
    ] {
        assert_eq!(
            abandon(&workspace.paths, id(rfd), "Why.").unwrap_err(),
            format!("Cannot abandon from '{status}'.")
        );
    }
}

#[test]
fn an_empty_reason_is_refused() {
    let workspace = Workspace::new();
    workspace.rfd("042-idea.md", &[("Status", "Accepted")], "Body.");

    assert_eq!(
        abandon(&workspace.paths, id("042"), "  ").unwrap_err(),
        "Give the reason RFD 042 is abandoned."
    );
}
