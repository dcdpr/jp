use jp_review_api::{AnchorView, Placed, Role};
use pretty_assertions::assert_eq;

use super::*;
use crate::composer::{Kind, Target};

fn comment(path: Option<&str>, range: Option<&str>, bytes: Option<[usize; 2]>) -> Comment {
    comment_with(Status::Open, path, range, bytes)
}

fn comment_with(
    status: Status,
    path: Option<&str>,
    range: Option<&str>,
    bytes: Option<[usize; 2]>,
) -> Comment {
    let outdated = range.is_some() && bytes.is_none();
    Comment {
        id: "R-1h4k2m9#1".to_owned(),
        status,
        role: Role::Reviewer,
        author: "Jane".to_owned(),
        created_at: "2026-11-17T10:00:00Z".parse().unwrap(),
        anchor: AnchorView {
            version: 1,
            path: path.map(ToOwned::to_owned),
            range: range.map(ToOwned::to_owned),
            projected: (!outdated).then_some(Placed {
                version: 2,
                range: range.map(ToOwned::to_owned),
                bytes,
            }),
            outdated,
            resolved: None,
        },
        excerpt: None,
        body: "Body.".to_owned(),
        body_html: "<p>Body.</p>\n".to_owned(),
        suggestion: None,
        replies: vec![],
        draft_reply: None,
        resolved_at: None,
    }
}

/// `comment`, numbered `number` in its review.
fn numbered(number: u32, comment: Comment) -> Comment {
    Comment {
        id: format!("R-1h4k2m9#{number}"),
        ..comment
    }
}

#[test]
fn the_panel_lists_general_comments_as_written_and_then_ranges_as_they_start() {
    let comments = [
        comment(Some("a.md"), Some("3"), Some([20, 30])),
        comment(None, None, None),
        comment(Some("a.md"), Some("1:3-1:5"), Some([5, 9])),
        comment(Some("a.md"), None, None),
        // On text that changed since.
        comment(Some("a.md"), Some("1"), None),
        comment(Some("b.md"), Some("1"), Some([0, 3])),
    ];

    assert_eq!(order(&comments, "a.md", false), [1, 3, 4, 2, 0]);
}

#[test]
fn two_ranges_starting_together_are_listed_as_written() {
    let comments = [
        comment(Some("a.md"), Some("1"), Some([5, 30])),
        comment(Some("a.md"), Some("1:1-1:3"), Some([5, 8])),
    ];

    assert_eq!(order(&comments, "a.md", false), [0, 1]);
}

#[test]
fn resolved_comments_are_listed_only_when_asked_for() {
    let comments = [
        comment_with(Status::Resolved, Some("a.md"), Some("1"), Some([0, 3])),
        comment_with(Status::Open, Some("a.md"), Some("2"), Some([4, 6])),
        comment_with(Status::Resolved, None, None, None),
        comment_with(Status::Resolved, Some("b.md"), None, None),
    ];

    assert_eq!(order(&comments, "a.md", false), [1]);
    assert_eq!(order(&comments, "a.md", true), [2, 0, 1]);
    assert_eq!(resolved(&comments, "a.md"), 2);
}

#[test]
fn a_passage_is_the_range_of_the_file_a_comment_stands_on() {
    let ranged = comment(Some("a.md"), Some("1:3-1:5"), Some([2, 4]));

    assert_eq!(passage(&ranged, "a.md"), Some([2, 4]));
    assert_eq!(passage(&ranged, "b.md"), None, "another file's");
    assert_eq!(passage(&comment(Some("a.md"), None, None), "a.md"), None);
    assert_eq!(
        passage(&comment(Some("a.md"), Some("1"), None), "a.md"),
        None,
        "the text changed since"
    );
    assert_eq!(passage(&comment(None, None, None), "a.md"), None);
}

#[test]
fn the_passages_of_a_file_are_those_highlighted_in_it() {
    let comments = [
        numbered(1, comment(Some("a.md"), Some("1"), Some([0, 3]))),
        numbered(2, comment(None, None, None)),
        numbered(
            3,
            comment_with(Status::Resolved, Some("a.md"), Some("3"), Some([7, 9])),
        ),
        numbered(4, comment(Some("b.md"), Some("1"), Some([0, 1]))),
    ];

    assert_eq!(passages(&comments, "a.md", false), [(1, [0, 3])]);
    assert_eq!(passages(&comments, "a.md", true), [
        (1, [0, 3]),
        (3, [7, 9])
    ]);
}

