use rusqlite::Connection;

use super::*;

fn setup() -> (Connection, String) {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::tests::setup_test_schema(&conn);
    let meta = crate::schema::discover(&conn).unwrap();
    let cte = crate::schema::normalizing_cte(&meta);
    (conn, cte)
}

#[test]
fn query_single_term() {
    assert_eq!(build_query(&["hello".into()]), r#""hello""#);
}

#[test]
fn query_multiple_terms() {
    assert_eq!(
        build_query(&["foo".into(), "bar".into()]),
        r#""foo" AND "bar""#,
    );
}

#[test]
fn query_escapes_quotes() {
    assert_eq!(build_query(&[r#"say "hi""#.into()]), r#""say ""hi""""#);
}

#[test]
fn query_skips_blank_terms() {
    assert_eq!(
        build_query(&["foo".into(), "  ".into(), "bar".into()]),
        r#""foo" AND "bar""#,
    );
}

#[test]
fn word_search_finds_content() {
    let (conn, cte) = setup();
    setup_word_table(&conn, &cte).unwrap();

    let results = search_words(&conn, &["productivity".into()], 10).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].note_id, "note-1");
}

#[test]
fn word_search_phrase() {
    let (conn, cte) = setup();
    setup_word_table(&conn, &cte).unwrap();

    let results = search_words(&conn, &["David Allen".into()], 10).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].note_id, "note-1");
}

#[test]
fn word_search_no_match() {
    let (conn, cte) = setup();
    setup_word_table(&conn, &cte).unwrap();

    let results = search_words(&conn, &["xyznonexistent".into()], 10).unwrap();
    assert!(results.is_empty());
}

#[test]
fn word_search_excludes_trashed() {
    let (conn, cte) = setup();
    setup_word_table(&conn, &cte).unwrap();

    let results = search_words(&conn, &["trashed".into()], 10).unwrap();
    assert!(results.is_empty());
}

#[test]
fn word_search_multiple_queries_and() {
    let (conn, cte) = setup();
    setup_word_table(&conn, &cte).unwrap();

    let results = search_words(&conn, &["productivity".into(), "David".into()], 10).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].note_id, "note-1");
}

#[test]
fn trigram_substring_match() {
    let (conn, cte) = setup();
    setup_trigram_table(&conn, &cte).unwrap();

    // "producti" is a substring of "productivity" in note-1
    let results = search_trigrams(&conn, &["producti".into()], 10).unwrap();
    assert!(!results.is_empty());
    assert_eq!(results[0].note_id, "note-1");
}

#[test]
fn trigram_no_match_for_short_queries() {
    let (conn, cte) = setup();
    setup_trigram_table(&conn, &cte).unwrap();

    // Trigram tokenizer needs >= 3 characters to produce useful matches
    let results = search_trigrams(&conn, &["pr".into()], 10).unwrap();
    assert!(results.is_empty());
}
