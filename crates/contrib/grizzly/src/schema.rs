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
#[path = "schema_tests.rs"]
mod tests;
