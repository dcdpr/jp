use std::{collections::BTreeSet, error::Error, rc::Rc};

use async_trait::async_trait;
use camino::{Utf8Path, Utf8PathBuf};
use directories::BaseDirs;
use jp_attachment::{
    Attachment, BoxedHandler, HANDLERS, Handler, distributed_slice, linkme, percent_decode_str,
    percent_encode_str, typetag,
};
use jp_mcp::Client;
use rusqlite::{Connection, OptionalExtension as _, params, types::Value};
use serde::{Deserialize, Serialize};
use tracing::{debug, trace, warn};
use url::Url;

/// Path to the Bear Sqlite database.
///
/// See: <https://bear.app/faq/where-are-bears-notes-located/>
static DB_PATH: &str =
    "Library/Group Containers/9K33E3U3T4.net.shinyfrog.bear/Application Data/database.sqlite";

#[distributed_slice(HANDLERS)]
#[linkme(crate = linkme)]
static HANDLER: fn() -> BoxedHandler = handler;

fn handler() -> BoxedHandler {
    (Box::new(BearNotes::default()) as Box<dyn Handler>).into()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct BearNotes(BTreeSet<Query>);

impl BearNotes {
    fn query_to_uri(&self, query: &Query) -> Result<Url, Box<dyn std::error::Error + Send + Sync>> {
        let (host, path, query_pairs) = match query {
            Query::Get(path) => ("get", path, vec![]),
            Query::Search { query, tags } => (
                "search",
                query,
                tags.clone()
                    .iter()
                    .map(|t| ("tag".to_owned(), t.to_owned()))
                    .collect::<Vec<_>>(),
            ),
        };

        let query_pairs = query_pairs
            .iter()
            .map(|(k, v)| format!("{k}={}", percent_encode_str(v)))
            .collect::<Vec<_>>()
            .join("&");

        let mut uri = format!("{}://{}/{}", self.scheme(), host, percent_encode_str(path));
        if !query_pairs.is_empty() {
            uri.push_str(&format!("?{query_pairs}"));
        }

        Ok(Url::parse(&uri)?)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(
    tag = "type",
    content = "query",
    rename = "lowercase",
    rename_all = "snake_case"
)]
enum Query {
    /// Get a note by its unique identifier.
    Get(String),

    /// Search for a note by its title or content, optionally filtering by tags.
    Search { query: String, tags: Vec<String> },
}

/// A note from the Bear note-taking app.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Note {
    /// The unique identifier of the note.
    id: String,

    /// The title of the note.
    title: String,

    /// The content of the note.
    content: String,

    /// A list of tags associated with the note.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tags: Vec<String>,
}

impl Note {
    pub fn try_to_xml(&self) -> Result<String, Box<dyn Error + Send + Sync>> {
        let mut buffer = String::new();
        let mut serializer = quick_xml::se::Serializer::new(&mut buffer);
        serializer.indent(' ', 2);
        self.serialize(serializer)?;
        Ok(buffer)
    }
}

#[typetag::serde(name = "bear")]
#[async_trait]
impl Handler for BearNotes {
    fn scheme(&self) -> &'static str {
        "bear"
    }

    async fn add(&mut self, uri: &Url) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.0.insert(uri_to_query(uri)?);

        Ok(())
    }

    async fn remove(&mut self, uri: &Url) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.0.remove(&uri_to_query(uri)?);

        Ok(())
    }

    async fn list(&self) -> Result<Vec<Url>, Box<dyn Error + Send + Sync>> {
        let mut uris = vec![];
        for query in &self.0 {
            uris.push(self.query_to_uri(query)?);
        }

        Ok(uris)
    }

    async fn get(
        &self,
        _: &Utf8Path,
        _: Client,
    ) -> Result<Vec<Attachment>, Box<dyn Error + Send + Sync>> {
        let db = get_database_path()?;
        trace!(db = %db, "Connecting to Bear database.");
        let conn = Connection::open(db)?;

        let mut attachments = vec![];
        for query in &self.0 {
            for note in get_notes(query, &conn)? {
                attachments.push(Attachment {
                    source: format!("{}://get/{}", self.scheme(), &note.id),
                    content: note.try_to_xml()?,
                    description: Some("A note from the Bear note-taking app.".to_owned()),
                });
            }
        }

        Ok(attachments)
    }
}

