use std::{collections::BTreeSet, error::Error};

use async_trait::async_trait;
use camino::Utf8Path;
use grizzly::{BearDb, search::SearchParams};
use jp_attachment::{
    Attachment, BoxedHandler, HANDLERS, Handler, distributed_slice, linkme, percent_decode_str,
    percent_encode_str, typetag,
};
use jp_mcp::Client;
use serde::{Deserialize, Serialize};
use tracing::debug;
use url::Url;

#[distributed_slice(HANDLERS)]
#[linkme(crate = linkme)]
static HANDLER: fn() -> BoxedHandler = handler;

fn handler() -> BoxedHandler {
    (Box::new(BearNotes::default()) as Box<dyn Handler>).into()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct BearNotes(BTreeSet<Query>);

impl BearNotes {
    fn query_to_uri(&self, query: &Query) -> Result<Url, Box<dyn Error + Send + Sync>> {
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

/// A note from the Bear note-taking app, formatted for attachment XML output.
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

impl From<grizzly::Note> for Note {
    fn from(note: grizzly::Note) -> Self {
        Self {
            id: note.id,
            title: note.title,
            content: note.content.unwrap_or_default(),
            tags: note.tags,
        }
    }
}

#[typetag::serde(name = "bear")]
#[async_trait]
impl Handler for BearNotes {
    fn scheme(&self) -> &'static str {
        "bear"
    }

    async fn add(
        &mut self,
        uri: &Url,
        _cwd: &Utf8Path,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
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
        let db = BearDb::open().map_err(|e| e.to_string())?;

        let mut attachments = vec![];
        for query in &self.0 {
            for note in get_notes(query, &db)? {
                attachments.push(
                    Attachment::text(
                        format!("{}://get/{}", self.scheme(), &note.id),
                        note.try_to_xml()?,
                    )
                    .with_description("A note from the Bear note-taking app."),
                );
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

/// Retrieve notes from Bear using the grizzly database layer.
fn get_notes(query: &Query, db: &BearDb) -> Result<Vec<Note>, Box<dyn Error + Send + Sync>> {
    let notes: Vec<Note> = match query {
        Query::Get(id) => db
            .get_notes(&[id.as_str()])
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(Note::from)
            .collect(),

        Query::Search { query, tags } => {
            let matches = db
                .search(&SearchParams {
                    queries: vec![query.clone()],
                    tags: tags.clone(),
                    context: 0,
                    ..Default::default()
                })
                .map_err(|e| e.to_string())?;

            // For each matched note, fetch the full note
            let ids: Vec<_> = matches.iter().map(|m| m.note_id.as_str()).collect();
            db.get_notes(&ids)
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(Note::from)
                .collect()
        }
    };

    debug!(?query, notes = %notes.len(), "Query completed.");

    Ok(notes)
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
