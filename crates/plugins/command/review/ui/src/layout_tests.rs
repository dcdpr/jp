use jp_review_api::{AnchorView, Placed, Role};
use pretty_assertions::assert_eq;

use super::*;

fn block(bytes: Option<[usize; 2]>) -> Block {
    Block {
        html: String::new(),
        bytes,
        line: false,
    }
}

/// A paragraph at bytes 0 to 10, a rule, and a list whose first item's text
/// starts at 14, after its `- ` marker.
fn rendered() -> [Block; 3] {
    [block(Some([0, 10])), block(None), block(Some([14, 30]))]
}

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

#[test]
fn a_ranged_comment_goes_under_the_block_its_range_starts_in() {
    let comments = [
        comment(Some("a.md"), Some("1:3-1:5"), Some([2, 4])),
        // The start of the list item's line, before its marker's text.
        comment(Some("a.md"), Some("3"), Some([12, 30])),
        comment(Some("a.md"), Some("9"), Some([90, 95])),
    ];

    let placement = place(&comments, "a.md", &rendered());

    assert_eq!(placement, Placement {
        above: vec![],
        under: vec![vec![0], vec![], vec![1, 2]],
    });
}

#[test]
fn comments_on_the_review_the_file_or_changed_text_go_above_it() {
    let comments = [
        comment(None, None, None),
        comment(Some("a.md"), None, None),
        comment(Some("a.md"), Some("1"), None),
        comment(Some("b.md"), Some("1"), Some([0, 3])),
    ];

    let placement = place(&comments, "a.md", &rendered());

    assert_eq!(placement, Placement {
        above: vec![0, 1, 2],
        under: vec![vec![]; 3],
    });
}

#[test]
fn a_file_with_no_text_puts_every_comment_above_it() {
    let comments = [comment(Some("a.md"), Some("1"), Some([0, 3]))];

    assert_eq!(
        place(&comments, "a.md", &[block(None)]).above,
        [0],
        "no block to go under"
    );
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
