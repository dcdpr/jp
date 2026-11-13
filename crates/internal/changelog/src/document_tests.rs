use std::num::NonZeroU64;

use indoc::indoc;

use super::*;
use crate::{
    fragment::{self, Type},
    highlight,
};

fn change_request(number: u64) -> ListedCredit {
    ListedCredit::ChangeRequest(NonZeroU64::new(number).unwrap())
}

#[test]
fn a_credit_is_a_change_request_or_a_commit() {
    assert_eq!(ListedCredit::parse("#1207"), Some(change_request(1207)));
    assert_eq!(ListedCredit::parse(" !1207 "), Some(change_request(1207)));
    assert_eq!(
        ListedCredit::parse("4E1F0A2"),
        Some(ListedCredit::Commit("4e1f0a2".to_owned()))
    );
    let sha256 = "a".repeat(64);
    assert_eq!(
        ListedCredit::parse(&sha256),
        Some(ListedCredit::Commit(sha256.clone()))
    );

    for neither in ["#0", "#", "1207", "4e1f0a", "xyz1234", "@JeanMertz", ""] {
        assert_eq!(ListedCredit::parse(neither), None, "{neither:?}");
    }
    assert_eq!(ListedCredit::parse(&"a".repeat(65)), None);
}

#[test]
fn a_credit_is_written_as_its_forge_writes_it() {
    let merge_request = change_request(1207);
    let commit = ListedCredit::Commit("4e1f0a2".to_owned());

    assert_eq!(merge_request.written(Forge::GitHub), "#1207");
    assert_eq!(merge_request.written(Forge::None), "#1207");
    assert_eq!(merge_request.written(Forge::GitLab), "!1207");
    assert_eq!(commit.written(Forge::GitLab), "4e1f0a2");
    assert_eq!(merge_request.to_string(), "#1207");
}

#[test]
fn credits_open_a_fragment() {
    let source = "A turn that stalls while idle is retried.\n";

    let written = with_credits(
        source,
        false,
        &[
            change_request(1207),
            ListedCredit::Commit("4e1f0a2".to_owned()),
        ],
        Forge::GitHub,
    );

    assert_eq!(written, indoc! {"
        - **Credits**: #1207
        - **Credits**: 4e1f0a2

        A turn that stalls while idle is retried.
    "});
    assert_eq!(
        fragment::parse(Type::Fixed, &written).unwrap().credits,
        Some(vec![
            change_request(1207),
            ListedCredit::Commit("4e1f0a2".to_owned())
        ])
    );
}

#[test]
fn a_highlights_credits_follow_its_authors() {
    let source = indoc! {"
        # Tool calls fan out

        - **Authors**: @JeanMertz

        Prose.
    "};

    let written = with_credits(source, true, &[change_request(1182)], Forge::GitLab);

    assert_eq!(written, indoc! {"
        # Tool calls fan out

        - **Authors**: @JeanMertz
        - **Credits**: !1182

        Prose.
    "});
    assert_eq!(
        highlight::parse(&written).unwrap().credits,
        Some(vec![change_request(1182)])
    );
}

/// A file written on Windows keeps its line endings, and its byte order mark.
#[test]
fn the_rest_of_the_file_is_kept_as_it_is() {
    let written = with_credits(
        "\u{feff}Entry.\r\n",
        false,
        &[change_request(7)],
        Forge::GitHub,
    );

    assert_eq!(written, "\u{feff}- **Credits**: #7\r\n\r\nEntry.\r\n");
}

/// A fragment's block opens the file, even when its entry opens with a heading.
#[test]
fn a_fragment_opening_with_a_heading_gets_its_credits_above_it() {
    let written = with_credits(
        "# Heading\n\nEntry.\n",
        false,
        &[change_request(7)],
        Forge::GitHub,
    );

    assert_eq!(written, "- **Credits**: #7\n\n# Heading\n\nEntry.\n");
    assert_eq!(
        fragment::parse(Type::Fixed, &written).unwrap().credits,
        Some(vec![change_request(7)])
    );
}

#[test]
fn no_credits_leave_the_file_as_it_is() {
    assert_eq!(
        with_credits("Entry.\n", false, &[], Forge::GitHub),
        "Entry.\n"
    );
}

/// A value long enough to wrap continues on indented lines, and a formatter's
/// escapes are undone.
#[test]
fn a_field_reads_with_the_lines_it_wraps_onto() {
    let lines = [
        "- **Authors**: Jean",
        "  Mertz",
        "- **Credits**: #12\\_3",
        "Prose.",
    ];

    let (fields, end) = fields(&lines, 0);

    assert_eq!(fields, vec![
        Field {
            key: "Authors".to_owned(),
            value: "Jean Mertz".to_owned(),
            line: 1,
        },
        Field {
            key: "Credits".to_owned(),
            value: "#12_3".to_owned(),
            line: 3,
        },
    ]);
    assert_eq!(end, 3);
}

#[test]
fn a_file_without_fields_has_an_empty_block() {
    assert_eq!(fields(&["Entry."], 0), (vec![], 0));
    assert_eq!(fields(&[], 0), (vec![], 0));
}
