use camino_tempfile::Utf8TempDir;
use serde_json::{Value, json};

use super::*;
use crate::{priority::PRIORITY_FILE, tests::jp_root};

/// An RFD document carrying `fields` in its metadata block.
fn rfd(heading: &str, fields: &[(&str, &str)]) -> String {
    let mut out = format!("# RFD {heading}\n\n");
    for (key, value) in fields {
        out.push_str(&format!("- **{key}**: {value}\n"));
    }
    out.push_str("\n## Summary\n\nBody.\n");

    out
}

fn source(name: &str, content: String) -> Source {
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

fn accepted(name: &str) -> Source {
    let heading = format!("{}: {name}", &name[..3]);
    source(name, rfd(&heading, &[("Status", "Accepted")]))
}

fn board(published: &[Source], drafts: &[Source], priority: &Value) -> Board {
    Board::assemble(
        published,
        drafts,
        Priority::normalize(priority),
        &BTreeMap::new(),
    )
}

fn entry<'a>(board: &'a Board, num: &str) -> &'a Entry {
    board
        .entries
        .iter()
        .find(|entry| entry.num == num)
        .unwrap_or_else(|| panic!("no entry {num}"))
}

/// A ticket file, as `jp ticket` writes one.
fn ticket(status: &str, labels: &[&str]) -> String {
    let mut out = format!("# A ticket\n\n- **Status**: {status}\n");
    out.push_str("- **Authors**: jp\n");
    out.push_str("- **Date**: 2026-08-05\n");
    for label in labels {
        out.push_str(&format!("- **Label**: {label}\n"));
    }

    out
}

#[test]
fn an_entry_carries_its_metadata() {
    let content = rfd("042: Ordered Things", &[
        ("Status", "Superseded"),
        ("Category", "Design"),
        ("Authors", "Jean Mertz"),
        ("Date", "2026-03-07"),
        ("Superseded by", "[RFD 043]"),
        ("Summary", "Keeps things in order."),
    ]);

    let board = board(&[source("042-ordered-things.md", content)], &[], &json!({}));
    let entry = entry(&board, "042");

    assert_eq!(entry.title, "Ordered Things");
    assert_eq!(entry.status.as_deref(), Some("Superseded"));
    assert_eq!(entry.category.as_deref(), Some("Design"));
    assert_eq!(entry.authors.as_deref(), Some("Jean Mertz"));
    assert_eq!(entry.date.as_deref(), Some("2026-03-07"));
    assert_eq!(entry.superseded_by.as_deref(), Some("043"));
    assert_eq!(entry.summary.as_deref(), Some("Keeps things in order."));
    assert_eq!(entry.slug, "042-ordered-things");
    assert_eq!(entry.file, "docs/rfd/042-ordered-things.md");
}

/// A document without a heading still lists, under its file name.
#[test]
fn an_entry_without_a_heading_is_titled_by_its_file() {
    let board = board(
        &[source(
            "042-untitled.md",
            "- **Status**: Draft\n".to_owned(),
        )],
        &[],
        &json!({}),
    );

    assert_eq!(entry(&board, "042").title, "042-untitled.md");
}

#[test]
fn planned_and_backlog_rfds_take_their_board_positions() {
    let board = board(
        &[
            accepted("001-first.md"),
            accepted("002-second.md"),
            accepted("003-third.md"),
        ],
        &[],
        &json!({
            "planned": [
                { "milestone": "First", "ids": ["002"] },
                { "milestone": null, "ids": ["001"] }
            ],
            "backlog": ["003"]
        }),
    );

    assert_eq!(board.cutoff(), 2);
    assert_eq!(entry(&board, "002").priority, Some(0));
    assert_eq!(entry(&board, "002").milestone.as_deref(), Some("First"));
    assert_eq!(entry(&board, "001").priority, Some(1));
    assert_eq!(entry(&board, "001").milestone, None);
    assert_eq!(entry(&board, "003").priority, Some(2));
    assert_eq!(entry(&board, "003").milestone, None);
}

/// The board file goes stale as soon as a status changes, so a terminal RFD
/// still listed in it holds no position.
#[test]
fn a_terminal_rfd_holds_no_position() {
    let implemented = source(
        "001-done.md",
        rfd("001: Done", &[("Status", "Implemented")]),
    );
    let abandoned = source(
        "002-dropped.md",
        rfd("002: Dropped", &[("Status", "Abandoned")]),
    );

    let board = board(
        &[implemented, abandoned],
        &[],
        &json!({ "planned": [{ "milestone": "First", "ids": ["001"] }], "backlog": ["002"] }),
    );

    for num in ["001", "002"] {
        assert_eq!(entry(&board, num).priority, None, "{num}");
        assert_eq!(entry(&board, num).milestone, None, "{num}");
    }
}