/// A comment on a word inside a commented paragraph is reached through the
/// word.
#[test]
fn a_point_in_two_passages_brings_out_the_shorter_one() {
    assert_eq!(next(&[(1, [0, 40]), (2, [10, 14])], None), Some(2));
    assert_eq!(next(&[(1, [0, 4]), (2, [0, 4])], None), Some(1));
    assert_eq!(next(&[], None), None);
    // The comment picked is on other text.
    assert_eq!(next(&[(1, [0, 40]), (2, [10, 14])], Some(7)), Some(2));
}

/// Comments 2 and 3 are on the same word, inside the paragraph comment 1 is on.
#[test]
fn a_point_picked_again_brings_out_each_comment_on_it_in_turn() {
    let hits = [(1, [0, 40]), (2, [10, 14]), (3, [10, 14])];

    assert_eq!(next(&hits, Some(2)), Some(3));
    assert_eq!(next(&hits, Some(3)), Some(1));
    assert_eq!(next(&hits, Some(1)), Some(2));
    assert_eq!(next(&[(4, [0, 4])], Some(4)), Some(4), "the only one stays");
}

/// Comment 1 is on bytes 10 to 20 of `a.md`, comment 2 on the whole file, and
/// comment 3 on text of it that changed since.
fn threads() -> [Comment; 3] {
    [
        numbered(1, comment(Some("a.md"), Some("2"), Some([10, 20]))),
        numbered(2, comment(Some("a.md"), None, None)),
        numbered(3, comment(Some("a.md"), Some("4"), None)),
    ]
}

fn range(path: &str, version: u32) -> Kind {
    Kind::Comment {
        on: Target::Range {
            path: path.to_owned(),
            version,
            bytes: [30, 40],
            excerpt: "selected".to_owned(),
        },
    }
}

#[test]
fn a_new_comment_on_a_range_is_written_under_the_range() {
    let draft = range("a.md", 2);

    assert_eq!(
        inline(Some(&draft), None, &threads(), "a.md", [2, 2]),
        Some(Inline::Composer([30, 40]))
    );
}

/// The range was captured in another file, or in another version than the one
/// shown, whose bytes lie elsewhere.
#[test]
fn a_new_comment_on_a_range_not_shown_is_written_beside_the_file() {
    assert_eq!(
        inline(Some(&range("b.md", 2)), None, &threads(), "a.md", [2, 2]),
        None
    );
    assert_eq!(
        inline(Some(&range("a.md", 1)), None, &threads(), "a.md", [2, 2]),
        None
    );
}

#[test]
fn an_edit_is_written_under_the_passage_of_the_comment_it_changes() {
    let edit = Kind::Edit {
        number: 1,
        excerpt: None,
    };

    assert_eq!(
        inline(Some(&edit), None, &threads(), "a.md", [2, 2]),
        Some(Inline::Composer([10, 20]))
    );
}

#[test]
fn a_reply_is_written_under_the_thread_it_joins() {
    let reply = Kind::Reply { number: 1 };

    assert_eq!(
        inline(Some(&reply), None, &threads(), "a.md", [2, 2]),
        Some(Inline::Thread {
            number: 1,
            bytes: [10, 20],
            replying: true,
        })
    );
}

/// A comment on the whole file, and one on text that changed since, have no
/// passage to sit under.
#[test]
fn what_is_about_no_passage_is_written_beside_the_file() {
    let whole = Kind::Edit {
        number: 2,
        excerpt: None,
    };
    let outdated = Kind::Reply { number: 3 };

    assert_eq!(inline(Some(&whole), None, &threads(), "a.md", [2, 2]), None);
    assert_eq!(
        inline(Some(&outdated), None, &threads(), "a.md", [2, 2]),
        None
    );
}

#[test]
fn the_comment_picked_shows_its_thread_under_its_passage() {
    let thread = Some(Inline::Thread {
        number: 1,
        bytes: [10, 20],
        replying: false,
    });

    assert_eq!(inline(None, Some(1), &threads(), "a.md", [2, 2]), thread);
    assert_eq!(
        inline(Some(&Kind::Pass), Some(1), &threads(), "a.md", [2, 2]),
        thread,
        "while a pass message is written beside the file"
    );
    assert_eq!(inline(None, Some(2), &threads(), "a.md", [2, 2]), None);
    assert_eq!(inline(None, Some(9), &threads(), "a.md", [2, 2]), None);
    assert_eq!(inline(None, None, &threads(), "a.md", [2, 2]), None);
}