fn uri_to_query(uri: &Url) -> Result<Query, Box<dyn Error + Send + Sync>> {
    let path = uri.path().trim_start_matches('/');
    let path = percent_decode_str(path)?;
    let query_pairs = uri
        .query_pairs()
        .map(|(k, v)| percent_decode_str(&v).map(|v| (k.to_string(), v)))
        .collect::<Result<Vec<_>, _>>()?;

    let query = match uri.host_str() {
        // Support official "Copy Link" x-callback-url links:
        // bear://x-callback-url/open-note?id=E340A2C4-8671-4233-860B-6AEFF7CB00D8
        Some("x-callback-url") if path == "open-note" => query_pairs
            .into_iter()
            .find_map(|(k, v)| (k == "id").then_some(v))
            .ok_or("Missing note id")
            .map(Query::Get)?,
        Some("get") => Query::Get(path),
        Some("search") => {
            let tags = query_pairs
                .into_iter()
                .filter_map(|(k, v)| if k == "tag" { Some(v) } else { None })
                .collect();

            Query::Search { query: path, tags }
        }
        _ => return Err("Invalid bear note query".into()),
    };

    Ok(query)
}

/// Retrieves notes from the Bear database based on the query.
fn get_notes(query: &Query, conn: &Connection) -> Result<Vec<Note>, Box<dyn Error + Send + Sync>> {
    rusqlite::vtab::array::load_module(conn)?;

    let mut notes = Vec::new();

    let ids = match query {
        Query::Get(id) => vec![id.to_owned()],
        Query::Search { query, tags } => {
            let pat = format!("%{query}%");

            let sql = if tags.is_empty() {
                "SELECT ZUNIQUEIDENTIFIER FROM ZSFNOTE N
                  WHERE (N.ZTITLE LIKE ?1 OR N.ZTEXT LIKE ?1)
                    AND N.ZTRASHED = 0 AND N.ZENCRYPTED = 0"
            } else {
                "SELECT N.ZUNIQUEIDENTIFIER
                  FROM ZSFNOTE N
                  JOIN Z_5TAGS NT ON N.Z_PK = NT.Z_5NOTES
                  JOIN ZSFNOTETAG T ON NT.Z_13TAGS = T.Z_PK
                  WHERE (N.ZTITLE LIKE ?1 OR N.ZTEXT LIKE ?1)
                    AND T.ZTITLE IN rarray(?2)
                    AND N.ZTRASHED = 0 AND N.ZENCRYPTED = 0
                  GROUP BY N.ZUNIQUEIDENTIFIER"
            };

            let mut stmt = conn.prepare(sql)?;

            if tags.is_empty() {
                stmt.query_map(params![pat], |row| row.get(0))?
                    .collect::<Result<Vec<String>, _>>()?
            } else {
                let values = Rc::new(
                    tags.iter()
                        .cloned()
                        .map(Value::from)
                        .collect::<Vec<Value>>(),
                );
                stmt.query_map(params![pat, values], |row| row.get(0))?
                    .collect::<Result<Vec<String>, _>>()?
            }
        }
    };

    let mut note_stmt = conn.prepare(
        "SELECT Z_PK, ZTITLE, ZTEXT FROM ZSFNOTE
         WHERE ZUNIQUEIDENTIFIER = ?1 AND ZTRASHED = 0 AND ZENCRYPTED = 0",
    )?;

    let mut tag_stmt = conn.prepare(
        "SELECT T.ZTITLE
         FROM ZSFNOTETAG T
         JOIN Z_5TAGS NT ON T.Z_PK = NT.Z_13TAGS
         WHERE NT.Z_5NOTES = ?1",
    )?;

    for id in ids {
        let row = note_stmt
            .query_row(params![&id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .optional()?;

        let Some((pk, title, content)) = row else {
            warn!(%id, "Note not found for given id");
            continue;
        };

        let tags = tag_stmt
            .query_map(params![pk], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<String>, _>>()?;

        notes.push(Note {
            id,
            title,
            content,
            tags,
        });
    }

    debug!(?query, notes = %notes.len(), "Query completed.",);
    Ok(notes)
}

/// Attempts to find the path to the Bear Sqlite database.
/// Assumes the standard macOS location.
fn get_database_path() -> Result<Utf8PathBuf, Box<dyn Error + Send + Sync>> {
    let path = BaseDirs::new()
        .ok_or("Could not find base directories")?
        .home_dir()
        .join(DB_PATH);

    if !path.exists() {
        return Err(format!("Missing Bear SQLite database at {}", path.display()).into());
    }

    path.try_into().map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use test_log::test;

    use super::*;

    #[test]
    fn test_note_try_to_xml() {
        let note = Note {
            id: 1.to_string(),
            title: "Test Title".to_string(),
            content: "Testing content in XML".to_string(),
            tags: vec!["tag #1".to_string(), "tag #2".to_string()],
        };

        let xml = note.try_to_xml().unwrap();
        assert_eq!(xml, indoc::indoc! {"
            <Note>
              <id>1</id>
              <title>Test Title</title>
              <content>Testing content in XML</content>
              <tags>tag #1</tags>
              <tags>tag #2</tags>
            </Note>"});
    }

    #[test]
    fn test_uri_to_query() {
        let cases = [
            (
                "bear://x-callback-url/open-note?id=123-456",
                Ok(Query::Get("123-456".to_string())),
            ),
            ("bear://get/1", Ok(Query::Get("1".to_string()))),
            (
                "bear://get/tag%20%231",
                Ok(Query::Get("tag #1".to_string())),
            ),
            (
                "bear://search/tag%20%231",
                Ok(Query::Search {
                    query: "tag #1".to_string(),
                    tags: vec![],
                }),
            ),
            (
                "bear://search/tag%20%231?tag=tag%20%232",
                Ok(Query::Search {
                    query: "tag #1".to_string(),
                    tags: vec!["tag #2".to_string()],
                }),
            ),
            (
                "bear://search/tag%20%231?tag=tag%20%232&tag=tag%20%233",
                Ok(Query::Search {
                    query: "tag #1".to_string(),
                    tags: vec!["tag #2".to_string(), "tag #3".to_string()],
                }),
            ),
            (
                "bear://invalid/foo",
                Err("Invalid bear note query".to_string()),
            ),
        ];

        for (uri, expected) in cases {
            let uri = Url::parse(uri).unwrap();
            let query = uri_to_query(&uri).map_err(|e| e.to_string());
            assert_eq!(query, expected);
        }
    }

    #[test]
    fn test_get_notes() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(indoc::indoc! {"
            CREATE TABLE ZSFNOTE (
                Z_PK INTEGER PRIMARY KEY,
                ZUNIQUEIDENTIFIER VARCHAR,
                ZTEXT VARCHAR,
                ZTITLE VARCHAR,
                ZTRASHED INTEGER,
                ZENCRYPTED INTEGER
            );

            INSERT INTO ZSFNOTE
                (Z_PK, ZUNIQUEIDENTIFIER, ZTITLE, ZTEXT, ZTRASHED, ZENCRYPTED)
            VALUES
                (1, '1', 'Test Title', 'Testing content in XML', 0, 0),
                (2, '2', 'Test Title 2', 'Testing content in XML 2', 0, 0);

            CREATE TABLE Z_5TAGS (
                Z_5NOTES INTEGER,
                Z_13TAGS INTEGER
            );

            INSERT INTO Z_5TAGS
                (Z_5NOTES, Z_13TAGS)
            VALUES
                (1, 1),
                (2, 2);

            CREATE TABLE ZSFNOTETAG (
                Z_PK INTEGER PRIMARY KEY,
                ZTITLE VARCHAR
            );

            INSERT INTO ZSFNOTETAG
                (Z_PK, ZTITLE)
            VALUES
                (1, 'tag #1'),
                (2, 'tag #2');
        "})
            .unwrap();

        let notes = get_notes(&Query::Get("1".to_string()), &conn).unwrap();

        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0], Note {
            id: "1".to_string(),
            title: "Test Title".to_string(),
            content: "Testing content in XML".to_string(),
            tags: vec!["tag #1".to_string()],
        });

        let notes = get_notes(
            &Query::Search {
                query: "Testing content".to_string(),
                tags: vec![],
            },
            &conn,
        )
        .unwrap();

        assert_eq!(notes.len(), 2);
        assert_eq!(notes, vec![
            Note {
                id: "1".to_string(),
                title: "Test Title".to_string(),
                content: "Testing content in XML".to_string(),
                tags: vec!["tag #1".to_string()],
            },
            Note {
                id: "2".to_string(),
                title: "Test Title 2".to_string(),
                content: "Testing content in XML 2".to_string(),
                tags: vec!["tag #2".to_string()],
            }
        ]);

        let notes = get_notes(
            &Query::Search {
                query: "Testing content".to_string(),
                tags: vec!["tag #2".to_string()],
            },
            &conn,
        )
        .unwrap();

        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0], Note {
            id: "2".to_string(),
            title: "Test Title 2".to_string(),
            content: "Testing content in XML 2".to_string(),
            tags: vec!["tag #2".to_string()],
        });
    }
}
