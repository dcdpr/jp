use camino_tempfile::Utf8TempDir;

use super::*;
use crate::{Labels, NewTicket, render};

fn id(body: &str) -> TicketId {
    body.parse().unwrap()
}

fn ids(bodies: &[&str]) -> Vec<TicketId> {
    bodies.iter().map(|body| id(body)).collect()
}

fn order(todo: &[&str], in_progress: &[&str], done: &[&str]) -> Order {
    let owned = |ids: &[&str]| ids.iter().map(|id| (*id).to_owned()).collect();

    Order {
        todo: owned(todo),
        in_progress: owned(in_progress),
        done: owned(done),
    }
}

/// Write a ticket under a fixed id, so a test can name it in the order file.
fn write_ticket(dir: &Utf8TempDir, body: &str, status: Status) {
    let document = render::ticket(&NewTicket {
        title: body,
        authors: "john",
        date: "2026-08-05",
        labels: &Labels::default(),
        description: "",
    });
    let document = render::set_metadata(&document, "Status", &status.to_string()).unwrap();

    fs::write(dir.path().join(format!("{body}-slug.md")), document).unwrap();
}

#[test]
fn ranked_tickets_come_first_and_the_rest_follow_by_id() {
    let order = order(&["T-02wt0k3", "T-02wt0k1"], &[], &[]);

    let laid_out = order.arrange(
        Status::Todo,
        &ids(&["02wt0k1", "02wt0k2", "02wt0k3", "02wt0k4"]),
    );

    assert_eq!(laid_out, ids(&["02wt0k3", "02wt0k1", "02wt0k2", "02wt0k4"]));
}

/// Done reads newest first, so a ticket closed a moment ago is at the top until
/// someone ranks it.
#[test]
fn unranked_done_tickets_run_newest_first() {
    let order = order(&[], &[], &["T-02wt0k2"]);

    let laid_out = order.arrange(Status::Done, &ids(&["02wt0k1", "02wt0k2", "02wt0k3"]));

    assert_eq!(laid_out, ids(&["02wt0k2", "02wt0k3", "02wt0k1"]));
}

/// An entry for a ticket that has since moved column, or gone, takes up no
/// place.
#[test]
fn an_entry_for_a_ticket_not_in_the_column_is_skipped() {
    let order = order(&["T-02wt0k9", "T-02wt0k2"], &[], &[]);

    let laid_out = order.arrange(Status::Todo, &ids(&["02wt0k1", "02wt0k2"]));

    assert_eq!(laid_out, ids(&["02wt0k2", "02wt0k1"]));
}

/// The site's board reads the file through a JavaScript `Map`, where a repeated
/// key keeps its last position.
#[test]
fn a_ticket_named_twice_takes_its_last_position() {
    let order = order(&["T-02wt0k1", "T-02wt0k2", "T-02wt0k1"], &[], &[]);

    let laid_out = order.arrange(Status::Todo, &ids(&["02wt0k1", "02wt0k2"]));

    assert_eq!(laid_out, ids(&["02wt0k2", "02wt0k1"]));
}

/// Ranking above an unranked ticket ranks the moved one and nothing below it:
/// the tickets nobody placed keep falling back to id order.
#[test]
fn ranking_before_an_unranked_ticket_ranks_only_the_moved_one() {
    let mut order = order(&["T-02wt0k1"], &[], &[]);
    let column = ids(&["02wt0k1", "02wt0k2", "02wt0k3", "02wt0k4"]);

    assert!(order.rank(
        Status::Todo,
        &column,
        id("02wt0k4"),
        Place::Before,
        id("02wt0k2")
    ));

    assert_eq!(order.todo, ["T-02wt0k1", "T-02wt0k4"]);
    assert_eq!(
        order.arrange(Status::Todo, &column),
        ids(&["02wt0k1", "02wt0k4", "02wt0k2", "02wt0k3"])
    );
}

/// Placing a ticket below an unranked one has to rank everything above it, or
/// the unranked one would fall back below it again.
#[test]
fn ranking_after_an_unranked_ticket_ranks_what_sits_above() {
    let mut order = Order::default();
    let column = ids(&["02wt0k1", "02wt0k2", "02wt0k3"]);

    assert!(order.rank(
        Status::Todo,
        &column,
        id("02wt0k1"),
        Place::After,
        id("02wt0k2")
    ));

    assert_eq!(order.todo, ["T-02wt0k2", "T-02wt0k1"]);
    assert_eq!(
        order.arrange(Status::Todo, &column),
        ids(&["02wt0k2", "02wt0k1", "02wt0k3"])
    );
}

