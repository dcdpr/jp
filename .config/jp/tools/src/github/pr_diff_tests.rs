use super::*;

fn changed(filename: &str, additions: u64, deletions: u64) -> ChangedFile {
    ChangedFile {
        filename: filename.to_owned(),
        status: DiffEntryStatus::Modified,
        additions,
        deletions,
        previous_filename: None,
    }
}

#[test]
fn renders_an_enumeration_as_a_header_and_one_block() {
    let files = vec![changed("src/foo.rs", 42, 10), changed("src/bar.rs", 5, 0)];

    assert_eq!(format_enumeration(42, 2, 120, &files), indoc::indoc! {"
        Pull #42, page 2 of 120 changed files (100 per page).

        <files>
        - src/foo.rs (modified, +42,-10)
        - src/bar.rs (modified, +5)
        </files>"});
}

#[test]
fn keeps_the_header_on_a_page_past_the_end() {
    // The total count is what tells the caller to stop walking pages, so it has
    // to survive the empty page that ends the walk.
    assert_eq!(format_enumeration(42, 9, 120, &[]), indoc::indoc! {"
        Pull #42, page 9 of 120 changed files (100 per page).

        No changed files on this page."});
}
