use strip_ansi_escapes::strip_str;

use super::*;

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
fn sort_marker_defaults_to_activity() {
    let m = sort_marker(None, false, false).expect("active list marks a column");
    assert_eq!(m.column, SortColumn::Activity);
    assert!(!m.descending);
}

#[test]
fn sort_marker_archived_default_has_no_column() {
    // Archived listing defaults to archive-time order, which has no column.
    assert_eq!(sort_marker(None, true, false), None);
}

#[test]
fn sort_marker_created_marks_the_id_column() {
    // `created` orders by the ID timestamp, so the ID column carries the marker.
    let m = sort_marker(Some(Sort::Created), false, false).unwrap();
    assert_eq!(m.column, SortColumn::Id);
}

#[test]
fn sort_marker_follows_explicit_field_and_direction() {
    let m = sort_marker(Some(Sort::Messages), false, true).unwrap();
    assert_eq!(m.column, SortColumn::Messages);
    assert!(m.descending);
}

#[test]
fn header_marks_only_the_sorted_column() {
    let columns = Columns {
        expires_at: false,
        local: false,
        title: true,
    };
    let rendered = list(
        build_header_row(columns, sort_marker(None, false, false)),
        vec![],
        false,
    );
    assert!(rendered.contains("Activity ↑"), "got:\n{rendered}");
    assert!(!rendered.contains("ID ↑"), "got:\n{rendered}");
}

#[test]
fn local_cell_marks_external_distinctly() {
    assert_eq!(strip_str(local_cell(false, false)), "N");
    assert_eq!(strip_str(local_cell(true, false)), "Y");
    assert_eq!(strip_str(local_cell(false, true)), "ext");
}
