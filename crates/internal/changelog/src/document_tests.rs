use indoc::indoc;

use super::*;
use crate::{fragment, highlight};

fn numbers(numbers: &[u64]) -> Vec<NonZeroU64> {
    numbers
        .iter()
        .map(|&number| NonZeroU64::new(number).unwrap())
        .collect()
}

#[test]
fn change_requests_are_written_after_every_field() {
    let source = indoc! {"
        ---
        type: fixed
        ---
        A turn that stalls while idle is retried.
    "};

    let written = with_change_requests(source, &numbers(&[1207, 1211])).unwrap();

    assert_eq!(written, indoc! {"
        ---
        type: fixed
        change_requests: [1207, 1211]
        ---
        A turn that stalls while idle is retried.
    "});
    assert_eq!(
        fragment::parse(&written).unwrap().change_requests,
        Some(numbers(&[1207, 1211]))
    );
}

#[test]
fn a_highlights_change_requests_are_written_the_same_way() {
    let source = "---\ntitle: Tool calls fan out\nauthors: [\"@JeanMertz\"]\n---\nProse.\n";

    let written = with_change_requests(source, &numbers(&[1182])).unwrap();

    assert_eq!(
        highlight::parse(&written).unwrap().change_requests,
        Some(numbers(&[1182]))
    );
}

/// A file written on Windows keeps its line endings, and its byte order mark.
#[test]
fn the_rest_of_the_file_is_kept_as_it_is() {
    let source = "\u{feff}---\r\ntype: fixed\r\n---\r\nEntry.\r\n";

    let written = with_change_requests(source, &numbers(&[7])).unwrap();

    assert_eq!(
        written,
        "\u{feff}---\r\ntype: fixed\r\nchange_requests: [7]\r\n---\r\nEntry.\r\n"
    );
}

#[test]
fn a_file_without_frontmatter_has_nowhere_to_write_them() {
    assert_eq!(
        with_change_requests("type: fixed\n\nEntry.\n", &numbers(&[7])),
        Err(FrontmatterError::Missing)
    );
    assert_eq!(
        with_change_requests("---\ntype: fixed\nEntry.\n", &numbers(&[7])),
        Err(FrontmatterError::Unclosed)
    );
}
