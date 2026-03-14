//! FTS5 full-text search tables and queries.
//!
//! Creates temporary in-memory FTS5 virtual tables populated from Bear's
//! notes, then queries them with BM25 ranking (unicode61 tokenizer) or
//! substring matching (trigram tokenizer).

use rusqlite::Connection;

use crate::Result;

/// A result from an FTS5 query.
pub struct FtsResult {
    pub note_id: String,
    pub title: String,
    pub content: Option<String>,
    /// BM25 rank (negative; closer to 0 = better match).
    pub rank: f64,
}

/// Create and populate the word-based FTS5 table (unicode61 tokenizer).
///
/// Stored in the `temp` schema so it's dropped when the connection closes.
pub fn setup_word_table(conn: &Connection, cte: &str) -> Result<()> {
    conn.execute_batch(
        "CREATE VIRTUAL TABLE temp.fts_notes USING fts5(note_id UNINDEXED, title, content, \
         tokenize='unicode61')",
    )?;

    conn.execute_batch(&format!(
        "{cte} INSERT INTO temp.fts_notes(note_id, title, content) SELECT id, title, content FROM \
         notes WHERE is_trashed = 0"
    ))?;

    Ok(())
}

/// Create and populate the trigram FTS5 table for substring matching.
///
/// The trigram tokenizer indexes 3-character sequences, enabling substring
/// queries that the word-based tokenizer can't handle (e.g. partial words).
/// Query terms must be at least 3 characters.
pub fn setup_trigram_table(conn: &Connection, cte: &str) -> Result<()> {
    conn.execute_batch(
        "CREATE VIRTUAL TABLE temp.fts_trigram USING fts5(note_id UNINDEXED, title, content, \
         tokenize='trigram')",
    )?;

    conn.execute_batch(&format!(
        "{cte} INSERT INTO temp.fts_trigram(note_id, title, content) SELECT id, title, content \
         FROM notes WHERE is_trashed = 0"
    ))?;

    Ok(())
}

/// Search the word-based FTS5 table.
pub fn search_words(conn: &Connection, queries: &[String], limit: usize) -> Result<Vec<FtsResult>> {
    query_table(conn, "fts_notes", &build_query(queries), limit)
}

/// Search the trigram FTS5 table.
pub fn search_trigrams(
    conn: &Connection,
    queries: &[String],
    limit: usize,
) -> Result<Vec<FtsResult>> {
    query_table(conn, "fts_trigram", &build_query(queries), limit)
}

/// Build an FTS5 MATCH query from user search terms.
///
/// Each term is double-quoted (phrase match) and multiple terms are combined
/// with AND. Quoting prevents FTS5 operators in user input from being
/// interpreted.
fn build_query(queries: &[String]) -> String {
    queries
        .iter()
        .filter(|q| !q.trim().is_empty())
        .map(|q| {
            let escaped = q.replace('"', "\"\"");
            format!("\"{escaped}\"")
        })
        .collect::<Vec<_>>()
        .join(" AND ")
}

fn query_table(
    conn: &Connection,
    table: &str,
    fts_query: &str,
    limit: usize,
) -> Result<Vec<FtsResult>> {
    let sql = format!(
        "SELECT note_id, title, content, rank FROM temp.{table} WHERE {table} MATCH ?1 ORDER BY \
         rank LIMIT ?2"
    );

    #[allow(clippy::cast_possible_wrap)]
    let limit = limit as i64;
    let mut stmt = conn.prepare(&sql)?;
    let results = stmt
        .query_map(rusqlite::params![fts_query, limit], |row| {
            Ok(FtsResult {
                note_id: row.get(0)?,
                title: row.get(1)?,
                content: row.get(2)?,
                rank: row.get(3)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(results)
}

#[cfg(test)]
#[path = "fts_tests.rs"]
mod tests;
