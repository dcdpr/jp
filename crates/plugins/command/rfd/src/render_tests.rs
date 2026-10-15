use std::collections::BTreeSet;

use serde_json::{Value, json};

use super::*;
use crate::{board::Source, priority::Priority};

fn source(name: &str, fields: &[(&str, &str)]) -> Source {
    let (id, slug) = name.split_once('-').unwrap();
    let title = slug.strip_suffix(".md").unwrap();
    let title = title[..1].to_uppercase() + &title[1..];

    let mut content = format!("# RFD {id}: {title}\n\n");
    for (key, value) in fields {
        content.push_str(&format!("- **{key}**: {value}\n"));
    }
    content.push_str("\n## Summary\n\nBody.\n");

    let file = if name.starts_with('D') {
        format!("docs/rfd/drafts/{name}")
    } else {
        format!("docs/rfd/{name}")
    };

    Source {
        name: name.to_owned(),
        file,
        content,
    }
}

/// Every section populated: two planned RFDs in a milestone and one below it,
/// one in the backlog, and two off the board.
fn board() -> Board {
    let published = [
        source("001-process.md", &[
            ("Status", "Implemented"),
            ("Category", "Process"),
        ]),
        source("010-pty.md", &[
            ("Status", "Accepted"),
            ("Category", "Design"),
        ]),
        source("014-guide.md", &[
            ("Status", "Superseded"),
            ("Category", "Guide"),
            ("Superseded by", "[RFD 015]"),
        ]),
        source("044-init.md", &[
            ("Status", "Accepted"),
            ("Category", "Design"),
            ("Requires", "[RFD 001]"),
            ("Summary", "Initializes workspaces."),
        ]),
        source("053-titles.md", &[
            ("Status", "Discussion"),
            ("Category", "Design"),
        ]),
    ];
    let drafts = [source("D50-compaction.md", &[
        ("Status", "Draft"),
        ("Category", "Design"),
        ("Extends", "[RFD 044]"),
    ])];
    let priority = Priority::normalize(&json!({
        "planned": [
            { "milestone": "v0.1", "ids": ["044", "D50"] },
            { "milestone": null, "ids": ["053"] }
        ],
        "backlog": ["010"]
    }));

    Board::assemble(
        &published,
        &drafts,
        priority,
        &BTreeSet::from(["044".to_owned()]),
    )
}

fn entry<'a>(board: &'a Board, num: &str) -> &'a Entry {
    board.entries.iter().find(|entry| entry.num == num).unwrap()
}

#[test]
fn no_section_named_lists_the_planned_rfds() {
    assert_eq!(Sections::from_flags(false, false, false), Sections {
        planned: true,
        backlog: false,
        other: false,
    });
    assert_eq!(Sections::from_flags(false, true, false), Sections {
        planned: false,
        backlog: true,
        other: false,
    });
    assert_eq!(Sections::from_flags(false, false, true), Sections {
        planned: true,
        backlog: true,
        other: true,
    });
}

#[test]
fn the_planned_list_marks_milestones_and_work_in_development() {
    let out = list_text(
        &board(),
        None,
        Sections::from_flags(false, false, false),
        false,
    );

    assert_eq!(
        out,
        "\
--planned (3), --backlog (1), --all (6), --full

Planned (priority order, * in development)

  1  *  044  Accepted    Init
  2     D50  Draft       Compaction
        ─── v0.1 milestone ───
  3     053  Discussion  Titles
"
    );
}

/// The status column widens for the longest status shown, which only an RFD a
/// later one replaced reaches.
#[test]
fn every_section_lists_with_one_status_width() {
    let out = list_text(
        &board(),
        None,
        Sections::from_flags(false, false, true),
        false,
    );

    assert_eq!(
        out,
        "\
--planned (3), --backlog (1), --all (6), --full

Planned (priority order, * in development)

  1  *  044  Accepted          Init
  2     D50  Draft             Compaction
        ─── v0.1 milestone ───
  3     053  Discussion        Titles

Backlog (not yet ranked)

        010  Accepted          Pty

Other (implemented, terminal, or unplaced)

        001  Implemented       Process
        014  Superseded (015)  Guide
"
    );
}

#[test]
fn full_adds_summaries_and_dependencies() {
    let out = list_text(
        &board(),
        None,
        Sections::from_flags(true, false, false),
        true,
    );

    assert_eq!(
        out,
        "\
--planned (3), --backlog (1), --all (6)

Planned (priority order, * in development)

  1  *  044  Accepted    Init
         Initializes workspaces.
         needs: 001
  2     D50  Draft       Compaction
         needs: 044
        ─── v0.1 milestone ───
  3     053  Discussion  Titles
"
    );
}

/// The category matches regardless of case, ranks count the RFDs shown, and the
/// counts in the first line follow the filter.
#[test]
fn a_category_narrows_every_section() {
    let out = list_text(
        &board(),
        Some("GUIDE"),
        Sections::from_flags(false, false, true),
        false,
    );

    assert_eq!(
        out,
        "\
--planned (0), --backlog (0), --all (1), --full

Other (implemented, terminal, or unplaced)

        014  Superseded (015)  Guide
"
    );
}

