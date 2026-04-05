use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

use crate::{Error, Note, Result, Tag, schema, search};

/// Path to the Bear SQLite database relative to the home directory.
///
/// See: <https://bear.app/faq/where-are-bears-notes-located/>
const DB_PATH: &str =
    "Library/Group Containers/9K33E3U3T4.net.shinyfrog.bear/Application Data/database.sqlite";

/// Handle to the Bear database.
///
/// Each query opens a short-lived, read-only connection to minimize
/// interference with Bear's own writes.
pub struct BearDb {
    path: DbPath,
    cte: String,
}

#[derive(Debug, Clone)]
enum DbPath {
    File(String),

    #[cfg(test)]
    Memory,
}

impl BearDb {
    /// Open the Bear database at its default macOS location.
    pub fn open() -> Result<Self> {
        let home = directories::BaseDirs::new()
            .ok_or(Error::NoHomeDirectory)?
            .home_dir()
            .to_path_buf();

        let path = home.join(DB_PATH);
        if !path.exists() {
            return Err(Error::DatabaseNotFound {
                path: path.display().to_string(),
            });
        }

        let path_str = path
            .to_str()
            .ok_or_else(|| Error::Other("Non-UTF8 database path".into()))?
            .to_owned();

        // Discover schema metadata with a temporary connection
        let conn = Self::open_readonly(&path_str)?;
        let meta = schema::discover(&conn)?;
        tracing::info!(
            junction_table = %meta.junction_table,
            notes_column = %meta.notes_column,
            tags_column = %meta.tags_column,
            "discovered Bear schema"
        );
        let cte = schema::normalizing_cte(&meta);
        drop(conn);

        Ok(Self {
            path: DbPath::File(path_str),
            cte,
        })
    }

    /// Create an in-memory database for testing.
    #[cfg(test)]
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        crate::db::tests::setup_test_schema(&conn);
        let meta = schema::discover(&conn)?;
        let cte = schema::normalizing_cte(&meta);
        drop(conn);

        Ok(Self {
            path: DbPath::Memory,
            cte,
        })
    }

    /// Execute a closure with a short-lived read-only connection.
    pub fn with_connection<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection, &str) -> Result<T>,
    {
        let conn = match &self.path {
            DbPath::File(path) => Self::open_readonly(path)?,

            #[cfg(test)]
            DbPath::Memory => {
                let conn = Connection::open_in_memory()?;
                tests::setup_test_schema(&conn);
                conn
            }
        };

        f(&conn, &self.cte)
    }

    /// Get one or more notes by their IDs.
    pub fn get_notes(&self, ids: &[&str]) -> Result<Vec<Note>> {
        self.with_connection(|conn, cte| {
            let mut notes = Vec::with_capacity(ids.len());

            for id in ids {
                let Some(note) = Note::get_by_id(conn, cte, id)? else {
                    continue;
                };

                notes.push(note);
            }

            Ok(notes)
        })
    }

    /// Search notes by query text, optionally filtering by tags and/or IDs.
    pub fn search(&self, params: &search::SearchParams) -> Result<Vec<search::SearchMatch>> {
        self.with_connection(|conn, cte| search::execute(conn, cte, params))
    }

    /// Get all tags.
    pub fn tags(&self) -> Result<Vec<Tag>> {
        self.with_connection(Tag::list)
    }

    fn open_readonly(path: &str) -> Result<Connection> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "query_only", "ON")?;

        Ok(conn)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use rusqlite::Connection;

    /// Set up a Bear-like schema with sample data in an in-memory database.
    pub fn setup_test_schema(conn: &Connection) {
        conn.execute_batch(
            r"
            CREATE TABLE ZSFNOTE (
                Z_PK INTEGER PRIMARY KEY,
                ZUNIQUEIDENTIFIER TEXT,
                ZTITLE TEXT,
                ZTEXT TEXT,
                ZMODIFICATIONDATE REAL,
                ZCREATIONDATE REAL,
                ZTRASHED INTEGER,
                ZARCHIVED INTEGER,
                ZENCRYPTED INTEGER DEFAULT 0
            );

            INSERT INTO ZSFNOTE
                (Z_PK, ZUNIQUEIDENTIFIER, ZTITLE, ZTEXT, ZMODIFICATIONDATE, ZCREATIONDATE, ZTRASHED, ZARCHIVED)
            VALUES
                (1, 'note-1', 'Getting Things Done', 'A productivity method by David Allen.' || char(10) || 'It focuses on capturing tasks.', 0, 0, 0, 0),
                (2, 'note-2', 'Pomodoro Technique', 'Work in 25-minute intervals.' || char(10) || 'Take short breaks between pomodoros.', 0, 0, 0, 0),
                (3, 'note-3', 'Shopping List', 'Eggs' || char(10) || 'Milk' || char(10) || 'Bread', 0, 0, 0, 0),
                (4, 'note-4', 'Trashed Note', 'This is trashed', 0, 0, 1, 0);

            CREATE TABLE ZSFNOTETAG (
                Z_PK INTEGER PRIMARY KEY,
                ZTITLE TEXT
            );

            INSERT INTO ZSFNOTETAG (Z_PK, ZTITLE)
            VALUES
                (1, 'productivity'),
                (2, 'personal'),
                (3, 'projects/jp');

            CREATE TABLE Z_5TAGS (
                Z_5NOTES INTEGER,
                Z_13TAGS INTEGER
            );

            INSERT INTO Z_5TAGS (Z_5NOTES, Z_13TAGS)
            VALUES
                (1, 1),
                (2, 1),
                (3, 2),
                (1, 3);
            ",
        )
        .unwrap();
    }
}