/// The composer wins: one thing sits in the file at a time.
#[test]
fn what_is_being_written_takes_the_place_of_the_thread_picked() {
    assert_eq!(
        inline(Some(&range("a.md", 2)), Some(1), &threads(), "a.md", [2, 2]),
        Some(Inline::Composer([30, 40]))
    );
}

/// While another version loads, the file shown is not the one the comments'
/// passages are placed in.
#[test]
fn a_thread_waits_for_the_version_its_passage_is_placed_in() {
    assert_eq!(inline(None, Some(1), &threads(), "a.md", [1, 2]), None);
}

/// Approval `number`, made in version `version` of `path`, standing at `bytes`
/// in the version shown, or of text that changed since when `bytes` is `None`.
fn approval(number: u32, path: &str, version: u32, bytes: Option<[usize; 2]>) -> Approval {
    Approval {
        number,
        author: "Jane".to_owned(),
        created_at: "2026-11-17T10:00:00Z".parse().unwrap(),
        anchor: AnchorView {
            version,
            path: Some(path.to_owned()),
            range: Some("3".to_owned()),
            projected: bytes.map(|bytes| Placed {
                version: 2,
                range: Some("3".to_owned()),
                bytes: Some(bytes),
            }),
            outdated: bytes.is_none(),
            resolved: None,
        },
        excerpt: Some("Approved text.".to_owned()),
    }
}

#[test]
fn the_passages_approved_are_those_standing_in_the_file_shown() {
    let approvals = [
        approval(1, "a.md", 1, Some([10, 20])),
        approval(2, "b.md", 1, Some([0, 5])),
        approval(3, "a.md", 1, None),
        approval(4, "a.md", 2, Some([40, 60])),
    ];

    assert_eq!(approved(&approvals, "a.md"), [(1, [10, 20]), (4, [40, 60])]);
    assert_eq!(approved(&approvals, "c.md"), []);
}

/// Approval 3 was made in version 3, of text version 2 doesn't hold yet.
#[test]
fn an_approval_whose_text_changed_is_listed_as_changed_since() {
    let approvals = [
        approval(1, "a.md", 1, Some([10, 20])),
        approval(2, "a.md", 1, None),
        approval(3, "a.md", 3, None),
        approval(4, "b.md", 1, None),
    ];

    let changed: Vec<u32> = changed_since(&approvals, "a.md", 2)
        .into_iter()
        .map(|approval| approval.number)
        .collect();

    assert_eq!(changed, [2]);
}

fn read(path: &str, bytes: Option<[usize; 2]>) -> ReadUntil {
    ReadUntil {
        path: path.to_owned(),
        author: "Jane".to_owned(),
        created_at: "2026-11-17T10:00:00Z".parse().unwrap(),
        version: 1,
        line: 40,
        projected: bytes.map(|bytes| Placed {
            version: 2,
            range: Some("42".to_owned()),
            bytes: Some(bytes),
        }),
    }
}

#[test]
fn the_line_read_to_is_the_one_of_the_file_shown() {
    let read = [read("a.md", Some([900, 931])), read("b.md", None)];

    assert_eq!(
        read_line(&read, "a.md"),
        Some(ReadLine {
            line: "42".to_owned(),
            bytes: [900, 931],
        })
    );
    assert_eq!(read_line(&read, "b.md"), None, "none of it is left");
    assert_eq!(read_line(&read, "c.md"), None, "not said");
}

#[test]
fn the_composer_sits_in_the_file_when_it_writes_about_a_passage_shown() {
    let reply = Kind::Reply { number: 1 };

    assert_eq!(site(&range("a.md", 2), true, true), Site::File);
    assert_eq!(site(&reply, true, true), Site::File);
}