#[test]
fn a_listing_with_nothing_to_show_says_so() {
    let board = board();

    assert_eq!(
        list_text(
            &board,
            Some("guide"),
            Sections::from_flags(false, false, false),
            false
        ),
        "No matching RFDs.\n"
    );
    assert_eq!(
        list_text(
            &board,
            Some("unknown"),
            Sections::from_flags(false, false, true),
            false
        ),
        "No matching RFDs.\n"
    );
}

/// The JSON listing is what a script reads, so its shape is pinned whole: the
/// keys, their order, and the tags.
#[test]
fn the_json_listing_tags_every_rfd() {
    let published = [Source {
        name: "042-ordered.md".to_owned(),
        file: "docs/rfd/042-ordered.md".to_owned(),
        content: "\
# RFD 042: Ordered

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz
- **Date**: 2026-03-07
- **Requires**: [RFD 001](001-jp-rfd-process.md)
- **Summary**: Keeps order.
"
        .to_owned(),
    }];
    let priority = Priority::normalize(&json!({
        "planned": [{ "milestone": "First", "ids": ["042"] }]
    }));
    let board = Board::assemble(
        &published,
        &[],
        priority,
        &BTreeSet::from(["042".to_owned()]),
    );

    assert_eq!(
        list_json(&board, None).unwrap(),
        r#"[
  {
    "num": "042",
    "title": "Ordered",
    "status": "Accepted",
    "category": "Design",
    "supersededBy": null,
    "authors": "Jean Mertz",
    "date": "2026-03-07",
    "summary": "Keeps order.",
    "slug": "042-ordered",
    "file": "docs/rfd/042-ordered.md",
    "references": [
      "001"
    ],
    "links": [
      "001"
    ],
    "referencedBy": [],
    "priority": 0,
    "milestone": "First",
    "inDevelopment": true,
    "dependsOn": [
      "001"
    ],
    "group": "planned",
    "rank": 1
  }
]
"#
    );
}

#[test]
fn the_json_listing_runs_in_section_order() {
    let json: Value = serde_json::from_str(&list_json(&board(), None).unwrap()).unwrap();

    let tags: Vec<(&str, &str, Option<u64>)> = json
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry["num"].as_str().unwrap(),
                entry["group"].as_str().unwrap(),
                entry["rank"].as_u64(),
            )
        })
        .collect();

    assert_eq!(tags, [
        ("044", "planned", Some(1)),
        ("D50", "planned", Some(2)),
        ("053", "planned", Some(3)),
        ("010", "backlog", None),
        ("001", "other", None),
        ("014", "other", None),
    ]);
}

#[test]
fn show_prints_an_rfds_metadata_and_board_position() {
    let board = board();

    assert_eq!(
        show_text(&board, entry(&board, "044")),
        "\
# RFD 044: Init

- **File**: docs/rfd/044-init.md
- **Status**: Accepted
- **Category**: Design
- **Requires**: RFD 001
- **Board**: planned, rank 1, in the v0.1 milestone
- **In development**: yes

Initializes workspaces.
"
    );
}

#[test]
fn show_says_where_an_rfd_off_the_planned_list_stands() {
    let board = board();

    assert_eq!(
        show_text(&board, entry(&board, "010")),
        "\
# RFD 010: Pty

- **File**: docs/rfd/010-pty.md
- **Status**: Accepted
- **Category**: Design
- **Board**: backlog
- **In development**: no
"
    );
    assert_eq!(
        show_text(&board, entry(&board, "014")),
        "\
# RFD 014: Guide

- **File**: docs/rfd/014-guide.md
- **Status**: Superseded
- **Category**: Guide
- **Superseded by**: RFD 015
- **Board**: not on the board
- **In development**: no
"
    );
    assert!(show_text(&board, entry(&board, "053")).contains("- **Board**: planned, rank 3\n"));
}

#[test]
fn show_names_the_rfds_that_link_to_this_one() {
    let published = [
        source("001-process.md", &[("Status", "Implemented")]),
        Source {
            content: "# RFD 002: Second\n\n- **Status**: Draft\n\nSee 001-process.\n".to_owned(),
            ..source("002-second.md", &[])
        },
    ];
    let board = Board::assemble(&published, &[], Priority::default(), &BTreeSet::new());

    assert!(
        show_text(&board, entry(&board, "001")).contains("- **Referenced by**: RFD 002\n"),
        "{}",
        show_text(&board, entry(&board, "001"))
    );
}

#[test]
fn show_json_is_the_listing_entry() {
    let board = board();
    let shown: Value =
        serde_json::from_str(&show_json(&board, entry(&board, "D50")).unwrap()).unwrap();
    let listed: Value = serde_json::from_str(&list_json(&board, None).unwrap()).unwrap();

    assert_eq!(shown, listed[1]);
}
