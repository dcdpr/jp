//! Test suite ported from snapper-fmt's `sentence/unicode.rs`, MIT-licensed,
//! Copyright (c) 2026 Rohit Goswami.
//! Verifies that comfort's inlined English splitter behaves identically to the
//! upstream English configuration.

use pretty_assertions::assert_eq;

use super::split_sentences;

fn split(text: &str) -> Vec<String> {
    split_sentences(text, &[])
}

fn split_with_atomic(text: &str, atomic_ranges: &[std::ops::Range<usize>]) -> Vec<String> {
    split_sentences(text, atomic_ranges)
}

#[test]
fn simple_sentences() {
    assert_eq!(
        split("Hello world. This is a test. Another sentence here."),
        vec!["Hello world.", "This is a test.", "Another sentence here."]
    );
}

#[test]
fn abbreviation_dr() {
    assert_eq!(split("Dr. Smith went home. He was tired."), vec![
        "Dr. Smith went home.",
        "He was tired."
    ]);
}

#[test]
fn abbreviation_eg() {
    assert_eq!(
        split("Use a formatter, e.g. snapper. It works well."),
        vec!["Use a formatter, e.g. snapper.", "It works well."]
    );
}

#[test]
fn abbreviation_fig() {
    assert_eq!(
        split("See Fig. 3 for details. The results are clear."),
        vec!["See Fig. 3 for details.", "The results are clear."]
    );
}

#[test]
fn empty_input() {
    assert_eq!(split(""), Vec::<String>::new());
}

#[test]
fn single_sentence() {
    assert_eq!(split("Just one sentence."), vec!["Just one sentence."]);
}

#[test]
fn question_and_exclamation() {
    assert_eq!(split("Is this working? Yes! It is."), vec![
        "Is this working?",
        "Yes!",
        "It is."
    ]);
}

#[test]
fn no_trailing_period() {
    assert_eq!(split("First sentence. Second without period"), vec![
        "First sentence.",
        "Second without period"
    ]);
}

#[test]
fn inline_org_link_preserved() {
    assert_eq!(
        split("See [[https://example.com][Ex. Site]] for details. Then continue."),
        vec![
            "See [[https://example.com][Ex. Site]] for details.",
            "Then continue."
        ]
    );
}

#[test]
fn inline_math_preserved() {
    assert_eq!(split("The value $x = 3.14$ matters. Next sentence."), vec![
        "The value $x = 3.14$ matters.",
        "Next sentence."
    ]);
}

#[test]
fn inline_markdown_link_preserved() {
    assert_eq!(
        split("Visit [Example Inc.](https://example.com) now. Then read more."),
        vec![
            "Visit [Example Inc.](https://example.com) now.",
            "Then read more."
        ]
    );
}

#[test]
fn bold_span_with_period_does_not_split_mid_span() {
    // Regression: `**Heading.** Body.` used to split at the period inside
    // the bold span, stranding `**` on the next line.
    assert_eq!(split("**Heading.** Body sentence here."), vec![
        "**Heading.** Body sentence here."
    ]);
}

#[test]
fn bold_span_with_internal_period_then_real_sentence_break() {
    // The period inside the bold span doesn't break, but the period
    // outside it still does.
    assert_eq!(split("**Title.** First sentence. Second sentence."), vec![
        "**Title.** First sentence.",
        "Second sentence.",
    ]);
}

#[test]
fn atomic_range_protects_explicit_span() {
    // The caller (format.rs) marks the bold span as atomic via byte range.
    // The splitter must not break inside it, even though it contains a
    // sentence-terminator period.
    let text = "**Heading.** Body sentence here.";
    let bold = 0..text.find("** B").unwrap() + 2; // covers `**Heading.**`
    assert_eq!(split_with_atomic(text, &[bold]), vec![
        "**Heading.** Body sentence here."
    ]);
}

