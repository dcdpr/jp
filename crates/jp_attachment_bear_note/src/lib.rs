use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    rc::Rc,
};

use directories::BaseDirs;
use jp_attachment::{
    distributed_slice, linkme, typetag, Attachment, BoxedHandler, Handler, HANDLERS,
};
use rusqlite::{params, types::Value, Connection, OptionalExtension as _};
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
    fn query_to_uri(&self, query: &Query) -> Result<Url, Box<dyn std::error::Error>> {
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
    pub fn try_to_xml(&self) -> Result<String, Box<dyn std::error::Error>> {
        quick_xml::se::to_string(self).map_err(Into::into)
    }
}

#[typetag::serde(name = "bear")]
impl Handler for BearNotes {
    fn scheme(&self) -> &'static str {
        "bear"
    }

    fn add(&mut self, uri: &Url) -> Result<(), Box<dyn std::error::Error>> {
        self.0.insert(uri_to_query(uri)?);

        Ok(())
    }

    fn remove(&mut self, uri: &Url) -> Result<(), Box<dyn std::error::Error>> {
        self.0.remove(&uri_to_query(uri)?);

        Ok(())
    }

    fn list(&self) -> Result<Vec<Url>, Box<dyn std::error::Error>> {
        let mut uris = vec![];
        for query in &self.0 {
            uris.push(self.query_to_uri(query)?);
        }

        Ok(uris)
    }

    fn get(&self, _: &Path) -> Result<Vec<Attachment>, Box<dyn std::error::Error>> {
        let mut attachments = vec![];
        for query in &self.0 {
            for note in get_notes(query)? {
                attachments.push(Attachment {
                    source: format!("{}://get/{}", self.scheme(), &note.id),
                    content: note.try_to_xml()?,
                });
            }
        }

        Ok(attachments)
    }
}

/// Decodes a percent-encoded query parameter value, handling potential UTF-8
/// errors.
fn percent_decode_str(encoded: &str) -> Result<String, Box<dyn std::error::Error>> {
    percent_encoding::percent_decode_str(encoded)
        .decode_utf8()
        .map(|s| s.to_string())
        .map_err(Into::into)
}

fn percent_encode_str(encoded: &str) -> String {
    percent_encoding::percent_encode(encoded.as_bytes(), percent_encoding::NON_ALPHANUMERIC)
        .to_string()
}

fn uri_to_query(uri: &Url) -> Result<Query, Box<dyn std::error::Error>> {
    let path = uri.path().trim_start_matches('/');
    let path = percent_decode_str(path)?;
    let query_pairs = uri
        .query_pairs()
        .map(|(k, v)| percent_decode_str(&v).map(|v| (k.to_string(), v)))
        .collect::<Result<Vec<_>, _>>()?;

    let query = match uri.host_str() {
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
fn get_notes(query: &Query) -> Result<Vec<Note>, Box<dyn std::error::Error>> {
    let db = get_database_path()?;
    trace!(db = %db.display(), "Connecting to Bear database.");
    let conn = Connection::open(db)?;
    rusqlite::vtab::array::load_module(&conn)?;

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
fn get_database_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let path = BaseDirs::new()
        .ok_or("Could not find base directories")?
        .home_dir()
        .join(DB_PATH);

    if !path.exists() {
        return Err(format!("Missing Bear SQLite database at {}", path.display()).into());
    }

    Ok(path)
}
