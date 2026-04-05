use rusqlite::Connection;
use tracing::{debug, warn};

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
///
/// Core Data junction tables follow the pattern `Z_<number><RELATIONSHIP>`.
/// We look for tables whose columns reference both notes (column ending in
/// `NOTES`) and tags (column ending in `TAGS`). Multiple candidates may exist
/// (e.g. `Z_5TAGS`, `Z_5NOTETAGS`), so we validate each by checking it
/// actually joins `ZSFNOTE` and `ZSFNOTETAG` rows.
pub fn discover(conn: &Connection) -> Result<SchemaMetadata, Error> {
    // Find ALL candidate junction tables containing "TAGS" in the name.
    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_master
         WHERE type = 'table' AND name GLOB 'Z_[0-9]*TAGS'
         ORDER BY name",
    )?;

    let candidates: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;

    if candidates.is_empty() {
        warn!("no junction table candidates found matching Z_[0-9]*TAGS");
        return Err(Error::SchemaDiscovery);
    }

    debug!(?candidates, "junction table candidates");

    // Try each candidate — pick the first one that has valid columns AND
    // produces at least one note-tag link.
    for table in &candidates {
        let columns = conn
            .prepare(&format!("PRAGMA table_info({table})"))?
            .query_map([], |row| row.get::<_, String>("name"))?
            .collect::<Result<Vec<_>, _>>()?;

        let notes_col = columns.iter().find(|c| c.ends_with("NOTES"));
        let tags_col = columns.iter().find(|c| c.ends_with("TAGS"));

        let (Some(notes_col), Some(tags_col)) = (notes_col, tags_col) else {
            debug!(
                table,
                ?columns,
                "skipping candidate: missing NOTES or TAGS column"
            );
            continue;
        };

        // Validate: does this junction table actually link ZSFNOTE rows to
        // ZSFNOTETAG rows? A dead or unrelated table will have zero matches.
        let count: i64 = conn
            .query_row(
                &format!(
                    "SELECT COUNT(*) FROM {table} jt
                     WHERE EXISTS (SELECT 1 FROM ZSFNOTE n WHERE n.Z_PK = jt.{notes_col})
                       AND EXISTS (SELECT 1 FROM ZSFNOTETAG t WHERE t.Z_PK = jt.{tags_col})
                     LIMIT 1"
                ),
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        if count == 0 {
            debug!(
                table,
                notes_col, tags_col, "skipping candidate: no valid note-tag links"
            );
            continue;
        }

        debug!(table, notes_col, tags_col, count, "selected junction table");

        return Ok(SchemaMetadata {
            junction_table: table.clone(),
            notes_column: notes_col.clone(),
            tags_column: tags_col.clone(),
        });
    }

    warn!(
        ?candidates,
        "none of the junction table candidates produced valid note-tag links"
    );
    Err(Error::SchemaDiscovery)
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
