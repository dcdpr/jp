use jp_review_api::Outcome;
use pretty_assertions::assert_eq;

use super::*;

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
fn a_time_is_shown_to_the_minute() {
    assert_eq!(
        time("2026-11-17T10:04:59Z".parse().unwrap()),
        "2026-11-17 10:04 UTC"
    );
}

#[test]
fn hidden_lines_are_counted_in_words() {
    assert_eq!(hidden(1), "1 changed line not shown");
    assert_eq!(hidden(2), "2 changed lines not shown");
}
