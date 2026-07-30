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
fn truncate_to_width_keeps_zero_width_input_at_a_zero_budget() {
    // A zero-column string already fits a zero-column budget, so it survives
    // untouched. Only input that doesn't fit is dropped for the ellipsis.
    assert_eq!(truncate_to_width("\u{200D}", 0), "\u{200D}");
    assert_eq!(truncate_to_width("a", 0), "");
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

/// Arabic Lam followed by Alef: two grapheme clusters that ligate into a single
/// column.
const LAM_ALEF: &str = "\u{0644}\u{0627}";

#[test]
fn lam_alef_ligates_narrower_than_its_clusters() {
    // The premise the truncator depends on. If this ever changes upstream, the
    // test below stops testing what it claims to.
    assert_eq!(display_width(LAM_ALEF), 1);
    let cluster_sum: usize = LAM_ALEF.graphemes(true).map(display_width).sum();
    assert_eq!(cluster_sum, 2);
}

#[test]
fn truncate_to_width_measures_prefixes_not_single_clusters() {
    // Four ligatures at one column each. A 3-column budget leaves 2 for text,
    // which is exactly two ligatures. Summing cluster widths would have spent
    // the whole budget on the first ligature and kept one.
    let s = LAM_ALEF.repeat(4);
    assert_eq!(display_width(&s), 4);
    assert_eq!(
        truncate_to_width(&s, 3),
        format!("{}\u{2026}", LAM_ALEF.repeat(2))
    );
}

/// Tifinagh consonant, consonant joiner, consonant: a ligature that renders in
/// one column.
const TIFINAGH_LIGATURE: &str = "\u{2D4F}\u{2D7F}\u{2D3E}";

#[test]
fn tifinagh_ligature_narrows_once_completed() {
    // Prefix width is not monotonic, which is the premise the full boundary
    // scan depends on: the joiner extends the first consonant's cluster and
    // costs a column there, and completing the ligature drops the total to one.
    assert_eq!(TIFINAGH_LIGATURE.graphemes(true).collect::<Vec<_>>(), [
        "\u{2D4F}\u{2D7F}",
        "\u{2D3E}"
    ]);
    assert_eq!(display_width("\u{2D4F}\u{2D7F}"), 2);
    assert_eq!(display_width(TIFINAGH_LIGATURE), 1);
}

#[test]
fn truncate_to_width_keeps_the_longest_fitting_prefix() {
    // The ligature renders in one column, so it plus the ellipsis fills the
    // 2-column budget exactly. Stopping at the first prefix that overshoots
    // would have rejected the two-column consonant-plus-joiner prefix and
    // returned the ellipsis alone.
    let s = format!("{TIFINAGH_LIGATURE}xx");
    assert_eq!(display_width(&s), 3);
    assert_eq!(
        truncate_to_width(&s, 2),
        format!("{TIFINAGH_LIGATURE}\u{2026}")
    );
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
