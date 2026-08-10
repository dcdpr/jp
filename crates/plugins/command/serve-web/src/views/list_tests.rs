use chrono::{DateTime, Utc};
use jp_plugin::message::ConversationSummary;

use super::*;

fn summary(id: &str, title: Option<&str>) -> ConversationSummary {
    ConversationSummary {
        id: id.to_owned(),
        title: title.map(ToOwned::to_owned),
        last_activated_at: "2025-01-01T00:00:00Z"
            .parse::<DateTime<Utc>>()
            .expect("fixed timestamp parses"),
        events_count: 0,
    }
}

#[test]
fn renders_filter_field_and_entries() {
    let conversations = vec![
        summary("0001", Some("Add a search bar")),
        summary("0002", None),
    ];

    let html = render(&conversations).into_string();

    assert!(html.contains(r#"id="filter""#), "no filter field: {html}");
    assert!(
        html.contains(r#"id="no-matches""#),
        "no empty state: {html}"
    );
    assert!(html.contains("Add a search bar"), "entry missing: {html}");
    assert!(html.contains("Untitled"), "untitled entry missing: {html}");
}

/// The field filters the list that is already on the page, so an empty list has
/// nothing to filter and would leave the script reaching for elements that were
/// never rendered.
#[test]
fn omits_filter_field_when_there_are_no_conversations() {
    let html = render(&[]).into_string();

    assert!(html.contains("No conversations yet."), "{html}");
    assert!(
        !html.contains(r#"id="filter""#),
        "filter field shown: {html}"
    );
    // The filter's script specifically. The page carries others regardless of how
    // many conversations there are, so "no script at all" would assert something
    // this test is not about.
    assert!(
        !html.contains("getElementById('filter')"),
        "filter script emitted: {html}"
    );
}
