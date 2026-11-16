use pretty_assertions::assert_eq;

use super::*;

#[test]
fn bold_code_and_links_become_styles() {
    assert_eq!(
        styled("Filed **T-1e932pb**, see `docs/ticket` and [the board](https://jp.dev/board)."),
        Styled {
            text: "Filed T-1e932pb, see docs/ticket and the board (https://jp.dev/board)."
                .to_owned(),
            styles: vec!["6:9:BOLD".to_owned(), "21:11:MONOSPACE".to_owned()],
        }
    );
}

#[test]
fn headings_are_bold_and_bullets_are_dots() {
    assert_eq!(
        styled("## In progress\n\n- T-1e932pb\n  * nested"),
        Styled {
            text: "In progress\n\n• T-1e932pb\n  • nested".to_owned(),
            styles: vec!["0:11:BOLD".to_owned()],
        }
    );
}

#[test]
fn a_fenced_block_is_monospace_without_its_fences() {
    assert_eq!(styled("Run:\n\n```sh\njust check\n```"), Styled {
        text: "Run:\n\njust check".to_owned(),
        styles: vec!["6:10:MONOSPACE".to_owned()],
    });
}

/// Surrogate pairs outside the BMP count as two UTF-16 code units.
#[test]
fn ranges_count_utf16_code_units() {
    assert_eq!(styled("🎉 **done**"), Styled {
        text: "🎉 done".to_owned(),
        styles: vec!["3:4:BOLD".to_owned()],
    });
}

#[test]
fn unclosed_markup_is_left_as_written() {
    assert_eq!(styled("a ** b `c").text, "a ** b `c");
    assert!(styled("a ** b `c").styles.is_empty());
}

#[test]
fn a_short_answer_is_one_chunk() {
    assert_eq!(chunks("One.\n\nTwo.", 100), vec!["One.\n\nTwo.".to_owned()]);
}

#[test]
fn a_long_answer_splits_at_paragraphs() {
    assert_eq!(chunks("aaaa\n\nbbbb\n\ncccc", 10), vec![
        "aaaa\n\nbbbb".to_owned(),
        "cccc".to_owned()
    ]);
}

/// Fences aren't sent, so they don't count toward the budget.
#[test]
fn a_code_block_stays_together() {
    assert_eq!(chunks("intro\n\n```\na\n\nb\n```", 6), vec![
        "intro".to_owned(),
        "```\na\n\nb\n```".to_owned()
    ]);
}

#[test]
fn a_paragraph_too_long_for_one_message_splits_at_lines_then_characters() {
    assert_eq!(chunks("aaa\nbbb\ncccccccc", 5), vec![
        "aaa".to_owned(),
        "bbb".to_owned(),
        "ccccc".to_owned(),
        "ccc".to_owned()
    ]);
}

/// Signal limits a message's UTF-8 size, so a short text in a multi-byte script
/// still splits.
#[test]
fn the_budget_counts_utf8_bytes_not_characters() {
    // Five characters, ten bytes.
    assert_eq!(chunks("ééééé", 6), vec![
        "ééé".to_owned(),
        "éé".to_owned()
    ]);
}

/// A `- ` bullet styles to the three-byte `•`, so a list is measured after
/// styling: here seven bytes of Markdown become eleven.
#[test]
fn the_budget_counts_the_styled_text() {
    let parts = chunks("- a\n- b", 8);

    assert_eq!(parts, vec!["- a".to_owned(), "- b".to_owned()]);
    assert_eq!(styled(&parts[0]).text, "• a");
}
