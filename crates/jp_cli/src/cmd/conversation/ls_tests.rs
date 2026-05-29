use super::*;

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
fn fit_title_is_full_within_width() {
    assert_eq!(fit_title(40, 80, 20), TitleFit::Full);
    assert_eq!(fit_title(80, 80, 20), TitleFit::Full);
}

#[test]
fn fit_title_shaves_title_by_the_overflow() {
    // 20 columns over budget, title column is 30 wide -> shave to 10.
    assert_eq!(fit_title(100, 80, 30), TitleFit::Truncate(10));
}

#[test]
fn fit_title_floor_is_the_header_width() {
    // Shaving lands exactly on the header width: keep a minimal column.
    assert_eq!(fit_title(100, 80, 25), TitleFit::Truncate(5));
}

#[test]
fn fit_title_drops_when_column_would_be_unusable() {
    // Shaving would push the column below the header width -> drop it.
    assert_eq!(fit_title(100, 80, 24), TitleFit::Drop);
}

#[test]
fn display_width_ignores_color_codes() {
    assert_eq!(display_width("\x1b[1;33mhi\x1b[0m"), 2);
}

#[test]
fn display_width_ignores_osc8_hyperlinks() {
    // The hyperlinked ID column must measure as its visible text only.
    // If the URL bytes were counted, the fit math would under-shave and the
    // table would still overflow.
    let linked = hyperlink("jp://show-metadata/abc", "abc");
    assert_eq!(display_width(&linked), 3);
}