/// A ticket ranked already stays ranked when another one moves above it.
#[test]
fn ranking_keeps_the_tickets_ranked_before() {
    let mut order = order(&["T-02wt0k1", "T-02wt0k2", "T-02wt0k3"], &[], &[]);
    let column = ids(&["02wt0k1", "02wt0k2", "02wt0k3", "02wt0k4"]);

    assert!(order.rank(
        Status::Todo,
        &column,
        id("02wt0k3"),
        Place::Before,
        id("02wt0k1")
    ));

    assert_eq!(order.todo, ["T-02wt0k3", "T-02wt0k1", "T-02wt0k2"]);
}

/// The column being written loses its stale entries; the others are not read
/// for this ranking, so they are left exactly as they were.
#[test]
fn ranking_drops_stale_entries_from_its_own_column_only() {
    let mut order = order(&["T-02wt0k9", "T-02wt0k1"], &["T-02wt0k8"], &["T-02wt0k7"]);
    let column = ids(&["02wt0k1", "02wt0k2"]);

    assert!(order.rank(
        Status::Todo,
        &column,
        id("02wt0k2"),
        Place::Before,
        id("02wt0k1")
    ));

    assert_eq!(order.todo, ["T-02wt0k2", "T-02wt0k1"]);
    assert_eq!(order.in_progress, ["T-02wt0k8"]);
    assert_eq!(order.done, ["T-02wt0k7"]);
}

#[test]
fn ranking_against_itself_or_outside_the_column_changes_nothing() {
    let before = order(&["T-02wt0k1"], &[], &[]);
    let column = ids(&["02wt0k1", "02wt0k2"]);

    let mut order = before.clone();
    assert!(!order.rank(
        Status::Todo,
        &column,
        id("02wt0k1"),
        Place::Before,
        id("02wt0k1")
    ));
    assert!(!order.rank(
        Status::Todo,
        &column,
        id("02wt0k1"),
        Place::Before,
        id("02wt0k9")
    ));
    assert_eq!(order, before);
}

#[test]
fn a_board_without_an_order_file_has_ranked_nothing() {
    let dir = Utf8TempDir::new().unwrap();

    assert_eq!(read(dir.path()).unwrap(), Order::default());
}

/// A write must never replace a file it couldn't read: the order in it would be
/// lost without anyone having seen it.
#[test]
fn an_unreadable_order_file_is_an_error() {
    let dir = Utf8TempDir::new().unwrap();
    fs::write(dir.path().join(BOARD_FILE), "not json").unwrap();

    let error = read(dir.path()).unwrap_err();

    assert!(
        error.to_string().starts_with(&format!(
            "{} is not a board order: ",
            dir.path().join(BOARD_FILE)
        )),
        "{error}"
    );
}

/// The site's board writes the same file, so both writers produce the same
/// bytes and neither leaves a diff for the other to undo.
#[test]
fn the_order_file_is_written_the_way_the_site_writes_it() {
    let dir = Utf8TempDir::new().unwrap();

    write(dir.path(), &order(&["T-02wt0k1", "T-02wt0k2"], &[], &[])).unwrap();

    assert_eq!(
        fs::read_to_string(dir.path().join(BOARD_FILE)).unwrap(),
        "{\n  \"todo\": [\n    \"T-02wt0k1\",\n    \"T-02wt0k2\"\n  ],\n  \"in_progress\": [],\n  \
         \"done\": []\n}\n"
    );
}

#[test]
fn rank_writes_the_order_file() {
    let dir = Utf8TempDir::new().unwrap();
    write_ticket(&dir, "02wt0k1", Status::Todo);
    write_ticket(&dir, "02wt0k2", Status::Todo);

    let status = rank(dir.path(), id("02wt0k2"), Place::Before, id("02wt0k1")).unwrap();

    assert_eq!(status, Status::Todo);
    assert_eq!(read(dir.path()).unwrap().todo, ["T-02wt0k2"]);
}

/// The order within one column says nothing about another, so ranking across
/// columns is refused rather than guessed at.
#[test]
fn rank_refuses_tickets_in_different_columns() {
    let dir = Utf8TempDir::new().unwrap();
    write_ticket(&dir, "02wt0k1", Status::Todo);
    write_ticket(&dir, "02wt0k2", Status::Done);

    let error = rank(dir.path(), id("02wt0k1"), Place::After, id("02wt0k2")).unwrap_err();

    assert_eq!(
        error.to_string(),
        "T-02wt0k1 is Todo and T-02wt0k2 is Done; a ticket is ranked within its own column."
    );
    assert!(!dir.path().join(BOARD_FILE).exists());
}

/// The column a ticket is in, read back from its file.
fn status_on_disk(dir: &Utf8TempDir, body: &str) -> Status {
    status_of(&store::list(dir.path()).unwrap(), id(body)).unwrap()
}

