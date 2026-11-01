use indoc::indoc;

use super::*;

const PAGE: &str = indoc! {"
    # Ubiquitous Language: Conversation

    - **Status**: Reference
    - **Authors**: Jean Mertz <git@jeanmertz.com>
    - **Date**: 2026-10-30
    - **Parent**: W-0a1b2c3
    - **Label**: domain=conversation
    - **Label**: relevant-rfd=064
    - **Summary**: Canonical definitions for Conversation, Turn, Thread, and Event
      Stream.

    ## Turn

    A single request-response cycle.
"};

fn page(source: &str) -> Page {
    Page::read(
        "W-0a1b2c4".parse().unwrap(),
        Utf8PathBuf::from("/w/docs/wiki/0a1b2c4-ubiquitous-language.md"),
        source.to_owned(),
    )
}

#[test]
fn reads_every_field() {
    let page = page(PAGE);

    assert_eq!(
        page.title.as_deref(),
        Some("Ubiquitous Language: Conversation")
    );
    assert_eq!(page.status(), Some(Status::Reference));
    assert_eq!(page.date.as_deref(), Some("2026-10-30"));
    assert_eq!(page.parent_id(), Some("W-0a1b2c3".parse().unwrap()));
    assert_eq!(page.labels, ["domain=conversation", "relevant-rfd=064"]);
    assert_eq!(
        page.summary.as_deref(),
        Some("Canonical definitions for Conversation, Turn, Thread, and Event Stream.")
    );
    assert_eq!(page.body(), "## Turn\n\nA single request-response cycle.");
}

#[test]
fn a_page_without_a_title_is_named_by_its_file() {
    let page = page("- **Status**: Snapshot\n");

    assert_eq!(page.title, None);
    assert_eq!(page.name(), "0a1b2c4-ubiquitous-language");
}

#[test]
fn statuses_parse_without_case() {
    assert_eq!("snapshot".parse(), Ok(Status::Snapshot));
    assert_eq!(" ARCHIVED ".parse(), Ok(Status::Archived));
    assert_eq!(
        "Draft".parse::<Status>(),
        Err("`Draft` is not a status; use Reference, Snapshot, or Archived.".to_owned())
    );
}

#[test]
fn the_body_of_a_page_without_metadata_follows_its_title() {
    assert_eq!(body("# Title\n\nProse.\n"), "Prose.");
    assert_eq!(body("Prose.\n"), "Prose.");
}

#[test]
fn a_query_searches_the_title_summary_and_body() {
    let page = page(PAGE);

    assert!(page.mentions("conversation"));
    assert!(page.mentions("EVENT STREAM"));
    assert!(page.mentions("request-response"));
    assert!(!page.mentions("domain=conversation"));
}

#[test]
fn renders_a_new_page() {
    let labels = Labels::from_tokens(["domain=conversation"]).unwrap();

    let out = render(&NewPage {
        title: "Turn",
        status: Status::Reference,
        authors: "Jean Mertz",
        date: "2026-10-30",
        parent: Some("W-0a1b2c3".parse().unwrap()),
        labels: &labels,
        summary: "What a turn is.",
        body: "A single request-response cycle.\n",
    });

    assert_eq!(out, indoc! {"
        # Turn

        - **Status**: Reference
        - **Authors**: Jean Mertz
        - **Date**: 2026-10-30
        - **Parent**: W-0a1b2c3
        - **Label**: domain=conversation
        - **Summary**: What a turn is.

        A single request-response cycle.
    "});
}

/// A page created without a summary carries the field empty, so `jp wiki check`
/// names what is missing.
#[test]
fn renders_an_empty_summary_without_trailing_space() {
    let out = render(&NewPage {
        title: "Turn",
        status: Status::Snapshot,
        authors: "jp",
        date: "2026-10-30",
        parent: None,
        labels: &Labels::default(),
        summary: "",
        body: "",
    });

    assert!(
        out.ends_with("- **Date**: 2026-10-30\n- **Summary**:\n"),
        "{out}"
    );
    assert_eq!(page(&out).summary, None);
}
