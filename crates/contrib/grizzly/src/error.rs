#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Bear database not found at {path}")]
    DatabaseNotFound { path: String },

    #[error("Could not determine home directory")]
    NoHomeDirectory,

    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("Schema discovery failed: could not find junction table")]
    SchemaDiscovery,

    #[error("Note not found: {id}")]
    NoteNotFound { id: String },

    #[error("FTS5 search error: {reason}")]
    Fts { reason: String },

    #[error("{0}")]
    Other(String),
}
