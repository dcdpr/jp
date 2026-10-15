use camino::Utf8PathBuf;
use camino_tempfile::Utf8TempDir;
use serde_json::json;

use super::*;

fn board(raw: &Value) -> Priority {
    Priority::normalize(raw)
}

fn ids(list: &[&str]) -> Vec<String> {
    list.iter().map(|id| (*id).to_owned()).collect()
}

#[test]
fn the_board_file_reads_into_milestone_groups_and_a_backlog() {
    let priority = board(&json!({
        "planned": [
            { "milestone": "First", "ids": ["090", "D63"] },
            { "milestone": null, "ids": ["053"] }
        ],
        "backlog": ["010"]
    }));

    assert_eq!(priority.planned, [
        Group {
            milestone: Some("First".to_owned()),
            ids: ids(&["090", "D63"]),
        },
        Group {
            milestone: None,
            ids: ids(&["053"]),
        },
    ]);
    assert_eq!(priority.order(), ["090", "D63", "053"]);
    assert_eq!(priority.backlog, ["010"]);
}

/// The planned RFDs no milestone claims sit below the last marker, so that
/// group always exists, even when the file names none.
#[test]
fn the_unassigned_group_is_always_last() {
    let priority = board(&json!({
        "planned": [{ "milestone": "First", "ids": ["090"] }]
    }));

    assert_eq!(priority.planned.len(), 2);
    assert_eq!(priority.planned[1], Group {
        milestone: None,
        ids: vec![],
    });
}

#[test]
fn the_older_flat_order_reads_as_one_unassigned_group() {
    let priority = board(&json!({ "order": ["090", 53], "backlog": [10] }));

    assert_eq!(priority.planned, [Group {
        milestone: None,
        ids: ids(&["090", "53"]),
    }]);
    assert_eq!(priority.backlog, ["10"]);
}

#[test]
fn a_malformed_board_file_reads_as_an_empty_board() {
    for raw in [
        json!(null),
        json!([]),
        json!({ "planned": "nope", "backlog": {} }),
    ] {
        let priority = board(&raw);

        assert_eq!(priority, Priority::default(), "{raw}");
        assert!(priority.order().is_empty());
    }
}

/// The site's board writes the file with `JSON.stringify(board, null, 2)` and a
/// trailing newline, so both writers produce the same bytes.
#[test]
fn the_board_file_is_written_the_way_the_site_writes_it() {
    let priority = board(&json!({
        "planned": [
            { "milestone": "First", "ids": ["090"] },
            { "milestone": null, "ids": [] }
        ],
        "backlog": ["010", "D01"]
    }));

    assert_eq!(
        priority.render(),
        r#"{
  "planned": [
    {
      "milestone": "First",
      "ids": [
        "090"
      ]
    },
    {
      "milestone": null,
      "ids": []
    }
  ],
  "backlog": [
    "010",
    "D01"
  ]
}
"#
    );
}

/// JP's own board file reads and writes back unchanged, which is what keeps a
/// command from leaving a formatting diff behind.
#[test]
fn the_jp_board_file_round_trips() {
    let dir = Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../docs/rfd");
    let source = fs::read_to_string(dir.join(PRIORITY_FILE)).unwrap();

    let priority = read(&dir).unwrap().unwrap();

    assert_eq!(priority.render(), source);
}

#[test]
fn rewrite_replaces_an_id_wherever_it_is_placed() {
    let mut priority = board(&json!({
        "planned": [{ "milestone": "First", "ids": ["090", "D05"] }],
        "backlog": ["D05"]
    }));

    assert!(priority.rewrite("D05", "117"));
    assert_eq!(priority.order(), ["090", "117"]);
    assert_eq!(priority.backlog, ["117"]);

    assert!(!priority.rewrite("D99", "118"));
}

#[test]
fn remove_takes_ids_off_every_list() {
    let mut priority = board(&json!({
        "planned": [
            { "milestone": "First", "ids": ["090", "053"] },
            { "milestone": null, "ids": ["044"] }
        ],
        "backlog": ["010", "053"]
    }));

    assert!(priority.remove(&["053", "044"]));
    assert_eq!(priority.order(), ["090"]);
    assert_eq!(priority.backlog, ["010"]);

    assert!(!priority.remove(&["999"]));
}

#[test]
fn find_and_insert_address_one_slot() {
    let mut priority = board(&json!({
        "planned": [
            { "milestone": "First", "ids": ["090", "053"] },
            { "milestone": null, "ids": ["044"] }
        ],
        "backlog": ["010"]
    }));

    assert_eq!(
        priority.find("053"),
        Some(Slot::Planned { group: 0, index: 1 })
    );
    assert_eq!(priority.find("010"), Some(Slot::Backlog(0)));
    assert_eq!(priority.find("999"), None);

    priority.insert(Slot::Planned { group: 1, index: 0 }, "116");
    priority.insert(Slot::Backlog(1), "D05");

    assert_eq!(priority.order(), ["090", "053", "116", "044"]);
    assert_eq!(priority.backlog, ["010", "D05"]);
    assert_eq!(priority.placed().collect::<Vec<_>>(), [
        "090", "053", "116", "044", "010", "D05"
    ]);
}

#[test]
fn a_missing_board_file_reads_as_none() {
    let dir = Utf8TempDir::new().unwrap();

    assert_eq!(read(dir.path()), Ok(None));
}

/// A command reads the file before it writes anything, so a broken one stops
/// the command before it changes a document.
#[test]
fn a_board_file_that_is_not_json_refuses_an_update() {
    let dir = Utf8TempDir::new().unwrap();
    fs::write(dir.path().join(PRIORITY_FILE), "{ nope").unwrap();

    let error = read_for_update(dir.path()).unwrap_err();

    assert!(
        error.starts_with(&format!(
            "{} is not valid JSON (",
            dir.path().join(PRIORITY_FILE)
        )),
        "{error}"
    );
    assert!(
        error.ends_with(
            "), so the priority board can't be updated. Fix it and run the command again."
        ),
        "{error}"
    );
}

#[test]
fn write_puts_the_rendered_board_in_the_rfd_directory() {
    let dir = Utf8TempDir::new().unwrap();

    let path = write(dir.path(), &Priority::default()).unwrap();

    assert_eq!(path, dir.path().join(PRIORITY_FILE));
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "{\n  \"planned\": [\n    {\n      \"milestone\": null,\n      \"ids\": []\n    }\n  ],\n  \
         \"backlog\": []\n}\n"
    );
}
