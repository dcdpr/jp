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

/// Sum of the display widths of each grapheme cluster in `s`.
fn cluster_width_sum(s: &str) -> usize {
    s.graphemes(true).map(display_width).sum()
}

#[test]
fn cluster_width_sum_is_an_upper_bound_on_display_width() {
    // The invariant the cheap path in `truncate_to_width` rests on: summing
    // cluster widths can over-count (ligatures) but never under-count, so a
    // prefix that fits under the sum certainly fits. A widening interaction
    // sits inside one cluster (U+2018 plus U+FE01 renders in two columns), where
    // measuring the cluster whole already accounts for it.
    for s in [
        LAM_ALEF,
        TIFINAGH_LIGATURE,
        ZWJ_EMOJI,
        "\u{2018}\u{FE01}",
        "\u{2018}\u{FE00}",
        "plain ascii",
        "\u{65E5}\u{672C}\u{8A9E}",
    ] {
        assert!(
            display_width(s) <= cluster_width_sum(s),
            "cluster sum under-counts {s:?}: {} > {}",
            display_width(s),
            cluster_width_sum(s)
        );
    }
}

#[test]
fn measurement_count_follows_the_budget_not_the_input_size() {
    // Whole-string measurement is the expensive step, so its count has to depend
    // on the budget rather than on how long the title is. Conversation titles are
    // user-editable and uncapped, and `conversation ls` truncates one per row.
    // Measuring every candidate prefix took 76s for the 100k input below, against
    // 7ms once the count is bounded.
    let (_, short) = longest_fitting_prefix(&"x".repeat(500), 9);
    let (_, long) = longest_fitting_prefix(&"x".repeat(100_000), 9);

    assert_eq!(short, long, "measurement count grew with the input");
    assert!(
        long <= MAX_LIGATURE_PROBES,
        "{long} measurements for a 9-column cut"
    );
}

#[test]
fn measurement_count_stays_bounded_across_ligatures() {
    // Ligatures are why the exact measurement exists at all, so the bound has to
    // hold for an input made entirely of them, where every probe finds a prefix
    // that fits and starts a fresh run.
    let (_, short) = longest_fitting_prefix(&LAM_ALEF.repeat(250), 2);
    let (_, long) = longest_fitting_prefix(&LAM_ALEF.repeat(50_000), 2);

    assert_eq!(short, long, "measurement count grew with the input");
}

#[test]
fn truncate_to_width_cuts_long_input_the_same_way_as_short() {
    // The bounded measurement must not change where the cut lands: these are the
    // 100k-scale counterparts of the ASCII and Lam-Alef cases above.
    assert_eq!(
        truncate_to_width(&"x".repeat(100_000), 10),
        "xxxxxxxxx\u{2026}"
    );
    assert_eq!(
        truncate_to_width(&LAM_ALEF.repeat(50_000), 3),
        format!("{}\u{2026}", LAM_ALEF.repeat(2))
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

#[test]
fn prefix_end_for_width_cuts_where_the_budget_runs_out() {
    assert_eq!(prefix_end_for_width("hello world", 5), 5);
    assert_eq!(prefix_end_for_width("hello", 10), 5);
    assert_eq!(prefix_end_for_width("", 10), 0);
}

#[test]
fn prefix_end_for_width_reports_zero_when_nothing_fits() {
    // The caller has to notice this and force progress itself; a wide cluster
    // under a narrow budget is the case that would otherwise stall a split.
    assert_eq!(prefix_end_for_width("\u{65E5}", 1), 0);
}

#[test]
fn suffix_start_for_width_is_the_mirror_of_the_prefix() {
    assert_eq!(suffix_start_for_width("hello world", 5), 6);
    assert_eq!(suffix_start_for_width("hello", 10), 0);
    assert_eq!(suffix_start_for_width("", 10), 0);
}

#[test]
fn suffix_start_for_width_reports_the_end_when_nothing_fits() {
    let s = "\u{65E5}";
    assert_eq!(suffix_start_for_width(s, 1), s.len());
}

#[test]
fn suffix_start_for_width_counts_columns_not_bytes() {
    // Eight two-column emoji. A 5-column budget fits two of them, and the third
    // would overshoot. A byte-counting scan would have kept one.
    let s = "\u{1F389}".repeat(8);
    assert_eq!(suffix_start_for_width(&s, 5), s.len() - 8);
}

#[test]
fn suffix_start_for_width_spends_budget_per_cluster_not_per_scalar() {
    // The prefix scan's ZWJ case, run from the other end: four clusters at two
    // columns each fit an 8-column budget. Summing scalar widths would have
    // spent it twice as fast and kept two.
    let s = ZWJ_EMOJI.repeat(8);
    assert_eq!(suffix_start_for_width(&s, 8), s.len() - ZWJ_EMOJI.len() * 4);
}

#[test]
fn suffix_start_for_width_measures_suffixes_not_single_clusters() {
    // Prefix width is not monotonic and neither is suffix width. Four ligatures
    // at one column each fit a 4-column budget; summing cluster widths would
    // have stopped at two.
    let s = LAM_ALEF.repeat(4);
    assert_eq!(suffix_start_for_width(&s, 4), 0);
}

#[test]
fn suffix_measurement_count_follows_the_budget_not_the_input_size() {
    // Same bound as the prefix scan, for the same reason: the whole-string
    // measurement is the expensive step and grep windows arbitrarily long lines.
    let (_, short) = longest_fitting_suffix(&"x".repeat(500), 9);
    let (_, long) = longest_fitting_suffix(&"x".repeat(100_000), 9);

    assert_eq!(short, long, "measurement count grew with the input");
    assert!(
        long <= MAX_LIGATURE_PROBES,
        "{long} measurements for a 9-column cut"
    );
}

/// The substrings `wrap_ranges` selects, for readable assertions.
fn wrapped(s: &str, max_width: usize) -> Vec<&str> {
    wrap_ranges(s, max_width)
        .into_iter()
        .map(|range| &s[range])
        .collect()
}

#[test]
fn wrap_ranges_breaks_at_the_last_space_that_fits() {
    assert_eq!(wrapped("the quick brown fox", 10), [
        "the quick",
        "brown fox"
    ]);
}

#[test]
fn wrap_ranges_leaves_text_that_already_fits_alone() {
    let rows = wrap_ranges("short", 10);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0], 0..5);
}

