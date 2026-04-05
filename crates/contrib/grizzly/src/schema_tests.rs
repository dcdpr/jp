use super::*;

/// Minimal ZSFNOTE + ZSFNOTETAG tables so the validation query can find links.
fn setup_backing_tables(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE ZSFNOTE (Z_PK INTEGER PRIMARY KEY, ZUNIQUEIDENTIFIER TEXT, ZTITLE TEXT,
         ZTEXT TEXT, ZMODIFICATIONDATE REAL, ZCREATIONDATE REAL, ZTRASHED INTEGER,
         ZARCHIVED INTEGER);
         INSERT INTO ZSFNOTE (Z_PK, ZUNIQUEIDENTIFIER, ZTITLE, ZTEXT, ZMODIFICATIONDATE,
         ZCREATIONDATE, ZTRASHED, ZARCHIVED) VALUES (1, 'n1', 'Note', 'body', 0, 0, 0, 0);

         CREATE TABLE ZSFNOTETAG (Z_PK INTEGER PRIMARY KEY, ZTITLE TEXT);
         INSERT INTO ZSFNOTETAG (Z_PK, ZTITLE) VALUES (10, 'sometag');",
    )
    .unwrap();
}

fn setup_test_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    setup_backing_tables(&conn);
    conn.execute_batch(
        "CREATE TABLE Z_5TAGS (Z_5NOTES INTEGER, Z_13TAGS INTEGER);
         INSERT INTO Z_5TAGS VALUES (1, 10);",
    )
    .unwrap();
    conn
}

#[test]
fn discovers_junction_table() {
    let conn = setup_test_db();
    let meta = discover(&conn).unwrap();
    assert_eq!(meta.junction_table, "Z_5TAGS");
    assert_eq!(meta.notes_column, "Z_5NOTES");
    assert_eq!(meta.tags_column, "Z_13TAGS");
}

#[test]
fn discovers_different_numbers() {
    let conn = Connection::open_in_memory().unwrap();
    setup_backing_tables(&conn);
    conn.execute_batch(
        "CREATE TABLE Z_7TAGS (Z_7NOTES INTEGER, Z_15TAGS INTEGER);
         INSERT INTO Z_7TAGS VALUES (1, 10);",
    )
    .unwrap();

    let meta = discover(&conn).unwrap();
    assert_eq!(meta.junction_table, "Z_7TAGS");
    assert_eq!(meta.notes_column, "Z_7NOTES");
    assert_eq!(meta.tags_column, "Z_15TAGS");
}

#[test]
fn skips_candidate_with_no_valid_links() {
    let conn = Connection::open_in_memory().unwrap();
    setup_backing_tables(&conn);

    // First candidate has the right columns but no valid links (empty table).
    conn.execute_batch("CREATE TABLE Z_3TAGS (Z_3NOTES INTEGER, Z_3TAGS INTEGER);")
        .unwrap();

    // Second candidate has valid links.
    conn.execute_batch(
        "CREATE TABLE Z_5TAGS (Z_5NOTES INTEGER, Z_13TAGS INTEGER);
         INSERT INTO Z_5TAGS VALUES (1, 10);",
    )
    .unwrap();

    let meta = discover(&conn).unwrap();
    // Z_3TAGS sorts first but is empty — should pick Z_5TAGS.
    assert_eq!(meta.junction_table, "Z_5TAGS");
}

#[test]
fn skips_candidate_with_wrong_fk_references() {
    let conn = Connection::open_in_memory().unwrap();
    setup_backing_tables(&conn);

    // This table has rows, but the PKs don't match ZSFNOTE or ZSFNOTETAG.
    conn.execute_batch(
        "CREATE TABLE Z_2TAGS (Z_2NOTES INTEGER, Z_2TAGS INTEGER);
         INSERT INTO Z_2TAGS VALUES (999, 999);",
    )
    .unwrap();

    // This one actually links to real rows.
    conn.execute_batch(
        "CREATE TABLE Z_5TAGS (Z_5NOTES INTEGER, Z_13TAGS INTEGER);
         INSERT INTO Z_5TAGS VALUES (1, 10);",
    )
    .unwrap();

    let meta = discover(&conn).unwrap();
    assert_eq!(meta.junction_table, "Z_5TAGS");
}

#[test]
fn fails_when_no_candidates() {
    let conn = Connection::open_in_memory().unwrap();
    setup_backing_tables(&conn);
    assert!(discover(&conn).is_err());
}

#[test]
fn fails_when_all_candidates_invalid() {
    let conn = Connection::open_in_memory().unwrap();
    setup_backing_tables(&conn);
    // Table exists but is empty.
    conn.execute_batch("CREATE TABLE Z_5TAGS (Z_5NOTES INTEGER, Z_13TAGS INTEGER);")
        .unwrap();

    assert!(discover(&conn).is_err());
}

#[test]
fn cte_contains_discovered_names() {
    let meta = SchemaMetadata {
        junction_table: "Z_5TAGS".into(),
        notes_column: "Z_5NOTES".into(),
        tags_column: "Z_13TAGS".into(),
    };
    let cte = normalizing_cte(&meta);
    assert!(cte.contains("FROM Z_5TAGS nt"));
    assert!(cte.contains("nt.Z_5NOTES"));
    assert!(cte.contains("nt.Z_13TAGS"));
}
