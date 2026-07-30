use super::*;
use crate::osc::hyperlink;

#[test]
fn truncate_to_width_keeps_strings_that_fit() {
    assert_eq!(truncate_to_width("hello", 10), "hello");
    assert_eq!(truncate_to_width("hello", 5), "hello");
}

#[test]
fn truncate_to_width_appends_ellipsis_when_cut() {
    // Four visible chars plus the ellipsis fill exactly five display columns.
    assert_eq!(truncate_to_width("hello world", 5), "hell…");
}

#[test]
fn truncate_to_width_minimal_budgets() {
    assert_eq!(truncate_to_width("hello", 1), "…");
    assert_eq!(truncate_to_width("hello", 0), "");
}

#[test]
fn truncate_to_width_counts_columns_not_bytes() {
    // Each emoji is four bytes wide but two columns, so a 10-column budget
    // fits four of them plus the ellipsis. A byte-counting truncator would
    // have fit two.
    let s = "🎉".repeat(8);
    assert_eq!(truncate_to_width(&s, 10), "🎉🎉🎉🎉…");
}

#[test]
fn truncate_to_width_never_splits_a_character() {
    let s = "🎉".repeat(8);
    for truncated in (0..=16).map(|max| truncate_to_width(&s, max)) {
        assert!(truncated.chars().all(|c| c == '🎉' || c == '…'));
    }
}

#[test]
fn display_width_ignores_color_codes() {
    assert_eq!(display_width("\x1b[1;33mhi\x1b[0m"), 2);
}

#[test]
fn display_width_ignores_osc8_hyperlinks() {
    // A hyperlinked string must measure as its visible text only. If the URL
    // bytes were counted, callers fitting output to a terminal would
    // over-shave.
    let linked = hyperlink("jp://show-metadata/abc", "abc");
    assert_eq!(display_width(&linked), 3);
}

/// Woman + ZWJ + laptop: one grapheme cluster whose scalars sum to four columns
/// but which renders in two.
const ZWJ_EMOJI: &str = "\u{1F469}\u{200D}\u{1F4BB}";

#[test]
fn zwj_sequence_renders_narrower_than_its_scalars() {
    // The premise the truncator depends on. If this ever changes upstream, the
    // two tests below stop testing what they claim to.
    assert_eq!(display_width(ZWJ_EMOJI), 2);
    let scalar_sum: usize = ZWJ_EMOJI
        .chars()
        .map(|c| display_width(&c.to_string()))
        .sum();
    assert_eq!(scalar_sum, 4);
}

#[test]
fn truncate_to_width_spends_budget_per_cluster_not_per_scalar() {
    // Eight clusters at two columns each. A 9-column budget leaves 8 for text,
    // which is exactly four clusters. Summing scalar widths would have spent
    // the budget twice as fast and kept only two.
    let s = ZWJ_EMOJI.repeat(8);
    assert_eq!(display_width(&s), 16);
    assert_eq!(
        truncate_to_width(&s, 9),
        format!("{}\u{2026}", ZWJ_EMOJI.repeat(4))
    );
}

#[test]
fn truncate_to_width_never_leaves_a_dangling_joiner() {
    // A zero-width joiner costs nothing, so a per-scalar budget would accept it
    // and then reject the scalar it joins, ending the line on U+200D.
    let s = ZWJ_EMOJI.repeat(2);
    for truncated in (0..=4).map(|max| truncate_to_width(&s, max)) {
        assert!(
            !truncated.trim_end_matches('\u{2026}').ends_with('\u{200D}'),
            "dangling joiner in {truncated:?}"
        );
    }
}

#[test]
fn display_width_counts_wide_characters_as_two_columns() {
    assert_eq!(display_width("日本語"), 6);
}

#[test]
fn max_line_width_takes_the_widest_line() {
    assert_eq!(max_line_width("ab\nabcd\nabc"), 4);
    assert_eq!(max_line_width(""), 0);
}
