use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use directories::BaseDirs;
use jp_attachment::{
    distributed_slice, linkme, typetag, Attachment, BoxedHandler, Handler, HANDLERS,
};
use rusqlite::{params, Connection, OptionalExtension as _};
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
        let (host, path) = match query {
            Query::Get(path) => ("get", path),
            Query::Tagged(path) => ("tagged", path),
            Query::Search(path) => ("search", path),
        };

        let path =
            percent_encoding::percent_encode(path.as_bytes(), percent_encoding::NON_ALPHANUMERIC)
                .to_string();

        Ok(Url::parse(&format!(
            "{}://{}/{}",
            self.scheme(),
            host,
            path
        ))?)
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

    /// Get all notes tagged with a specific tag.
    Tagged(String),

    /// Search for a note by its title or content.
    Search(String),
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

fn uri_to_query(uri: &Url) -> Result<Query, Box<dyn std::error::Error>> {
    let path = uri.path().trim_start_matches('/');
    let path = percent_encoding::percent_decode_str(path)
        .decode_utf8()?
        .to_string();

    let query = match uri.host_str() {
        Some("get") => Query::Get(path),
        Some("tagged") => Query::Tagged(path),
        Some("search") => Query::Search(path),
        _ => return Err("Invalid bear note query".into()),
    };

    Ok(query)
}

/// Retrieves notes from the Bear database based on the query.
fn get_notes(query: &Query) -> Result<Vec<Note>, Box<dyn std::error::Error>> {
    let db = get_database_path()?;
    trace!(db = %db.display(), "Connecting to Bear database.");
    let conn = Connection::open(db)?;

    let mut notes = Vec::new();

    let ids = match query {
        Query::Get(id) => vec![id.to_owned()],
        Query::Tagged(tag) => {
            let mut stmt = conn.prepare(
                "SELECT N.ZUNIQUEIDENTIFIER
                FROM ZSFNOTE N
                JOIN Z_5TAGS NT ON N.Z_PK = NT.Z_5NOTES
                JOIN ZSFNOTETAG T ON NT.Z_13TAGS = T.Z_PK
                WHERE T.ZTITLE = ?1 AND N.ZTRASHED = 0 AND N.ZENCRYPTED = 0",
            )?;
            stmt.query_map(params![tag], |row| row.get(0))?
                .collect::<Result<Vec<String>, _>>()?
        }
        Query::Search(query) => {
            let pat = format!("%{query}%");
            let mut stmt = conn.prepare(
                "SELECT ZUNIQUEIDENTIFIER FROM ZSFNOTE
                 WHERE (ZTITLE LIKE ?1 OR ZTEXT LIKE ?1) AND ZTRASHED = 0 AND ZENCRYPTED = 0",
            )?;

            stmt.query_map(params![pat], |row| row.get(0))?
                .collect::<Result<Vec<String>, _>>()?
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