/// The site's board reads the file through a JavaScript `Map`, where a repeated
/// id keeps its last position.
#[test]
fn an_id_listed_twice_takes_its_last_position() {
    let board = board(
        &[accepted("001-first.md"), accepted("002-second.md")],
        &[],
        &json!({ "planned": [{ "milestone": null, "ids": ["001", "002"] }], "backlog": ["001"] }),
    );

    assert_eq!(entry(&board, "001").priority, Some(2));
    assert_eq!(entry(&board, "002").priority, Some(1));
}

#[test]
fn an_id_no_rfd_carries_is_skipped() {
    let board = board(
        &[accepted("001-first.md")],
        &[],
        &json!({ "planned": [{ "milestone": null, "ids": ["999", "001"] }] }),
    );

    assert_eq!(board.entries.len(), 1);
    assert_eq!(entry(&board, "001").priority, Some(1));
}

/// Extending implies requiring, so both lists hold the RFD above this one on
/// the board, each named once.
#[test]
fn an_rfd_depends_on_what_it_requires_and_extends() {
    let content = rfd("042: Both", &[
        ("Status", "Accepted"),
        ("Requires", "[RFD 072], [RFD 001]"),
        ("Extends", "[RFD 001], [RFD 100]"),
    ]);

    let board = board(&[source("042-both.md", content)], &[], &json!({}));

    assert_eq!(entry(&board, "042").depends_on, ["072", "001", "100"]);
}

/// A published RFD never links to a draft, so only RFDs of its own kind count
/// as linking to it.
#[test]
fn referenced_by_counts_links_within_one_kind() {
    let linking = source(
        "002-linking.md",
        rfd("002: Linking", &[("Status", "Accepted")]) + "\nSee 001-first.\n",
    );
    let draft = source(
        "D01-draft.md",
        rfd("D01: Draft", &[("Status", "Draft")]) + "\nSee ../001-first.md.\n",
    );

    let board = board(&[accepted("001-first.md"), linking], &[draft], &json!({}));

    assert_eq!(entry(&board, "001").referenced_by, ["002"]);
    assert_eq!(entry(&board, "002").references, ["001"]);
    assert_eq!(entry(&board, "D01").references, ["001"]);
}

#[test]
fn references_leave_out_the_rfd_itself_the_templates_and_tickets() {
    let content = rfd("042: Self", &[("Status", "Accepted")])
        + "\nSee 042-self, 000-design-template, 100-tickets, and T-02wt0kx.\n";

    let board = board(&[source("042-self.md", content)], &[], &json!({}));
    let entry = entry(&board, "042");

    assert_eq!(entry.references, ["100"]);
    // `links` feeds the site's cross-kind widget, which resolves tickets too.
    assert_eq!(entry.links, ["000", "100", "T-02wt0kx"]);
}

/// Only a tracking ticket In Progress puts its RFD in development: a ticket
/// whose work merely belongs to the RFD doesn't, and neither does a tracking
/// ticket nobody is working on.
#[test]
fn an_rfd_is_in_development_when_its_tracking_ticket_is_in_progress() {
    let dir = Utf8TempDir::new().unwrap();
    for (id, content) in [
        ("02wt0k1", ticket("In Progress", &["tracking-rfd=042"])),
        ("02wt0k2", ticket("In Progress", &["tracking-rfd=RFD 7"])),
        ("02wt0k3", ticket("In Progress", &["relevant-rfd=043"])),
        ("02wt0k4", ticket("Todo", &["tracking-rfd=044"])),
        ("02wt0k5", ticket("In Progress", &["tracking-rfd=whatever"])),
    ] {
        fs::write(dir.path().join(format!("{id}-ticket.md")), content).unwrap();
    }
    let tickets = Links::default().by_rfd(&store::list(dir.path()).unwrap());

    let board = Board::assemble(
        &[
            accepted("007-seven.md"),
            accepted("042-forty-two.md"),
            accepted("043-forty-three.md"),
            accepted("044-forty-four.md"),
        ],
        &[],
        Priority::default(),
        &tickets,
    );

    let in_development: Vec<&str> = board
        .entries
        .iter()
        .filter(|entry| entry.in_development)
        .map(|entry| entry.num.as_str())
        .collect();
    assert_eq!(in_development, ["007", "042"]);
    assert_eq!(entry(&board, "043").tickets.len(), 1);
    assert!(!entry(&board, "043").tickets[0].tracking);
}

