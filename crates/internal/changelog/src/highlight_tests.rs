use indoc::indoc;

use super::*;

#[test]
fn a_highlight_holds_a_title_the_authors_to_credit_and_its_prose() {
    let highlight = parse(indoc! {r#"
        ---
        title: "Unified editor and inline reply widget"
        authors: ["@JeanMertz"]
        ---
        Every prompt in JP now runs through a single editor service, and short
        replies can be written inline without leaving the conversation view.
    "#})
    .unwrap();

    assert_eq!(highlight, Highlight {
        title: "Unified editor and inline reply widget".to_owned(),
        authors: vec!["@JeanMertz".to_owned()],
        change_requests: None,
        body: indoc! {"
            Every prompt in JP now runs through a single editor service, and short
            replies can be written inline without leaving the conversation view."}
        .to_owned(),
    });
}

#[test]
fn change_requests_replace_the_credits_history_names() {
    let highlight = parse(indoc! {r#"
        ---
        title: Tool calls fan out
        authors: ["@JeanMertz"]
        change_requests: [1182, 1183]
        ---
        The assistant calls one tool on many inputs at once.
    "#})
    .unwrap();

    assert_eq!(
        highlight.change_requests,
        Some(vec![
            NonZeroU64::new(1182).unwrap(),
            NonZeroU64::new(1183).unwrap()
        ])
    );
}

#[test]
fn the_title_authors_and_prose_are_read_without_the_space_around_them() {
    // Quoted, so YAML keeps the spaces it would drop from a plain scalar.
    let highlight = parse(indoc! {r#"
        ---
        title: "  Tool calls fan out  "
        authors: [" @JeanMertz "]
        ---

        The assistant calls one tool on many inputs at once.

    "#})
    .unwrap();

    assert_eq!(highlight.title, "Tool calls fan out");
    assert_eq!(highlight.authors, vec!["@JeanMertz".to_owned()]);
    assert_eq!(
        highlight.body,
        "The assistant calls one tool on many inputs at once."
    );
}

#[test]
fn a_highlight_needs_a_title() {
    let missing = parse("---\nauthors: [\"@JeanMertz\"]\n---\nProse.\n").unwrap_err();
    assert_eq!(
        missing.to_string(),
        "invalid frontmatter: missing field `title` at line 2, column 1"
    );

    let blank = parse("---\ntitle: \"  \"\nauthors: [\"@JeanMertz\"]\n---\nProse.\n").unwrap_err();
    assert_eq!(blank, HighlightError::EmptyTitle);
}

#[test]
fn a_highlight_credits_at_least_one_author() {
    let missing = parse("---\ntitle: Tool calls fan out\n---\nProse.\n").unwrap_err();
    assert_eq!(
        missing.to_string(),
        "invalid frontmatter: missing field `authors` at line 2, column 1"
    );

    let none = parse("---\ntitle: Tool calls fan out\nauthors: []\n---\nProse.\n").unwrap_err();
    assert_eq!(none, HighlightError::NoAuthors);
}

#[test]
fn every_author_names_someone() {
    let error =
        parse("---\ntitle: Tool calls fan out\nauthors: [\"@JeanMertz\", \" \"]\n---\nProse.\n")
            .unwrap_err();

    assert_eq!(error, HighlightError::EmptyAuthor);
}

#[test]
fn a_highlight_has_no_type() {
    // A fragment's field in a highlight's frontmatter, which strict parsing
    // reports rather than drops.
    let error = parse(indoc! {r#"
        ---
        type: added
        title: Tool calls fan out
        authors: ["@JeanMertz"]
        ---
        Prose.
    "#})
    .unwrap_err();

    assert_eq!(
        error.to_string(),
        "invalid frontmatter: unknown field `type`, expected one of title, authors, \
         change_requests at line 2, column 1"
    );
}

#[test]
fn a_misspelled_field_is_an_error_rather_than_ignored() {
    let error = parse("---\ntitle: Tool calls fan out\nauthor: [\"@JeanMertz\"]\n---\nProse.\n")
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "invalid frontmatter: unknown field `author`, expected one of title, authors, \
         change_requests at line 3, column 1"
    );
}

#[test]
fn a_highlight_opens_with_its_frontmatter() {
    assert_eq!(
        parse("title: Tool calls fan out\n---\nProse.\n"),
        Err(HighlightError::Frontmatter(FrontmatterError::Missing))
    );
}

#[test]
fn a_highlight_needs_prose() {
    assert_eq!(
        parse("---\ntitle: Tool calls fan out\nauthors: [\"@JeanMertz\"]\n---\n\n  \n"),
        Err(HighlightError::EmptyBody)
    );
}

#[test]
fn a_highlight_carries_no_upgrade_section() {
    let error = parse(indoc! {r#"
        ---
        title: Plugins are found by their manifest
        authors: ["@JeanMertz"]
        ---
        JP reads the manifest each plugin binary embeds.

        BREAKING CHANGE: Command plugins are found by their manifest

        Rebuild your plugins against this release.
    "#})
    .unwrap_err();

    assert_eq!(error, HighlightError::Marker {
        line: 7,
        marker: "BREAKING CHANGE:",
    });
}

#[test]
fn a_token_starting_any_line_of_a_highlight_is_an_error() {
    // The rule fragments follow, where a line starting with the token is never
    // text, with no line left where a highlight could open an upgrade section.
    let error = parse(indoc! {r#"
        ---
        title: Breaking changes in commit messages
        authors: ["@JeanMertz"]
        ---
        A commit can mark a breaking change:

        ```text
        BREAKING-CHANGE: the flag is gone
        ```
    "#})
    .unwrap_err();

    assert_eq!(error, HighlightError::Marker {
        line: 8,
        marker: "BREAKING-CHANGE:",
    });
}

#[test]
fn a_token_that_does_not_start_its_line_is_text() {
    let highlight = parse(indoc! {r#"
        ---
        title: Upgrade sections
        authors: ["@JeanMertz"]
        ---
        `BREAKING CHANGE:` in inline code, and
         BREAKING CHANGE: indented, are both text.
    "#})
    .unwrap();

    assert_eq!(highlight.body, indoc! {"
        `BREAKING CHANGE:` in inline code, and
         BREAKING CHANGE: indented, are both text."});
}

/// Release notes place many highlights in one document, where two defining one
/// label differently would collide, so the prose comes with its links inline.
#[test]
fn a_highlights_links_are_inline() {
    let highlight = parse(indoc! {r#"
        ---
        title: Tool calls fan out
        authors: ["@JeanMertz"]
        ---
        The assistant calls [one tool] on many inputs at once.

        [one tool]: https://example.com/tools
    "#})
    .unwrap();

    assert_eq!(
        highlight.body,
        "The assistant calls [one tool](https://example.com/tools) on many inputs at once."
    );
}

#[test]
fn a_highlight_of_definitions_alone_has_no_prose() {
    assert_eq!(
        parse("---\ntitle: Fan out\nauthors: [\"@JeanMertz\"]\n---\n[docs]: https://example.com\n"),
        Err(HighlightError::EmptyBody)
    );
}

#[test]
fn a_misplaced_token_says_where_the_section_goes_instead() {
    assert_eq!(
        HighlightError::Marker {
            line: 7,
            marker: "BREAKING CHANGE:",
        }
        .to_string(),
        "line 7 starts with `BREAKING CHANGE:`, and a highlight has no upgrade section; write the \
         section in the fragment for the change, or, to show the token, indent the line or put \
         the token in inline code"
    );
}
