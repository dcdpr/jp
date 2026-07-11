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
fn display_width_ignores_osc8_hyperlinks() {
    // The hyperlinked ID column must measure as its visible text only.
    // If the URL bytes were counted, the fit math would under-shave and the
    // table would still overflow.
    let linked = hyperlink("jp://show-metadata/abc", "abc");
    assert_eq!(display_width(&linked), 3);
}

#[test]
fn local_cell_marks_external_distinctly() {
    assert_eq!(strip_str(local_cell(false, false)), "N");
    assert_eq!(strip_str(local_cell(true, false)), "Y");
    assert_eq!(strip_str(local_cell(false, true)), "ext");
}

#[test]
fn payload_keys_are_the_stable_contract() {
    let created = DateTime::<Utc>::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
    let last_event = created + chrono::Duration::hours(1);
    let details = Details {
        id: ConversationId::try_from(created).unwrap(),
        active: true,
        pinned_at: None,
        archived_at: None,
        title: Some("My title".into()),
        messages: 4,
        last_event_at: Some(last_event),
        expires_at: None,
        local: true,
        external: false,
    };

    let json = payload(std::slice::from_ref(&details));
    let items = json.as_array().expect("payload is a JSON array");
    assert_eq!(items.len(), 1);

    // Fixed key set: display columns (headers, markers, truncation) must not
    // leak into the machine payload. A key rename here is a breaking change.
    let item = &items[0];
    let mut keys: Vec<_> = item
        .as_object()
        .expect("one object per conversation")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, vec![
        "active",
        "archived_at",
        "created_at",
        "events",
        "expires_at",
        "external",
        "id",
        "last_event_at",
        "local",
        "pinned_at",
        "title",
    ]);

    assert_eq!(item["id"], serde_json::json!(details.id.to_string()));
    assert_eq!(item["title"], serde_json::json!("My title"));
    assert_eq!(item["active"], serde_json::json!(true));
    assert_eq!(item["events"], serde_json::json!(4));
    // Absent optionals serialize as null, not as omitted keys.
    assert_eq!(item["pinned_at"], serde_json::Value::Null);
    assert_eq!(item["expires_at"], serde_json::Value::Null);
    // Timestamps are RFC 3339 UTC strings.
    assert!(
        item["last_event_at"]
            .as_str()
            .is_some_and(|v| v.contains('T') && v.ends_with('Z')),
        "RFC 3339 UTC timestamp, got: {}",
        item["last_event_at"]
    );
}
