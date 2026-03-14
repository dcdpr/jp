use rusqlite::Connection;

use crate::Result;

/// A tag from the Bear database.
#[derive(Debug, Clone, PartialEq)]
pub struct Tag {
    pub name: String,
}

impl Tag {
    /// List all tags.
    pub fn list(conn: &Connection, cte: &str) -> Result<Vec<Self>> {
        let sql = format!("{cte} SELECT name FROM tags ORDER BY name");
        let mut stmt = conn.prepare(&sql)?;
        let tags = stmt
            .query_map([], |row| Ok(Tag { name: row.get(0)? }))?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(tags)
    }
}

#[cfg(test)]
#[path = "tag_tests.rs"]
mod tests;
