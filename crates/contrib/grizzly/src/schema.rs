use rusqlite::Connection;

use crate::Error;

/// Metadata discovered from Bear's Core Data SQLite schema.
///
/// Bear uses Apple's Core Data framework which creates numbered junction tables
/// (e.g. `Z_5TAGS`) with numbered columns (e.g. `Z_5NOTES`, `Z_13TAGS`). These
/// numbers can change across Bear versions, so we discover them at runtime.
#[derive(Debug, Clone)]
pub struct SchemaMetadata {
    pub junction_table: String,
    pub notes_column: String,
    pub tags_column: String,
}

/// Discover the junction table and column names from the Bear database.
pub fn discover(conn: &Connection) -> Result<SchemaMetadata, Error> {
    // Find the junction table matching Z_<number>TAGS
    let junction_table = conn
        .query_row(
            "SELECT name FROM sqlite_master
             WHERE type = 'table' AND name GLOB 'Z_[0-9]*TAGS'
             LIMIT 1",
            [],
            |row| row.get(0),
        )
        .map_err(|_| Error::SchemaDiscovery)?;

    // Read column names from the junction table
    let columns = conn
        .prepare(&format!("PRAGMA table_info({junction_table})"))?
        .query_map([], |row| row.get::<_, String>("name"))?
        .collect::<Result<Vec<_>, _>>()?;

    let notes_column = columns
        .iter()
        .find(|c| c.ends_with("NOTES"))
        .cloned()
        .ok_or(Error::SchemaDiscovery)?;

    let tags_column = columns
        .iter()
        .find(|c| c.ends_with("TAGS"))
        .cloned()
        .ok_or(Error::SchemaDiscovery)?;

    Ok(SchemaMetadata {
        junction_table,
        notes_column,
        tags_column,
    })
}

/// Generate a normalizing CTE that abstracts Bear's Core Data schema into clean
/// `notes`, `tags`, `note_tags` views.
#[must_use]
pub fn normalizing_cte(meta: &SchemaMetadata) -> String {
    format!(
        r"
WITH
  cd AS (SELECT unixepoch('2001-01-01') AS epoch),
  notes AS (
    SELECT
      n.ZUNIQUEIDENTIFIER AS id,
      n.Z_PK              AS pk,
      n.ZTITLE            AS title,
      n.ZTEXT             AS content,
      datetime(n.ZMODIFICATIONDATE + cd.epoch, 'unixepoch') AS updated_at,
      datetime(n.ZCREATIONDATE + cd.epoch, 'unixepoch')     AS created_at,
      n.ZTRASHED  AS is_trashed,
      n.ZARCHIVED AS is_archived
    FROM ZSFNOTE n, cd
  ),
  tags AS (
    SELECT
      t.Z_PK    AS id,
      t.ZTITLE  AS name
    FROM ZSFNOTETAG t
  ),
  note_tags AS (
    SELECT
      (SELECT n.ZUNIQUEIDENTIFIER FROM ZSFNOTE n WHERE n.Z_PK = nt.{notes_col}) AS note_id,
      nt.{tags_col} AS tag_id
    FROM {table} nt
  )
",
        notes_col = meta.notes_column,
        tags_col = meta.tags_column,
        table = meta.junction_table,
    )
}

#[cfg(test)]
mod tests {
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
}
