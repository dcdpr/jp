//! Ticket files on disk: id allocation and the create, comment, close, and list
//! operations.
//!
//! Ids come from a counter file rather than from the highest file in the
//! directory, so deleting a ticket never frees its number for reuse.
//! The counter is the authority; the files on disk are consulted only as a
//! floor, so a counter that lost an increment to a bad merge still can't hand
//! out an id that already exists.

use std::{fmt, fs, io};

use camino::{Utf8Path, Utf8PathBuf};

use crate::{
    Comment, Kind, ParseError, Status, Ticket, TicketId,
    import::{Import, escaped},
    parse, render,
};

/// Directory holding the ticket files, relative to the workspace root.
pub const DEFAULT_DIR: &str = "docs/ticket";

/// File holding the highest ticket id ever handed out.
const COUNTER: &str = ".counter";

type Result<T> = std::result::Result<T, Error>;

/// Something went wrong reading or writing a ticket file.
#[derive(Debug)]
pub enum Error {
    /// No file in the directory carries this id.
    NoSuchTicket(TicketId),
    /// A reply named a comment position the ticket doesn't have.
    NoSuchComment { id: TicketId, position: usize },
    /// A ticket file that isn't a well-formed ticket.
    Parse(ParseError),
    /// The filesystem said no.
    Io(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSuchTicket(id) => write!(f, "No ticket {id}."),
            Self::NoSuchComment { id, position } => {
                write!(f, "{id} has no comment #{position} to reply to.")
            }
            Self::Parse(error) => error.fmt(f),
            Self::Io(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Parse(error) => Some(error),
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ParseError> for Error {
    fn from(error: ParseError) -> Self {
        Self::Parse(error)
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// A ticket file and the result of reading it.
///
/// Listing keeps unreadable files rather than failing on them: one hand-mangled
/// ticket shouldn't hide the rest of the board.
pub struct Entry {
    pub path: Utf8PathBuf,
    pub ticket: std::result::Result<Ticket, ParseError>,
}

/// Create a ticket at `Todo`, returning its id and path.
///
/// `implements` names the RFD this work comes from, if any.
pub fn create(
    dir: &Utf8Path,
    kind: Kind,
    title: &str,
    authors: &str,
    date: &str,
    implements: Option<&str>,
    description: &str,
) -> Result<(TicketId, Utf8PathBuf)> {
    let id = allocate_id(dir)?;
    let path = dir.join(format!("{}{}.md", id.file_prefix(), slug(title)));
    fs::write(
        &path,
        render::ticket(id, title, kind, authors, date, implements, description),
    )?;

    Ok((id, path))
}

/// Record that a ticket became an RFD, and close it.
///
/// The work item is finished; the work moved.
/// Writing the link before the status means a retry after a failure finds the
/// link already there.
pub fn promote(dir: &Utf8Path, id: TicketId, rfd: &str) -> Result<Utf8PathBuf> {
    let path = locate(dir, id)?;
    let source = fs::read_to_string(&path)?;

    let updated =
        render::set_metadata(&source, "Promoted to", rfd).ok_or(ParseError::MissingMetadata)?;
    let updated = render::set_metadata(&updated, "Status", &Status::Done.to_string())
        .ok_or(ParseError::MissingMetadata)?;
    fs::write(&path, updated)?;

    Ok(path)
}

/// Append a comment to a ticket, returning its 1-based position.
///
/// `re` is the position of the comment being replied to.
pub fn append_comment(
    dir: &Utf8Path,
    id: TicketId,
    from: &str,
    date: &str,
    re: Option<usize>,
    body: &str,
) -> Result<usize> {
    let path = locate(dir, id)?;
    let source = fs::read_to_string(&path)?;
    let count = parse::comment_count(&source);

    let re = match re {
        Some(position) if position == 0 || position > count => {
            return Err(Error::NoSuchComment { id, position });
        }
        Some(position) => Some(format!("{id}#{position}")),
        None => None,
    };

    let comment = Comment {
        from: from.to_owned(),
        date: date.to_owned(),
        re,
        body: body.to_owned(),
    };
    fs::write(&path, render::append_comment(&source, &comment))?;

    Ok(count + 1)
}

/// What an import did.
pub struct Imported {
    pub id: TicketId,
    pub path: Utf8PathBuf,
    /// Whether the ticket was created rather than refreshed.
    pub created: bool,
    pub comments: usize,
}

/// Replace a ticket's content from an upstream issue, creating it if this is
/// the first import.
///
/// The ticket is found by its `GitHub` field, so the local id and the upstream
/// number stay independent.
/// Re-importing is safe and idempotent: the metadata block survives, so triage
/// done here is never undone from there.
pub fn import(dir: &Utf8Path, upstream: &Import<'_>) -> Result<Imported> {
    let marker = upstream.marker();
    let (title, description, comments) = escaped(upstream);

    let existing = list(dir)?.into_iter().find(|entry| {
        entry
            .ticket
            .as_ref()
            .is_ok_and(|ticket| ticket.metadata.github.as_deref() == Some(marker.as_str()))
    });

    let (id, path, created) = if let Some(entry) = existing {
        (entry.ticket.map_err(Error::Parse)?.id, entry.path, false)
    } else {
        let (id, path) = create(
            dir,
            upstream.kind,
            &title,
            upstream.authors,
            upstream.date,
            None,
            "",
        )?;

        // Record the link before writing content, so a failure halfway leaves a
        // ticket the next import will find rather than duplicate.
        let source = fs::read_to_string(&path)?;
        let linked =
            render::set_metadata(&source, "GitHub", &marker).ok_or(ParseError::MissingMetadata)?;
        fs::write(&path, linked)?;

        (id, path, true)
    };

    let source = fs::read_to_string(&path)?;
    let updated = render::replace_content(&source, id, &title, &description, &comments)
        .ok_or(ParseError::MissingMetadata)?;
    fs::write(&path, updated)?;

    Ok(Imported {
        id,
        path,
        created,
        comments: comments.len(),
    })
}

/// Mark a ticket as `Done`, returning its path and the status it held before.
pub fn close(dir: &Utf8Path, id: TicketId) -> Result<(Utf8PathBuf, Status)> {
    let path = locate(dir, id)?;
    let source = fs::read_to_string(&path)?;
    let previous = parse::document(&source)?.metadata.status;

    if previous != Status::Done {
        // The status line is required, and `parse::document` above rejected any
        // ticket that lacks one.
        let updated = render::set_metadata(&source, "Status", &Status::Done.to_string())
            .ok_or(ParseError::MissingField("Status"))?;
        fs::write(&path, updated)?;
    }

    Ok((path, previous))
}

/// Read every ticket in `dir`, ordered by id.
///
/// A directory that doesn't exist yet holds no tickets.
pub fn list(dir: &Utf8Path) -> Result<Vec<Entry>> {
    files(dir)?
        .into_iter()
        .map(|(_, path)| {
            let source = fs::read_to_string(&path)?;
            Ok(Entry {
                ticket: parse::document(&source),
                path,
            })
        })
        .collect()
}

/// Resolve a ticket id to its file.
fn locate(dir: &Utf8Path, id: TicketId) -> Result<Utf8PathBuf> {
    files(dir)?
        .into_iter()
        .find(|(found, _)| *found == id)
        .map(|(_, path)| path)
        .ok_or(Error::NoSuchTicket(id))
}

/// Every `NNNN-slug.md` in `dir`, paired with its id and ordered by it.
fn files(dir: &Utf8Path) -> Result<Vec<(TicketId, Utf8PathBuf)>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => return Err(error.into()),
    };

    let mut files = vec![];
    for entry in entries {
        // A path that isn't UTF-8 can't be a ticket file: ids and slugs are
        // ASCII.
        let Ok(path) = Utf8PathBuf::try_from(entry?.path()) else {
            continue;
        };
        if path.extension() != Some("md") {
            continue;
        }
        let Some(id) = path
            .file_stem()
            .and_then(|stem| stem.split_once('-'))
            .and_then(|(number, _)| number.parse::<TicketId>().ok())
        else {
            continue;
        };
        files.push((id, path));
    }
    files.sort_unstable();

    Ok(files)
}

/// Hand out the next id, recording it in the counter file.
fn allocate_id(dir: &Utf8Path) -> Result<TicketId> {
    fs::create_dir_all(dir)?;

    let counter = fs::read_to_string(dir.join(COUNTER))
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok())
        .unwrap_or(0);
    let highest = files(dir)?
        .into_iter()
        .map(|(id, _)| id.number())
        .max()
        .unwrap_or(0);

    let next = counter.max(highest) + 1;
    fs::write(dir.join(COUNTER), format!("{next}\n"))?;

    Ok(TicketId::new(next))
}

/// Build the filename slug from a title.
fn slug(title: &str) -> String {
    let mut slug = String::new();
    for char_ in title.chars() {
        match char_ {
            c if c.is_ascii_alphanumeric() => slug.push(c.to_ascii_lowercase()),
            '_' => slug.push('_'),
            _ if slug.ends_with('-') => {}
            _ => slug.push('-'),
        }
    }

    // Every pushed character is ASCII, so a byte index is a character index.
    let slug = slug.trim_matches('-');
    let cut = slug.len().min(60);

    match slug[..cut].trim_end_matches('-') {
        "" => "untitled".to_owned(),
        trimmed => trimmed.to_owned(),
    }
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
