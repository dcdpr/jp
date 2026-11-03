use std::fs;

use super::*;
use crate::{cmd::tests::Workspace, priority};

fn id(text: &str) -> RfdId {
    text.parse().unwrap()
}

/// Open RFDs on a board with one milestone, and one that is done.
/// RFD 053 requires RFD 044, so 044 has to stay above it.
fn workspace() -> Workspace {
    let workspace = Workspace::new();
    workspace.rfd("044-base.md", &[("Status", "Accepted")], "Body.");
    workspace.rfd("050-other.md", &[("Status", "Discussion")], "Body.");
    workspace.rfd(
        "053-built-on-base.md",
        &[
            ("Status", "Accepted"),
            ("Requires", "[RFD 044](044-base.md)"),
        ],
        "Body.",
    );
    workspace.rfd("060-later.md", &[("Status", "Discussion")], "Body.");
    workspace.rfd("061-unplaced.md", &[("Status", "Discussion")], "Body.");
    workspace.rfd("070-done.md", &[("Status", "Implemented")], "Body.");
    workspace.rfd("D05-sketch.md", &[("Status", "Draft")], "Body.");
    workspace.board(
        r#"{
  "planned": [
    { "milestone": "v0.1", "ids": ["044", "050"] },
    { "milestone": null, "ids": ["053"] }
  ],
  "backlog": ["060", "D05"]
}"#,
    );

    workspace
}

/// A planned group: its milestone and its ids.
type Group = (Option<String>, Vec<String>);

/// The board, as planned groups and backlog.
fn board(workspace: &Workspace) -> (Vec<Group>, Vec<String>) {
    let priority = priority::read(&workspace.paths.rfds).unwrap().unwrap();
    let planned = priority
        .planned
        .into_iter()
        .map(|group| (group.milestone, group.ids))
        .collect();

    (planned, priority.backlog)
}

fn ids(list: &[&str]) -> Vec<String> {
    list.iter().map(|id| (*id).to_owned()).collect()
}

#[test]
fn before_puts_an_rfd_in_the_others_milestone_above_it() {
    let workspace = workspace();

    let output = rank(&workspace.paths, id("060"), Place::Before(id("050"))).unwrap();

    assert_eq!(
        output.text,
        "RFD 060 now sits above RFD 050, in the v0.1 milestone.\n"
    );
    assert_eq!(
        board(&workspace),
        (
            vec![
                (Some("v0.1".to_owned()), ids(&["044", "060", "050"])),
                (None, ids(&["053"])),
            ],
            ids(&["D05"])
        )
    );
}

/// A backlog RFD ranked next to a planned one becomes planned.
#[test]
fn after_puts_an_rfd_below_the_other() {
    let workspace = workspace();

    let output = rank(&workspace.paths, id("D05"), Place::After(id("053"))).unwrap();

    assert_eq!(
        output.text,
        "RFD D05 now sits below RFD 053, among the planned RFDs.\n"
    );
    assert_eq!(
        board(&workspace),
        (
            vec![
                (Some("v0.1".to_owned()), ids(&["044", "050"])),
                (None, ids(&["053", "D05"])),
            ],
            ids(&["060"])
        )
    );
}

#[test]
fn backlog_moves_an_rfd_to_the_top_of_the_backlog() {
    let workspace = workspace();

    let output = rank(&workspace.paths, id("050"), Place::Backlog).unwrap();

    assert_eq!(output.text, "RFD 050 is in the backlog.\n");
    assert_eq!(board(&workspace).1, ids(&["050", "060", "D05"]));
}

/// The top of the backlog would put an RFD above what it depends on, so it goes
/// below the last of those instead.
#[test]
fn backlog_keeps_an_rfd_below_what_it_depends_on() {
    let workspace = workspace();
    workspace.rfd(
        "D05-sketch.md",
        &[
            ("Status", "Draft"),
            ("Requires", "[RFD 060](../060-later.md)"),
        ],
        "Body.",
    );
    workspace.board(r#"{"planned": [], "backlog": ["061", "060"]}"#);

    rank(&workspace.paths, id("D05"), Place::Backlog).unwrap();

    assert_eq!(board(&workspace).1, ids(&["061", "060", "D05"]));
}

/// The site's board refuses a drop that puts an RFD above what it depends on,
/// and so does this.
#[test]
fn a_move_above_a_dependency_is_refused() {
    let workspace = workspace();
    let before = workspace.read("docs/rfd/.priority.json");

    assert_eq!(
        rank(&workspace.paths, id("053"), Place::Before(id("044"))).unwrap_err(),
        "RFD 053 depends on RFD 044, which would sit below it. Rank RFD 044 above it first."
    );
    assert_eq!(
        rank(&workspace.paths, id("044"), Place::After(id("053"))).unwrap_err(),
        "RFD 053 depends on RFD 044, which would sit below it. Rank RFD 053 below it first."
    );
    assert_eq!(workspace.read("docs/rfd/.priority.json"), before);
}

#[test]
fn rfds_off_the_board_are_refused() {
    let workspace = workspace();

    for (rfd, place, error) in [
        (
            "070",
            Place::Backlog,
            "RFD 070 is Implemented, and the board ranks open work only.",
        ),
        (
            "060",
            Place::Before(id("070")),
            "RFD 070 is Implemented, and holds no place on the board.",
        ),
        (
            "060",
            Place::After(id("061")),
            "RFD 061 is not on the board. Place it first, with `jp rfd rank 061 --backlog`.",
        ),
        (
            "060",
            Place::Before(id("060")),
            "RFD 060 can't be ranked against itself.",
        ),
    ] {
        assert_eq!(
            rank(&workspace.paths, id(rfd), place).unwrap_err(),
            error,
            "{rfd} {place:?}"
        );
    }

    assert_eq!(
        rank(&workspace.paths, id("099"), Place::Backlog).unwrap_err(),
        "No RFD found with number 099."
    );
}

#[test]
fn a_board_file_is_written_when_there_is_none() {
    let workspace = workspace();
    fs::remove_file(workspace.paths.rfds.join(".priority.json")).unwrap();

    rank(&workspace.paths, id("044"), Place::Backlog).unwrap();

    assert_eq!(
        workspace.read("docs/rfd/.priority.json"),
        "{\n  \"planned\": [\n    {\n      \"milestone\": null,\n      \"ids\": []\n    }\n  ],\n  \
         \"backlog\": [\n    \"044\"\n  ]\n}\n"
    );
}

#[test]
fn a_broken_board_file_is_left_alone() {
    let workspace = workspace();
    workspace.board("{ nope");

    let error = rank(&workspace.paths, id("044"), Place::Backlog).unwrap_err();

    assert!(error.contains("is not valid JSON"), "{error}");
    assert_eq!(workspace.read("docs/rfd/.priority.json"), "{ nope");
}