#[test]
fn atomic_range_does_not_swallow_following_sentence_break() {
    let text = "**Title.** First. Second.";
    let bold = 0..text.find("** F").unwrap() + 2;
    assert_eq!(split_with_atomic(text, &[bold]), vec![
        "**Title.** First.",
        "Second.",
    ]);
}

#[test]
fn atomic_range_overlapping_a_regex_match_dedupes_to_first() {
    // `**Heading.**` is matched by both the caller's AST atomic range AND
    // the bold-regex fallback. The caller's range wins; the regex match
    // gets dropped as overlapping.
    let text = "**Heading.** Body.";
    let bold = 0..12; // `**Heading.**`
    let out = split_with_atomic(text, &[bold]);
    assert_eq!(out, vec!["**Heading.** Body."]);
}

#[test]
#[allow(
    clippy::reversed_empty_ranges,
    reason = "testing malformed input on purpose"
)]
fn atomic_range_out_of_bounds_is_ignored() {
    // Defensive: malformed ranges shouldn't panic.
    let text = "Hello world.";
    let bogus = vec![100..200, 5..3];
    let out = split_with_atomic(text, &bogus);
    assert_eq!(out, vec!["Hello world."]);
}

#[test]
fn strikethrough_with_period_is_preserved() {
    assert_eq!(split("~~obsolete.~~ Still here."), vec![
        "~~obsolete.~~ Still here."
    ]);
}

#[test]
fn inline_code_preserved() {
    assert_eq!(split("Use `std.io.Read` for input. Then process."), vec![
        "Use `std.io.Read` for input.",
        "Then process."
    ]);
}

#[test]
fn quoted_exclamation_no_false_split() {
    assert_eq!(split(r#"He said "wow!" and left. She agreed."#), vec![
        r#"He said "wow!" and left."#,
        "She agreed."
    ]);
}

#[test]
fn paren_exclamation_no_false_split() {
    assert_eq!(
        split("He replied (with emphasis!) loudly. She agreed."),
        vec!["He replied (with emphasis!) loudly.", "She agreed."]
    );
}

#[test]
fn paren_question_no_false_split() {
    assert_eq!(
        split("The answer (really?) surprised them. Next sentence."),
        vec!["The answer (really?) surprised them.", "Next sentence."]
    );
}

#[test]
fn url_trailing_period_not_swallowed() {
    assert_eq!(
        split("Visit https://example.com/path. Then read more."),
        vec!["Visit https://example.com/path.", "Then read more."]
    );
}

#[test]
fn url_with_query_trailing_period() {
    assert_eq!(
        split("See https://example.com/path?q=1&r=2. Next sentence."),
        vec!["See https://example.com/path?q=1&r=2.", "Next sentence."]
    );
}

#[test]
fn ellipsis_splits() {
    assert_eq!(split("Sentence one... Sentence two."), vec![
        "Sentence one...",
        "Sentence two."
    ]);
}

#[test]
fn soft_line_breaks_are_collapsed_to_spaces() {
    // The text comes in with embedded newlines (markdown soft breaks).
    // Each output sentence must be one logical line — no `\n` leakage.
    let out = split("If foo, that\ntool is included. This\nprevents a problem.");
    assert_eq!(out, vec![
        "If foo, that tool is included.",
        "This prevents a problem.",
    ]);
}

#[test]
fn runs_of_whitespace_collapse_to_one_space() {
    let out = split("First  sentence.    Second\n\nsentence.");
    assert_eq!(out, vec!["First sentence.", "Second sentence."]);
}

#[test]
fn inline_code_internal_whitespace_is_preserved_through_normalisation() {
    // The two spaces inside the backticks survive the whitespace collapse
    // because the inline code span is placeholdered first.
    let out = split("Use `foo  bar` for this.");
    assert_eq!(out, vec!["Use `foo  bar` for this."]);
}

#[test]
fn quoted_period_end_of_sentence() {
    // "done." followed by uppercase Start is a real sentence boundary.
    assert_eq!(split(r#"End of quote: "done." Start again."#), vec![
        r#"End of quote: "done.""#,
        "Start again."
    ]);
}