#[test]
fn a_wide_screen_opens_the_composer_under_the_button_that_asked_for_it() {
    let on_review = Kind::Comment { on: Target::Review };
    let on_file = Kind::Comment {
        on: Target::File {
            path: "a.md".to_owned(),
            version: 2,
        },
    };
    // On a comment with no passage in the file shown.
    let edit = Kind::Edit {
        number: 2,
        excerpt: None,
    };

    assert_eq!(site(&Kind::Pass, false, true), Site::Send);
    assert_eq!(site(&on_review, false, true), Site::Comments);
    assert_eq!(site(&on_file, false, true), Site::Comments);
    assert_eq!(site(&edit, false, true), Site::Comments);
    assert_eq!(
        site(&Kind::Reply { number: 3 }, false, true),
        Site::Comments
    );
    // A range of a file or a version that isn't the one shown.
    assert_eq!(site(&range("b.md", 2), false, true), Site::Comments);
}

#[test]
fn a_narrow_screen_keeps_the_composer_in_a_sheet() {
    assert_eq!(site(&Kind::Pass, false, false), Site::Sheet);
    assert_eq!(site(&range("a.md", 2), false, false), Site::Sheet);
    assert_eq!(
        site(&Kind::Comment { on: Target::Review }, false, false),
        Site::Sheet
    );
}

fn heading(level: u8, anchor: &str) -> Heading {
    Heading {
        level,
        text: anchor.to_owned(),
        anchor: anchor.to_owned(),
    }
}

#[test]
fn the_outline_lists_sections_and_their_subsections() {
    let headings = [
        heading(1, "title"),
        heading(2, "design"),
        heading(3, "layout"),
        heading(4, "detail"),
        heading(2, "risks"),
    ];

    let listed: Vec<&str> = outline(&headings)
        .into_iter()
        .map(|heading| heading.anchor.as_str())
        .collect();

    assert_eq!(listed, ["design", "layout", "risks"]);
}

#[test]
fn the_heading_being_read_is_the_last_one_scrolled_past_the_line() {
    let tops = [-900.0, -40.0, 96.0, 700.0];

    assert_eq!(reading(&tops, 96.0), Some(2), "at the line");
    assert_eq!(reading(&tops, 95.0), Some(1));
    assert_eq!(reading(&[300.0, 700.0], 96.0), None, "above the first");
    assert_eq!(reading(&[], 96.0), None);
}

#[test]
#[expect(clippy::float_cmp, reason = "whole pixels subtract exactly")]
fn a_passage_that_shows_is_left_where_it_is() {
    assert_eq!(scroll_for([120.0, 160.0], [96.0, 800.0]), 0.0);
}

#[test]
#[expect(clippy::float_cmp, reason = "whole pixels subtract exactly")]
fn a_passage_out_of_sight_is_scrolled_to_the_top_of_the_band() {
    assert_eq!(scroll_for([1200.0, 1240.0], [96.0, 800.0]), 1104.0, "below");
    assert_eq!(scroll_for([-300.0, -260.0], [96.0, 800.0]), -396.0, "above");
}

/// The band ends where a sheet covers the bottom of a narrow screen.
#[test]
#[expect(clippy::float_cmp, reason = "whole pixels subtract exactly")]
fn a_passage_under_the_end_of_the_band_is_scrolled_up_into_it() {
    assert_eq!(scroll_for([400.0, 440.0], [96.0, 310.0]), 304.0);
    assert_eq!(scroll_for([280.0, 330.0], [96.0, 310.0]), 184.0, "half in");
}

#[test]
fn each_status_has_its_highlight_and_resolved_ones_show_when_asked() {
    let comments = [
        comment_with(Status::Draft, Some("a.md"), Some("1"), Some([0, 3])),
        comment_with(Status::Open, Some("a.md"), Some("2"), Some([4, 6])),
        comment_with(Status::Resolved, Some("a.md"), Some("3"), Some([7, 9])),
        comment_with(Status::Open, Some("a.md"), Some("4"), None),
        comment_with(Status::Open, Some("b.md"), Some("1"), Some([0, 1])),
    ];

    assert_eq!(highlights(&comments, "a.md", false), [
        (DRAFT, [0, 3]),
        (OPEN, [4, 6])
    ]);
    assert_eq!(highlights(&comments, "a.md", true), [
        (DRAFT, [0, 3]),
        (OPEN, [4, 6]),
        (RESOLVED, [7, 9])
    ]);
    assert_eq!(unresolved(&comments, "a.md"), 3);
}
