use camino::Utf8PathBuf;
use camino_tempfile::Utf8TempDir;

use super::*;

fn id(text: &str) -> PageId {
    text.parse().unwrap()
}

/// A page under `parent` when one is named.
fn page(id_: &str, parent: Option<&str>, status: &str) -> Page {
    let parent = parent.map_or(String::new(), |parent| format!("- **Parent**: {parent}\n"));
    let source = format!("# {id_}\n\n- **Status**: {status}\n{parent}- **Summary**: x\n");

    Page::read(id(id_), Utf8PathBuf::from(format!("/w/{id_}.md")), source)
}

fn ids(rows: &[(usize, &Page)]) -> Vec<(usize, String)> {
    rows.iter()
        .map(|(depth, page)| (*depth, page.id.to_string()))
        .collect()
}

#[test]
fn layout_follows_the_top_pages_then_their_children() {
    let pages = [
        page("W-0000003", None, "Snapshot"),
        page("W-0000001", None, "Reference"),
        page("W-0000002", Some("W-0000001"), "Reference"),
        page("W-0000004", Some("W-0000002"), "Reference"),
        page("W-0000005", None, "Archived"),
    ];

    let rows = layout(&pages, &Order::default());

    assert_eq!(ids(&rows), [
        (0, "W-0000001".to_owned()),
        (1, "W-0000002".to_owned()),
        (2, "W-0000004".to_owned()),
        (0, "W-0000003".to_owned()),
    ]);
}

#[test]
fn ranked_pages_come_first() {
    let pages = [
        page("W-0000001", None, "Reference"),
        page("W-0000002", None, "Reference"),
        page("W-0000003", None, "Reference"),
    ];
    let order = Order {
        top: vec!["W-0000003".to_owned()],
        ..Order::default()
    };

    let rows = layout(&pages, &order);

    assert_eq!(ids(&rows), [
        (0, "W-0000003".to_owned()),
        (0, "W-0000001".to_owned()),
        (0, "W-0000002".to_owned()),
    ]);
}

/// A loop of parents still shows every page once.
#[test]
fn pages_whose_parents_loop_are_laid_out_at_the_top() {
    let pages = [
        page("W-0000001", Some("W-0000002"), "Reference"),
        page("W-0000002", Some("W-0000001"), "Reference"),
    ];

    let rows = layout(&pages, &Order::default());

    assert_eq!(ids(&rows), [
        (0, "W-0000001".to_owned()),
        (1, "W-0000002".to_owned()),
    ]);
}

/// A page under an archived parent is read as a top page, which is how the
/// sidebar shows it until `jp wiki check` gets it a new parent.
#[test]
fn a_page_under_an_archived_parent_sits_at_the_top() {
    let pages = [
        page("W-0000001", None, "Archived"),
        page("W-0000002", Some("W-0000001"), "Reference"),
    ];

    assert_eq!(place_of(&pages[1], &pages), Place::Top);
}

#[test]
fn rank_writes_the_list_down_to_the_moved_page() {
    let place = Place::Top;
    let siblings = [id("W-0000001"), id("W-0000002"), id("W-0000003")];
    let mut order = Order::default();

    // The pages below the moved one keep sorting by id, so only it is written.
    assert!(order.rank(place, &siblings, id("W-0000002"), true, id("W-0000001")));
    assert_eq!(order.list(place), ["W-0000002"]);

    assert!(order.rank(place, &siblings, id("W-0000003"), false, id("W-0000002")));
    assert_eq!(order.list(place), ["W-0000002", "W-0000003"]);
    assert_eq!(arrange(order.list(place), &siblings), [
        id("W-0000002"),
        id("W-0000003"),
        id("W-0000001")
    ]);

    assert!(!order.rank(place, &siblings, id("W-0000003"), true, id("W-0000003")));
    assert!(!order.rank(place, &siblings, id("W-0000009"), true, id("W-0000001")));
}

#[test]
fn remove_drops_a_page_and_its_children_list() {
    let mut order = Order {
        top: vec!["W-0000001".to_owned(), "W-0000002".to_owned()],
        children: BTreeMap::from([("W-0000001".to_owned(), vec!["W-0000003".to_owned()])]),
    };

    assert!(order.remove(id("W-0000001")));
    assert_eq!(order.list(Place::Top), ["W-0000002"]);
    assert!(order.children.is_empty());
    assert!(!order.remove(id("W-0000009")));
}

#[test]
fn an_order_round_trips_through_its_file() {
    let dir = Utf8TempDir::new().unwrap();
    assert_eq!(Order::read(dir.path()), Ok(Order::default()));

    let order = Order {
        top: vec!["W-0000001".to_owned()],
        children: BTreeMap::from([("W-0000001".to_owned(), vec![])]),
    };
    order.write(dir.path()).unwrap();

    assert_eq!(
        std::fs::read_to_string(dir.path().join(ORDER_FILE)).unwrap(),
        "{\n  \"top\": [\n    \"W-0000001\"\n  ],\n  \"children\": {}\n}\n"
    );
    assert_eq!(Order::read(dir.path()).unwrap().top, ["W-0000001"]);

    std::fs::write(dir.path().join(ORDER_FILE), "not json").unwrap();
    assert!(Order::read(dir.path()).is_err());
}
