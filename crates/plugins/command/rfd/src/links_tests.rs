use camino_tempfile::Utf8TempDir;

use super::*;

fn id(text: &str) -> RfdId {
    text.parse().unwrap()
}

fn labels(tokens: &[&str]) -> Labels {
    Labels::from_tokens(tokens).unwrap()
}

/// A ticket file, as `jp ticket` writes one, carrying `tokens`.
fn ticket(title: &str, status: &str, tokens: &[&str]) -> String {
    let mut text =
        format!("# {title}\n\n- **Status**: {status}\n- **Authors**: jp\n- **Date**: 2026-08-05\n");
    for token in tokens {
        text.push_str(&format!("- **Label**: {token}\n"));
    }

    text
}

#[test]
fn the_keys_default_to_tracking_rfd_and_relevant_rfd() {
    let links = Links::default();

    assert_eq!(links.tracking, "tracking-rfd");
    assert_eq!(links.relevant, "relevant-rfd");
    assert!(links.filed_with.is_empty());
}

/// A value is read the way `jp rfd` reads an id anywhere, and a key matches
/// without case; a value naming no RFD is passed over.
#[test]
fn a_ticket_names_rfds_under_either_key() {
    let links = Links::default();
    let carried = labels(&[
        "tracking-rfd=116",
        "Relevant-RFD=91",
        "relevant-rfd=RFD 100",
        "relevant-rfd=d5",
        "relevant-rfd=whatever",
        "type=tracking",
    ]);

    assert_eq!(links.tracked(&carried), [id("116")]);
    assert_eq!(links.relevant(&carried), [id("091"), id("100"), id("D05")]);
}

#[test]
fn configured_keys_replace_the_defaults() {
    let links = Links {
        tracking: "tracks".to_owned(),
        relevant: "rfd".to_owned(),
        filed_with: vec![],
    };
    let carried = labels(&["tracks=042", "rfd=043", "tracking-rfd=044"]);

    assert_eq!(links.tracked(&carried), [id("042")]);
    assert_eq!(links.relevant(&carried), [id("043")]);
}

#[test]
fn tickets_are_gathered_by_the_rfd_they_name_tracking_ones_first() {
    let dir = Utf8TempDir::new().unwrap();
    for (name, text) in [
        (
            "02wt0k1-phase.md",
            ticket("Phase one", "Todo", &["relevant-rfd=116"]),
        ),
        (
            "02wt0k2-track.md",
            ticket("Track 116", "In Progress", &[
                "relevant-rfd=100",
                "tracking-rfd=116",
            ]),
        ),
        // Naming one RFD both ways counts once, as tracking it.
        (
            "02wt0k3-both.md",
            ticket("Both", "Done", &["relevant-rfd=100", "tracking-rfd=100"]),
        ),
        (
            "02wt0k4-unrelated.md",
            ticket("Other", "Todo", &["type=bug"]),
        ),
        ("02wt0k5-broken.md", "Not a ticket.\n".to_owned()),
    ] {
        fs::write(dir.path().join(name), text).unwrap();
    }
    let entries = store::list(dir.path()).unwrap();

    let by_rfd = Links::default().by_rfd(&entries);

    assert_eq!(by_rfd.keys().collect::<Vec<_>>(), ["100", "116"]);
    assert_eq!(by_rfd["116"], [
        LinkedTicket {
            id: "T-02wt0k2".to_owned(),
            title: "Track 116".to_owned(),
            status: Status::InProgress,
            tracking: true,
        },
        LinkedTicket {
            id: "T-02wt0k1".to_owned(),
            title: "Phase one".to_owned(),
            status: Status::Todo,
            tracking: false,
        },
    ]);
    assert_eq!(by_rfd["100"], [
        LinkedTicket {
            id: "T-02wt0k3".to_owned(),
            title: "Both".to_owned(),
            status: Status::Done,
            tracking: true,
        },
        LinkedTicket {
            id: "T-02wt0k2".to_owned(),
            title: "Track 116".to_owned(),
            status: Status::InProgress,
            tracking: false,
        },
    ]);
}

#[test]
fn a_tracking_ticket_is_filed_with_its_link_first() {
    let links = Links {
        filed_with: vec!["type=tracking".to_owned(), "area=cli".to_owned()],
        ..Links::default()
    };

    assert_eq!(links.tracking_labels(id("116")), [
        "tracking-rfd=116",
        "type=tracking",
        "area=cli"
    ]);
    assert_eq!(
        links.describe_tracking(id("116")),
        "`tracking-rfd=116`, `type=tracking` and `area=cli`"
    );
    assert_eq!(
        Links::default().describe_tracking(id("116")),
        "`tracking-rfd=116`"
    );
}

/// Renumbering an RFD points the tickets naming it at the new id, under either
/// key, and leaves everything else in the file as it was.
#[test]
fn relink_points_the_links_at_the_new_id() {
    let dir = Utf8TempDir::new().unwrap();
    let path = dir.path().join("02wt0k1-ticket.md");
    let text = format!(
        "{}\nThe description names RFD 095, which stays as written.\n",
        ticket("Ticket", "Todo", &[
            "relevant-rfd=095",
            "relevant-rfd=100",
            "tracking-rfd=RFD 95",
            "type=feature",
        ])
    );
    fs::write(&path, &text).unwrap();

    assert!(
        Links::default()
            .relink(&path, id("095"), id("042"))
            .unwrap()
    );

    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        format!(
            "{}\nThe description names RFD 095, which stays as written.\n",
            ticket("Ticket", "Todo", &[
                "relevant-rfd=042",
                "relevant-rfd=100",
                "tracking-rfd=042",
                "type=feature",
            ])
        )
    );
}

#[test]
fn relink_leaves_a_file_naming_another_rfd_alone() {
    let dir = Utf8TempDir::new().unwrap();
    let ticket_path = dir.path().join("02wt0k1-ticket.md");
    let text = ticket("Ticket", "Todo", &["relevant-rfd=100", "priority=095"]);
    fs::write(&ticket_path, &text).unwrap();
    let index = dir.path().join("index.md");
    fs::write(&index, "# Tickets\n\n<TicketIndex />\n").unwrap();

    let links = Links::default();
    assert!(!links.relink(&ticket_path, id("095"), id("042")).unwrap());
    assert!(!links.relink(&index, id("095"), id("042")).unwrap());

    assert_eq!(fs::read_to_string(&ticket_path).unwrap(), text);
}
