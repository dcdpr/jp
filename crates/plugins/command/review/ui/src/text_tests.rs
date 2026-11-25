use std::collections::BTreeMap;

use jp_review_api::Outcome;
use pretty_assertions::assert_eq;

use super::*;

fn recorded(number: u32, label: Option<&str>) -> Version {
    Version {
        number,
        label: label.map(ToOwned::to_owned),
        created_at: "2026-11-17T10:00:00Z".parse().unwrap(),
        files: BTreeMap::new(),
    }
}

#[test]
fn each_diff_line_says_what_it_is() {
    let diff = "@@ -1,2 +1,2 @@\n a\n-b\n+B\n\\ No newline at end of file\n";

    assert_eq!(diff_lines(diff), [
        (Line::Hunk, "@@ -1,2 +1,2 @@"),
        (Line::Context, " a"),
        (Line::Removed, "-b"),
        (Line::Added, "+B"),
        (Line::Note, "\\ No newline at end of file"),
    ]);
}

#[test]
fn a_suggestion_removes_its_range_and_adds_its_replacement() {
    assert_eq!(suggestion("is **Abandoned", "is abandoned\nfor now"), [
        (Line::Removed, "-is **Abandoned".to_owned()),
        (Line::Added, "+is abandoned".to_owned()),
        (Line::Added, "+for now".to_owned()),
    ]);
    assert_eq!(suggestion("gone", ""), [(
        Line::Removed,
        "-gone".to_owned()
    )]);
}

#[test]
fn comment_counts_name_each_status_there_is() {
    assert_eq!(
        counts(&Counts {
            draft: 1,
            open: 0,
            resolved: 2,
        }),
        "1 draft, 2 resolved"
    );
    assert_eq!(counts(&Counts::default()), "no comments");
}

#[test]
fn a_pass_says_where_it_runs_and_how_it_ended() {
    let running_here = Running {
        number: 2,
        conversation: None,
        stoppable: true,
    };
    let elsewhere = Running {
        stoppable: false,
        ..running_here.clone()
    };
    let ended = Pass {
        number: 1,
        author: "Jane".to_owned(),
        created_at: "2026-11-17T10:00:00Z".parse().unwrap(),
        version: 1,
        comments: vec![],
        message: None,
        conversation: None,
        ended: Some(Ended {
            outcome: Outcome::Stopped,
            at: "2026-11-17T10:04:00Z".parse().unwrap(),
            version: Some(2),
            error: None,
            answer: None,
        }),
    };

    assert_eq!(running(&running_here), "Pass 2 running");
    assert_eq!(running(&elsewhere), "Pass 2 running in another process");
    assert_eq!(pass(&ended), "Pass 1 stopped");
}

#[test]
fn a_version_is_listed_with_what_sets_it_apart() {
    assert_eq!(version(&recorded(2, None), false), "Version 2");
    assert_eq!(
        version(&recorded(1, Some("base")), false),
        "Version 1 (base)"
    );
    assert_eq!(version(&recorded(3, None), true), "Version 3 (latest)");
    assert_eq!(
        version(&recorded(1, Some("base")), true),
        "Version 1 (base, latest)"
    );
}

#[test]
fn the_header_names_what_the_page_shows_in_full_and_in_short() {
    assert_eq!(
        showing(Showing::Latest, 3),
        ("Version 3".to_owned(), "v3".to_owned())
    );
    assert_eq!(
        showing(Showing::Version(2), 3),
        ("Version 2".to_owned(), "v2".to_owned())
    );
    assert_eq!(
        showing(Showing::Pass(2), 3),
        ("Pass 2 changes".to_owned(), "Pass 2".to_owned())
    );
    assert_eq!(
        showing(Showing::Compare, 3),
        ("Compare versions".to_owned(), "Compare".to_owned())
    );
}

#[test]
fn the_send_button_says_what_it_sends() {
    assert_eq!(send(0), ("Send".to_owned(), "Send".to_owned()));
    assert_eq!(send(1), ("Send 1 comment".to_owned(), "Send 1".to_owned()));
    assert_eq!(send(2), ("Send 2 comments".to_owned(), "Send 2".to_owned()));
}

#[test]
fn a_file_is_listed_with_its_unresolved_comments() {
    assert_eq!(file("docs/a.md", 0), "docs/a.md");
    assert_eq!(file("docs/a.md", 2), "docs/a.md (2 unresolved)");
}

#[test]
fn the_resolved_switch_says_what_it_does_next() {
    assert_eq!(resolved(2, false), "Show 2 resolved");
    assert_eq!(resolved(2, true), "Hide resolved");
}

#[test]
fn a_time_is_shown_to_the_minute() {
    assert_eq!(
        time("2026-11-17T10:04:59Z".parse().unwrap()),
        "2026-11-17 10:04 UTC"
    );
}

/// A marker under a block that runs past the line read says so, since the lines
/// under it in that block were not read.
#[test]
fn the_read_marker_names_its_line() {
    assert_eq!(read_until("42", false), "Read until line 42");
    assert_eq!(
        read_until("42", true),
        "Read until line 42, within the block above"
    );
}

#[test]
fn hidden_lines_are_counted_in_words() {
    assert_eq!(hidden(1), "1 changed line not shown");
    assert_eq!(hidden(2), "2 changed lines not shown");
}
