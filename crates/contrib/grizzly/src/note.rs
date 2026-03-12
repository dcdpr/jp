use rusqlite::{Connection, OptionalExtension as _, params};

use crate::Result;

/// A note from the Bear database.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub id: String,
    pub title: String,
    pub content: Option<String>,
    pub tags: Vec<String>,
    pub updated_at: Option<String>,
}

impl Note {
    /// Fetch a single note by its unique identifier.
    pub fn get_by_id(conn: &Connection, cte: &str, id: &str) -> Result<Option<Self>> {
        let sql = format!(
            "{cte}
             SELECT n.id, n.title, n.content, n.updated_at
             FROM notes n
             WHERE n.id = ?1 AND n.is_trashed = 0"
        );

        let row = conn
            .query_row(&sql, params![id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .optional()?;

        let Some((note_id, title, content, updated_at)) = row else {
            return Ok(None);
        };

        let tags = Self::fetch_tags(conn, cte, &note_id)?;

        Ok(Some(Note {
            id: note_id,
            title,
            content,
            tags,
            updated_at,
        }))
    }

    /// Format a note as pseudo-XML for LLM consumption.
    #[must_use]
    pub fn to_xml(&self) -> String {
        let tags_str = self.tags.join(" ");
        let updated = self.updated_at.as_deref().unwrap_or("unknown");
        let content = self.content.as_deref().unwrap_or("");

        format!(
            "<note id=\"{}\" tags=\"{tags_str}\" updated-at=\"{updated}\">\n{content}\n</note>",
            self.id,
        )
    }

    /// Format a note, showing only the specified lines.
    #[must_use]
    pub fn to_xml_with_lines(&self, line_specs: &[LineSpec]) -> String {
        let tags_str = self.tags.join(" ");
        let updated = self.updated_at.as_deref().unwrap_or("unknown");
        let content = self.content.as_deref().unwrap_or("");
        let lines: Vec<&str> = content.lines().collect();

        let mut selected = String::new();
        for spec in line_specs {
            let (start, end) = match spec {
                LineSpec::Single(n) => (*n, *n),
                LineSpec::Range(s, e) => (*s, *e),
            };
            let start = start.saturating_sub(1); // 1-indexed to 0-indexed
            let end = end.min(lines.len());
            for idx in start..end {
                if let Some(line) = lines.get(idx) {
                    selected.push_str(&format!("{:03}: {line}\n", idx + 1));
                }
            }
        }

        format!(
            "<note id=\"{}\" tags=\"{tags_str}\" updated-at=\"{updated}\">\n{selected}</note>",
            self.id,
        )
    }

    fn fetch_tags(conn: &Connection, cte: &str, note_id: &str) -> Result<Vec<String>> {
        let sql = format!(
            "{cte}
             SELECT t.name
             FROM tags t
             JOIN note_tags nt ON t.id = nt.tag_id
             WHERE nt.note_id = ?1
             ORDER BY t.name"
        );

        let mut stmt = conn.prepare(&sql)?;
        let tags = stmt
            .query_map(params![note_id], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(tags)
    }
}

/// A line range specification for `note_get`.
#[derive(Debug, Clone, PartialEq)]
pub enum LineSpec {
    Single(usize),
    Range(usize, usize),
}

impl LineSpec {
    /// Parse a line spec from a JSON value.
    /// Accepts integers or strings like "10:20", "10-20", "10..20".
    pub fn parse(value: &serde_json::Value) -> Option<Self> {
        if let Some(n) = value.as_u64() {
            return usize::try_from(n).ok().map(LineSpec::Single);
        }

        let s = value.as_str()?;
        for sep in [":", "-", ".."] {
            if let Some((a, b)) = s.split_once(sep) {
                let start = a.trim().parse().ok()?;
                let end = b.trim().parse().ok()?;
                return Some(LineSpec::Range(start, end));
            }
        }

        s.trim().parse().ok().map(LineSpec::Single)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BearDb;

    #[test]
    fn get_note_by_id() {
        let db = BearDb::in_memory().unwrap();
        let notes = db.get_notes(&["note-1"]).unwrap();

        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].title, "Getting Things Done");
        assert_eq!(notes[0].tags, vec!["productivity"]);
    }

    #[test]
    fn trashed_notes_excluded() {
        let db = BearDb::in_memory().unwrap();
        let notes = db.get_notes(&["note-4"]).unwrap();
        assert!(notes.is_empty());
    }

    #[test]
    fn missing_note_returns_empty() {
        let db = BearDb::in_memory().unwrap();
        let notes = db.get_notes(&["nonexistent"]).unwrap();
        assert!(notes.is_empty());
    }

    #[test]
    fn note_xml_format() {
        let note = Note {
            id: "abc-123".into(),
            title: "Test".into(),
            content: Some("Line 1\nLine 2".into()),
            tags: vec!["tag1".into(), "tag2".into()],
            updated_at: Some("2024-01-01 00:00:00".into()),
        };

        let xml = note.to_xml();
        assert!(xml.contains(r#"id="abc-123""#));
        assert!(xml.contains(r#"tags="tag1 tag2""#));
        assert!(xml.contains("Line 1\nLine 2"));
    }

    #[test]
    fn parse_line_specs() {
        assert_eq!(
            LineSpec::parse(&serde_json::json!(10)),
            Some(LineSpec::Single(10))
        );
        assert_eq!(
            LineSpec::parse(&serde_json::json!("10:20")),
            Some(LineSpec::Range(10, 20))
        );
        assert_eq!(
            LineSpec::parse(&serde_json::json!("10-20")),
            Some(LineSpec::Range(10, 20))
        );
        assert_eq!(
            LineSpec::parse(&serde_json::json!("10..20")),
            Some(LineSpec::Range(10, 20))
        );
        assert_eq!(LineSpec::parse(&serde_json::json!("garbage")), None);
    }
}
