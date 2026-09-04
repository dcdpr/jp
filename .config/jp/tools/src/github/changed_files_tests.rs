use super::*;

fn changed(filename: &str, status: DiffEntryStatus, additions: u64, deletions: u64) -> ChangedFile {
    ChangedFile {
        filename: filename.to_owned(),
        status,
        additions,
        deletions,
        previous_filename: None,
    }
}

#[test]
fn renders_a_changed_file_with_its_stat() {
    let file = changed("src/foo.rs", DiffEntryStatus::Modified, 42, 10);

    assert_eq!(file.to_string(), "src/foo.rs (modified, +42,-10)");
}

#[test]
fn omits_a_zero_side_of_the_stat() {
    let added = changed("src/new.rs", DiffEntryStatus::Added, 12, 0);
    let removed = changed("src/old.rs", DiffEntryStatus::Removed, 0, 7);

    assert_eq!(added.to_string(), "src/new.rs (added, +12)");
    assert_eq!(removed.to_string(), "src/old.rs (removed, -7)");
}

#[test]
fn renders_a_rename_with_its_previous_name() {
    // A pure rename changes no lines, so the stat drops out entirely and the
    // old path is the only thing left to report.
    let file = ChangedFile {
        previous_filename: Some("src/old.rs".to_owned()),
        ..changed("src/new.rs", DiffEntryStatus::Renamed, 0, 0)
    };

    assert_eq!(file.to_string(), "src/new.rs (renamed from src/old.rs)");
}

#[test]
fn renders_changed_files_as_one_block() {
    let files = vec![
        changed("src/foo.rs", DiffEntryStatus::Modified, 42, 10),
        changed("src/bar.rs", DiffEntryStatus::Added, 5, 0),
    ];

    assert_eq!(format_changed_files(&files), indoc::indoc! {"
        <files>
        - src/foo.rs (modified, +42,-10)
        - src/bar.rs (added, +5)
        </files>"});
}

#[test]
fn reports_a_page_past_the_end_as_empty() {
    assert_eq!(format_changed_files(&[]), "No changed files on this page.");
}

#[test]
fn states_the_not_found_hint_once_below_the_block() {
    let files = vec!["src/foo.rs".to_owned(), "src/bar.rs".to_owned()];

    assert_eq!(format_not_found(&files), indoc::indoc! {"
        <not_found>
        - src/foo.rs
        - src/bar.rs
        </not_found>

        Not present on this page. Bump `page`, or call without `files` to enumerate the changed files and locate the right page."});
}