/// A move that names its neighbour changes the column and the order together.
#[test]
fn move_to_places_the_ticket_next_to_its_neighbour() {
    let dir = Utf8TempDir::new().unwrap();
    write_ticket(&dir, "02wt0k1", Status::Todo);
    write_ticket(&dir, "02wt0k2", Status::InProgress);
    write_ticket(&dir, "02wt0k3", Status::InProgress);

    let moved = move_to(
        dir.path(),
        id("02wt0k1"),
        Status::InProgress,
        Some((Place::After, id("02wt0k2"))),
    )
    .unwrap();

    assert_eq!(moved, Moved {
        path: dir.path().join("02wt0k1-slug.md"),
        previous: Status::Todo,
    });
    assert_eq!(status_on_disk(&dir, "02wt0k1"), Status::InProgress);
    assert_eq!(
        read(dir.path())
            .unwrap()
            .arrange(Status::InProgress, &ids(&["02wt0k1", "02wt0k2", "02wt0k3"])),
        ids(&["02wt0k2", "02wt0k1", "02wt0k3"])
    );
}

/// The neighbour is checked before anything is written, so a refused move
/// leaves the ticket where it was.
#[test]
fn move_to_refuses_a_neighbour_outside_the_column_and_changes_nothing() {
    let dir = Utf8TempDir::new().unwrap();
    write_ticket(&dir, "02wt0k1", Status::Todo);
    write_ticket(&dir, "02wt0k2", Status::Done);

    let error = move_to(
        dir.path(),
        id("02wt0k1"),
        Status::InProgress,
        Some((Place::Before, id("02wt0k2"))),
    )
    .unwrap_err();

    assert_eq!(
        error.to_string(),
        "T-02wt0k2 is Done, not In Progress; a ticket is placed next to one in the column it \
         moves to."
    );
    assert_eq!(status_on_disk(&dir, "02wt0k1"), Status::Todo);
    assert!(!dir.path().join(BOARD_FILE).exists());
}

/// The order file is read before the status changes, so an unreadable one
/// refuses the move rather than leaving it half done.
#[test]
fn move_to_refuses_an_unreadable_order_file_before_moving() {
    let dir = Utf8TempDir::new().unwrap();
    write_ticket(&dir, "02wt0k1", Status::Todo);
    write_ticket(&dir, "02wt0k2", Status::Done);
    fs::write(dir.path().join(BOARD_FILE), "not json").unwrap();

    let error = move_to(
        dir.path(),
        id("02wt0k1"),
        Status::Done,
        Some((Place::After, id("02wt0k2"))),
    )
    .unwrap_err();

    assert!(
        error.to_string().contains(" is not a board order: "),
        "{error}"
    );
    assert_eq!(status_on_disk(&dir, "02wt0k1"), Status::Todo);
}

/// Without a neighbour the ticket joins the column below the ranked ones, and
/// the order file isn't touched.
#[test]
fn move_to_without_a_neighbour_changes_only_the_status() {
    let dir = Utf8TempDir::new().unwrap();
    write_ticket(&dir, "02wt0k1", Status::Todo);

    let moved = move_to(dir.path(), id("02wt0k1"), Status::Done, None).unwrap();

    assert_eq!(moved.previous, Status::Todo);
    assert_eq!(status_on_disk(&dir, "02wt0k1"), Status::Done);
    assert!(!dir.path().join(BOARD_FILE).exists());
}

#[test]
fn move_to_refuses_a_ticket_placed_next_to_itself() {
    let dir = Utf8TempDir::new().unwrap();
    write_ticket(&dir, "02wt0k1", Status::Todo);

    let error = move_to(
        dir.path(),
        id("02wt0k1"),
        Status::Done,
        Some((Place::Before, id("02wt0k1"))),
    )
    .unwrap_err();

    assert_eq!(
        error.to_string(),
        "T-02wt0k1 can't be ranked against itself."
    );
    assert_eq!(status_on_disk(&dir, "02wt0k1"), Status::Todo);
}

#[test]
fn rank_refuses_a_ticket_against_itself_and_a_missing_one() {
    let dir = Utf8TempDir::new().unwrap();
    write_ticket(&dir, "02wt0k1", Status::Todo);

    assert_eq!(
        rank(dir.path(), id("02wt0k1"), Place::Before, id("02wt0k1"))
            .unwrap_err()
            .to_string(),
        "T-02wt0k1 can't be ranked against itself."
    );
    assert_eq!(
        rank(dir.path(), id("02wt0k1"), Place::Before, id("02wt0k9"))
            .unwrap_err()
            .to_string(),
        "No ticket T-02wt0k9."
    );
}