#[test]
fn load_reads_rfds_drafts_the_board_and_the_tickets() {
    let root = Utf8TempDir::new().unwrap();
    let dir = root.path().join("docs/rfd");
    let tickets = root.path().join("docs/ticket");
    fs::create_dir_all(dir.join("drafts")).unwrap();
    fs::create_dir_all(&tickets).unwrap();

    fs::write(
        dir.join("042-ordered.md"),
        rfd("042: Ordered", &[("Status", "Accepted")]),
    )
    .unwrap();
    fs::write(
        dir.join("drafts/D05-sketch.md"),
        rfd("D05: Sketch", &[("Status", "Draft")]),
    )
    .unwrap();
    // Templates, index pages, and files without a slug are not RFDs.
    fs::write(dir.join("000-design-template.md"), "# RFD NNN: TITLE\n").unwrap();
    fs::write(dir.join("index.md"), "# RFDs\n").unwrap();
    fs::write(dir.join("043-.md"), "# RFD 043: No slug\n").unwrap();
    fs::write(dir.join("044-notes.txt"), "").unwrap();
    fs::write(dir.join("drafts/index.md"), "# Drafts\n").unwrap();
    fs::write(
        dir.join(PRIORITY_FILE),
        r#"{"planned": [{"milestone": null, "ids": ["D05"]}], "backlog": ["042"]}"#,
    )
    .unwrap();
    fs::write(
        tickets.join("02wt0k1-track.md"),
        ticket("In Progress", &["tracking-rfd=042"]),
    )
    .unwrap();

    let Loaded { board, warnings } =
        load(root.path(), &dir, Some((&tickets, &Links::default()))).unwrap();

    assert!(warnings.is_empty(), "{warnings:?}");
    let files: Vec<&str> = board
        .entries
        .iter()
        .map(|entry| entry.file.as_str())
        .collect();
    assert_eq!(files, [
        "docs/rfd/042-ordered.md",
        "docs/rfd/drafts/D05-sketch.md"
    ]);
    assert_eq!(entry(&board, "D05").priority, Some(0));
    assert_eq!(entry(&board, "042").priority, Some(1));
    assert!(entry(&board, "042").in_development);
    assert!(!entry(&board, "D05").in_development);
    assert_eq!(entry(&board, "042").tickets[0].id, "T-02wt0k1");
}

/// Ranking reads no tickets, so a broken ticket directory can't get in its way.
#[test]
fn load_without_tickets_reads_none() {
    let root = Utf8TempDir::new().unwrap();
    let dir = root.path().join("rfd");
    let tickets = root.path().join("tickets");
    fs::create_dir_all(&dir).unwrap();
    fs::create_dir_all(&tickets).unwrap();
    fs::write(
        dir.join("042-ordered.md"),
        rfd("042: Ordered", &[("Status", "Accepted")]),
    )
    .unwrap();
    fs::write(tickets.join("02wt0k1-broken.md"), "no heading\n").unwrap();

    let Loaded { board, warnings } = load(root.path(), &dir, None).unwrap();

    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(entry(&board, "042").tickets.is_empty());
}

/// A new project has no drafts, no board file, and maybe no tickets yet, and
/// lists fine without them.
#[test]
fn load_needs_only_the_rfd_directory() {
    let root = Utf8TempDir::new().unwrap();
    let dir = root.path().join("rfd");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("001-first.md"),
        rfd("001: First", &[("Status", "Accepted")]),
    )
    .unwrap();

    let tickets = root.path().join("tickets");
    let Loaded { board, warnings } =
        load(root.path(), &dir, Some((&tickets, &Links::default()))).unwrap();

    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(board.entries.len(), 1);
    assert_eq!(board.priority, Priority::default());
}

#[test]
fn load_refuses_a_missing_rfd_directory() {
    let root = Utf8TempDir::new().unwrap();
    let dir = root.path().join("docs/rfd");

    assert_eq!(
        load(root.path(), &dir, None).unwrap_err(),
        format!(
            "There is no RFD directory at {dir}. Set `plugins.command.rfd.options.dir` to the \
             directory holding the RFDs."
        )
    );
}

/// A broken board file or ticket leaves part of the board unknown, which the
/// listing has to say rather than hide.
#[test]
fn load_warns_about_what_it_could_not_read() {
    let root = Utf8TempDir::new().unwrap();
    let dir = root.path().join("rfd");
    let tickets = root.path().join("tickets");
    fs::create_dir_all(&dir).unwrap();
    fs::create_dir_all(&tickets).unwrap();
    fs::write(dir.join(PRIORITY_FILE), "{ not json").unwrap();
    fs::write(tickets.join("02wt0k1-broken.md"), "no heading\n").unwrap();

    let Loaded { board, warnings } =
        load(root.path(), &dir, Some((&tickets, &Links::default()))).unwrap();

    assert_eq!(board.priority, Priority::default());
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(
        warnings[0].starts_with(&format!("{} is not valid JSON (", dir.join(PRIORITY_FILE))),
        "{}",
        warnings[0]
    );
    assert_eq!(
        warnings[1],
        format!(
            "1 ticket file in {tickets} could not be read; an RFD whose tracking ticket is among \
             them does not show as in development."
        )
    );
}

/// JP's own RFDs and board read cleanly, whatever they currently hold.
#[test]
fn the_jp_board_loads() {
    let root = jp_root().canonicalize_utf8().unwrap();

    let tickets = root.join("docs/ticket");
    let Loaded { board, .. } = load(
        &root,
        &root.join("docs/rfd"),
        Some((&tickets, &Links::default())),
    )
    .unwrap();

    let process = entry(&board, "001");
    assert_eq!(process.title, "The JP RFD Process");
    assert_eq!(process.file, "docs/rfd/001-jp-rfd-process.md");
}