#[test]
fn wrap_ranges_drops_only_the_whitespace_it_breaks_on() {
    // The space between the rows is consumed by the break, so neither row
    // carries it. Every other byte survives into exactly one row.
    let s = "alpha beta gamma";
    let rows = wrapped(s, 10);
    assert_eq!(rows, ["alpha beta", "gamma"]);
    assert_eq!(rows.concat().len(), s.len() - 1);
}

#[test]
fn wrap_ranges_hard_breaks_a_word_wider_than_the_row() {
    assert_eq!(wrapped("antidisestablishmentarianism", 10), [
        "antidisest",
        "ablishment",
        "arianism"
    ]);
}

#[test]
fn wrap_ranges_moves_an_over_wide_word_to_its_own_rows() {
    // The break lands at the space rather than mid-word, and the long word then
    // hard-breaks on the rows after it.
    assert_eq!(wrapped("hi antidisestablishmentarianism", 10), [
        "hi",
        "antidisest",
        "ablishment",
        "arianism"
    ]);
}

#[test]
fn wrap_ranges_counts_columns_not_bytes() {
    // Three-column-wide CJK: a 6-column row fits three characters, not six.
    assert_eq!(wrapped(&"\u{65E5}".repeat(6), 6), [
        "\u{65E5}\u{65E5}\u{65E5}",
        "\u{65E5}\u{65E5}\u{65E5}"
    ]);
}

#[test]
fn wrap_ranges_advances_past_a_cluster_wider_than_the_row() {
    // A row too narrow for even one cluster must still consume it, or the split
    // never terminates. One cluster per row is the degenerate but finite answer.
    assert_eq!(wrapped(&"\u{65E5}".repeat(3), 1), [
        "\u{65E5}", "\u{65E5}", "\u{65E5}"
    ]);
}

#[test]
fn wrap_ranges_never_returns_an_empty_row_set() {
    // Callers render one row per range, so an empty set would drop the line
    // entirely rather than printing it blank.
    let empty = wrap_ranges("", 10);
    assert_eq!(empty.len(), 1);
    assert_eq!(empty[0], 0..0);

    // A zero budget can't be met, so the text is handed back unsplit rather
    // than broken into one cluster per row.
    let unsplittable = wrap_ranges("abc", 0);
    assert_eq!(unsplittable.len(), 1);
    assert_eq!(unsplittable[0], 0..3);
}

#[test]
fn wrap_ranges_keeps_leading_whitespace_on_the_first_row() {
    // Breaking at offset 0 would emit an empty row and make no progress, so the
    // indent stays put and the row breaks at the next space instead.
    assert_eq!(wrapped("    indented text here", 10), [
        "    indent",
        "ed text",
        "here"
    ]);
}

#[test]
fn wrap_ranges_collapses_a_run_of_whitespace_at_a_break() {
    assert_eq!(wrapped("alpha beta    gamma delta", 12), [
        "alpha beta",
        "gamma delta"
    ]);
}

#[test]
fn wrap_ranges_measures_a_bounded_prefix_rather_than_the_whole_suffix() {
    // Measuring the remaining suffix once per row is quadratic, and grep wraps
    // single-line tool results that run to megabytes. This input takes minutes
    // that way and milliseconds when each row measures only its own width.
    let s = "lorem ipsum dolor sit amet ".repeat(20_000);
    let rows = wrap_ranges(&s, 40);

    assert_eq!(rows.len(), 14_286, "rows for {} bytes", s.len());
    assert_eq!(rows.last().map(|row| row.end), Some(s.len()));
}

#[test]
fn wrap_ranges_rows_all_fit_the_budget() {
    // The property the whole function exists for, checked across the shapes the
    // cases above pin individually.
    for s in [
        "the quick brown fox jumps over the lazy dog",
        "antidisestablishmentarianism and more",
        &"\u{65E5}\u{672C}\u{8A9E} ".repeat(10),
        &ZWJ_EMOJI.repeat(20),
        &LAM_ALEF.repeat(40),
    ] {
        for max_width in [4, 7, 10, 23] {
            for row in wrapped(s, max_width) {
                assert!(
                    display_width(row) <= max_width || row.graphemes(true).count() == 1,
                    "row {row:?} exceeds {max_width} columns"
                );
            }
        }
    }
}
