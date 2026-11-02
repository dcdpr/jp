use indoc::indoc;
use serde_json::Value;

use super::*;

/// Headings and the slugs the site gives them, kept as JSON so a test of the
/// site's slug function can read the same cases.
const FIXTURE: &str = include_str!("../fixtures/heading-slugs.json");

/// The strings of a fixture case's array `key`.
fn strings<'a>(case: &'a Value, key: &str) -> Vec<&'a str> {
    case[key]
        .as_array()
        .unwrap_or_else(|| panic!("{case} has no `{key}`"))
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect()
}

/// Every case in the fixture: each heading of its document reads as the text
/// the site reads, and takes the slug the site gives it.
///
/// The site's own test checks its slug function against the same texts and
/// slugs, so the two can't drift apart without one of them failing.
#[test]
fn slugs_match_the_fixture() {
    let cases: Value = serde_json::from_str(FIXTURE).unwrap();

    for case in cases.as_array().unwrap() {
        let name = case["case"].as_str().unwrap();
        let markdown = case["markdown"].as_str().unwrap();

        let found = headings(markdown);
        let texts: Vec<&str> = found.iter().map(|heading| heading.text.as_str()).collect();
        let slugs: Vec<&str> = found.iter().map(|heading| heading.slug.as_str()).collect();

        assert_eq!(texts, strings(case, "texts"), "{name}");
        assert_eq!(slugs, strings(case, "slugs"), "{name}");
    }
}

#[test]
fn a_heading_carries_its_level_text_and_line() {
    let source = indoc! {"
        # Ubiquitous Language

        Intro.

        ## Turn `jp_conversation::Turn`

        ### Pinned *Conversation*
    "};

    assert_eq!(headings(source), [
        Heading {
            level: 1,
            text: "Ubiquitous Language".to_owned(),
            slug: "ubiquitous-language".to_owned(),
            line: 0,
        },
        Heading {
            level: 2,
            text: "Turn jp_conversation::Turn".to_owned(),
            slug: "turn-jp-conversation-turn".to_owned(),
            line: 4,
        },
        Heading {
            level: 3,
            text: "Pinned Conversation".to_owned(),
            slug: "pinned-conversation".to_owned(),
            line: 6,
        },
    ]);
}

/// The frontmatter is read past, not dropped: a heading's line still counts
/// from the top of the file.
#[test]
fn lines_count_from_the_top_of_the_file() {
    let source = "---\ntitle: Front\n---\n\n## After\n";

    let found = headings(source);

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].line, 4);
}

#[test]
fn a_document_without_headings_has_none() {
    assert_eq!(headings("Prose.\n\n- a list\n"), []);
    assert_eq!(headings(""), []);
}

#[test]
fn slugify_drops_separators_at_either_end_and_collapses_runs() {
    assert_eq!(slugify("  --Hello,   World!--  "), "hello-world");
    assert_eq!(slugify("a -- b"), "a-b");
    assert_eq!(slugify("!!!"), "");
}

#[test]
fn slugify_decomposes_compatibility_characters() {
    // A ligature becomes its letters, and full-width punctuation separates.
    assert_eq!(slugify("ﬁle！name"), "file-name");
    // A non-breaking space is a space.
    assert_eq!(slugify("a\u{a0}b"), "a-b");
}

/// Whitespace NFKD leaves alone still separates, as JavaScript's `\s` matches
/// it.
#[test]
fn slugify_separates_at_whitespace_beyond_ascii() {
    assert_eq!(slugify("a\u{1680}b\u{2028}c\u{feff}d"), "a-b-c-d");
}

/// Control characters go before separators are found, so a tab joins the words
/// either side of it rather than separating them.
#[test]
fn slugify_drops_control_characters() {
    assert_eq!(slugify("a\tb"), "ab");
}

#[test]
fn slugify_prefixes_only_an_ascii_leading_digit() {
    assert_eq!(slugify("42"), "_42");
    assert_eq!(slugify("-1 step"), "_1-step");
    assert_eq!(slugify("Step 1"), "step-1");
}

#[test]
fn slugify_lowercases_beyond_ascii() {
    assert_eq!(slugify("ÉCOLE Straße"), "ecole-straße");
    // A capital sigma ending a word lowers to the final form, as it does in
    // JavaScript.
    assert_eq!(
        slugify("\u{3a3}\u{39f}\u{3a6}\u{39f}\u{3a3}"),
        "\u{3c3}\u{3bf}\u{3c6}\u{3bf}\u{3c2}"
    );
}
