use super::*;

fn setup_test_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE Z_5TAGS (Z_5NOTES INTEGER, Z_13TAGS INTEGER);")
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
    conn.execute_batch("CREATE TABLE Z_7TAGS (Z_7NOTES INTEGER, Z_15TAGS INTEGER);")
        .unwrap();

    let meta = discover(&conn).unwrap();
    assert_eq!(meta.junction_table, "Z_7TAGS");
    assert_eq!(meta.notes_column, "Z_7NOTES");
    assert_eq!(meta.tags_column, "Z_15TAGS");
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
