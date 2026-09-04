use super::*;

#[test]
fn renders_a_listing_as_one_bulleted_block() {
    let files = vec![
        RepoFile {
            path: "README.md".to_owned(),
            size: Some(1024),
        },
        RepoFile {
            path: "src/main.rs".to_owned(),
            size: None,
        },
    ];

    assert_eq!(format_listing(&files), indoc::indoc! {"
        <results>
        - README.md (1024 bytes)
        - src/main.rs
        </results>"});
}

#[test]
fn reports_an_empty_listing_as_a_sentence() {
    assert_eq!(format_listing(&[]), "No files found.");
}
