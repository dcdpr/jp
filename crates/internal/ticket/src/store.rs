//! Ticket files on disk: id allocation and the create, comment, close, and list
//! operations.
//!
//! Ids carry a time bucket and a random tail rather than a counter, so two
//! checkouts that cannot see each other still hand out different ids.
//! Nothing coordinates allocation; see
//! `docs/rfd/102-collision-resistant-ticket-identifiers.md`.

use std::{
    collections::hash_map::RandomState,
    fmt, fs,
    hash::BuildHasher,
    io::{self, Write},
    time::{SystemTime, UNIX_EPOCH},
};

use camino::{Utf8Path, Utf8PathBuf};

use crate::{
    Comment, Kind, ParseError, Status, Ticket, TicketId,
    id::{MAX_BUCKET, TAIL_SPACE},
    import::{Import, escaped},
    parse, render,
};

/// Directory holding the ticket files, relative to the workspace root.
pub const DEFAULT_DIR: &str = "docs/ticket";

/// Start of the id time range: `2026-08-10T00:00:00Z`, the day the ticket
/// system went live.
const EPOCH_SECS: u64 = 1_786_320_000;

/// Seconds one id bucket spans.
const BUCKET_SECS: u64 = 5;

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
    /// Two files claim the same id.
    ///
    /// Two checkouts drew one id and both branches landed.
    Duplicate {
        id: TicketId,
        paths: Vec<Utf8PathBuf>,
    },
    /// A path that isn't a ticket file in the directory being written to.
    NotATicketFile(Utf8PathBuf),
    /// Every id drawn was already claimed on disk.
    Contended,
    /// The clock reads before the epoch ids are measured from.
    ClockBeforeEpoch,
    /// The time component has no bucket left to express.
    ///
    /// Allocation refuses rather than wrapping, which would reuse old time
    /// prefixes, or widening, which would break the fixed-width form.
    Exhausted,
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
            Self::Duplicate { id, paths } => {
                let names = paths
                    .iter()
                    .map(|path| path.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(f, "{id} is claimed by more than one file: {names}.")
            }
            Self::NotATicketFile(path) => {
                write!(f, "{path} is not a ticket file in the ticket directory.")
            }
            Self::Contended => f.write_str(
                "Every id drawn was already taken; another process is creating tickets.",
            ),
            Self::ClockBeforeEpoch => {
                f.write_str("The system clock reads before 2026-08-10, when ticket ids start.")
            }
            Self::Exhausted => f.write_str(
                "The ticket id format has no time buckets left; it needs a wider time component.",
            ),
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
#[derive(Debug)]
pub struct Entry {
    /// The id, which the filename carries and the document does not.
    pub id: TicketId,
    pub path: Utf8PathBuf,
    pub ticket: std::result::Result<Ticket, ParseError>,
}

/// How many ids to draw before giving up on a contended directory.
///
/// Each attempt sees the file the previous one lost to, so the tail advances
/// every round rather than redrawing into the same clash.
const CLAIM_ATTEMPTS: usize = 16;

/// Create a ticket at `Todo`, returning its id and path.
///
/// `implements` names the RFD this work comes from, if any.
///
/// Creating the file exclusively is what claims the id: two processes drawing
/// in the same bucket can land on one tail, and the loser finds out here rather
/// than overwriting the winner's ticket.
pub fn create(
    dir: &Utf8Path,
    kind: Kind,
    title: &str,
    authors: &str,
    date: &str,
    implements: Option<&str>,
    description: &str,
) -> Result<(TicketId, Utf8PathBuf)> {
    let slug = slug(title);

    for _ in 0..CLAIM_ATTEMPTS {
        let id = allocate_id(dir)?;
        let path = dir.join(format!("{}{slug}.md", id.file_prefix()));

        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                let document = render::ticket(title, kind, authors, date, implements, description);
                file.write_all(document.as_bytes())?;

                return Ok((id, path));
            }
            // Another process claimed this id first. The next attempt sees its
            // file and draws a higher tail.
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }

    Err(Error::Contended)
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

/// Rewrite a ticket's title and description, keeping its metadata and comments.
///
/// `None` leaves that part as it was.
pub fn edit(
    dir: &Utf8Path,
    id: TicketId,
    title: Option<&str>,
    description: Option<&str>,
) -> Result<Utf8PathBuf> {
    let path = locate(dir, id)?;
    let source = fs::read_to_string(&path)?;
    let ticket = parse::document(&source)?;

    let updated = render::replace_content(
        &source,
        title.unwrap_or(&ticket.title),
        description.unwrap_or(&ticket.description),
        &ticket.comments,
    )
    .ok_or(ParseError::MissingMetadata)?;
    fs::write(&path, updated)?;

    Ok(path)
}

/// Set one metadata field on a ticket.
///
/// The field is created if the ticket doesn't carry it yet.
pub fn set_field(dir: &Utf8Path, id: TicketId, key: &str, value: &str) -> Result<Utf8PathBuf> {
    let path = locate(dir, id)?;
    let source = fs::read_to_string(&path)?;

    let updated = render::set_metadata(&source, key, value).ok_or(ParseError::MissingMetadata)?;
    fs::write(&path, updated)?;

    Ok(path)
}

/// Delete a ticket, returning the path that held it.
///
/// Unlike an RFD, a ticket can go: one carrying false claims or imported spam
/// is removed outright so nothing reads it as true.
/// Its id is not retired: a later creation in the same time bucket can draw it
/// again.
pub fn delete(dir: &Utf8Path, id: TicketId) -> Result<Utf8PathBuf> {
    let path = locate(dir, id)?;
    fs::remove_file(&path)?;

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
        // A reply always targets a comment on this ticket, so the position
        // alone says everything. Nothing in a ticket names the ticket.
        Some(position) => Some(format!("#{position}")),
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
        (entry.id, entry.path, false)
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
    let updated = render::replace_content(&source, &title, &description, &comments)
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
/// Two files claiming one id is an error: nothing downstream can tell which of
/// them a reference means, so the board refuses to render rather than pick.
pub fn list(dir: &Utf8Path) -> Result<Vec<Entry>> {
    let files = files(dir)?;
    if let Some(error) = duplicate(&files) {
        return Err(error);
    }

    files
        .into_iter()
        .map(|(id, path)| {
            let source = fs::read_to_string(&path)?;
            Ok(Entry {
                id,
                ticket: parse::document(&source),
                path,
            })
        })
        .collect()
}

/// Resolve a ticket id to its file.
pub fn locate_ticket(dir: &Utf8Path, id: TicketId) -> Result<Utf8PathBuf> {
    locate(dir, id)
}

/// Resolve a ticket id to its file.
///
/// Two files claiming the id is an error rather than a choice: a write by id
/// would otherwise land on whichever one the directory happened to yield first.
fn locate(dir: &Utf8Path, id: TicketId) -> Result<Utf8PathBuf> {
    let mut claiming: Vec<Utf8PathBuf> = files(dir)?
        .into_iter()
        .filter(|(found, _)| *found == id)
        .map(|(_, path)| path)
        .collect();

    if claiming.len() > 1 {
        return Err(Error::Duplicate {
            id,
            paths: claiming,
        });
    }

    claiming.pop().ok_or(Error::NoSuchTicket(id))
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

/// The reference token and slug of `path`, when it is a ticket file directly
/// inside `dir`.
///
/// The token is the id as other documents write it: `T-02wt0kx` now, `T0005`
/// for a ticket left in the pre-RFD-102 format, which is exactly what a
/// migration reassigns.
fn ticket_filename<'a>(dir: &Utf8Path, path: &'a Utf8Path) -> Option<(String, &'a str)> {
    if !same_directory(dir, path.parent()?) || path.extension() != Some("md") {
        return None;
    }

    let (id, slug) = path.file_stem()?.split_once('-')?;
    if slug.is_empty() {
        return None;
    }

    if id.len() == 4 && id.bytes().all(|byte| byte.is_ascii_digit()) {
        return Some((format!("T{id}"), slug));
    }

    id.parse::<TicketId>().ok().map(|id| (id.to_string(), slug))
}

/// Whether two paths name the same directory.
///
/// A repository reached through a symlink — `/tmp` on macOS, a linked home —
/// has two spellings for one directory, and comparing the strings alone would
/// reject a ticket sitting right where it belongs.
fn same_directory(left: &Utf8Path, right: &Utf8Path) -> bool {
    if left == right {
        return true;
    }

    match (left.canonicalize_utf8(), right.canonicalize_utf8()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

/// The first id claimed by more than one file, if any.
///
/// `files` is ordered by id, so duplicates are adjacent.
fn duplicate(files: &[(TicketId, Utf8PathBuf)]) -> Option<Error> {
    files
        .windows(2)
        .find(|pair| pair[0].0 == pair[1].0)
        .map(|pair| Error::Duplicate {
            id: pair[0].0,
            paths: vec![pair[0].1.clone(), pair[1].1.clone()],
        })
}

/// Hand out an id for a ticket about to be written.
///
/// The time component comes from the clock; the tail is random, except when
/// this bucket already holds an id.
/// Then it continues from the highest one, so tickets created back to back stay
/// in creation order rather than shuffling.
///
/// An id sitting in a *future* bucket is ignored rather than continued from:
/// one merged ticket from a machine with a fast clock would otherwise drag
/// every later local timestamp forward with it.
fn allocate_id(dir: &Utf8Path) -> Result<TicketId> {
    allocate_in(dir, current_bucket()?)
}

/// Hand out an id in `bucket`, or the first bucket after it with room.
fn allocate_in(dir: &Utf8Path, mut bucket: u32) -> Result<TicketId> {
    fs::create_dir_all(dir)?;

    let existing: Vec<TicketId> = files(dir)?.into_iter().map(|(id, _)| id).collect();

    loop {
        let highest = existing.iter().rev().find(|id| id.bucket() == bucket);
        let tail = match highest {
            Some(id) => match id.tail() + 1 {
                // The bucket is full, which takes 1,024 tickets in five
                // seconds. Move to the next one rather than reusing a tail.
                next if next >= TAIL_SPACE => {
                    bucket += 1;
                    continue;
                }
                next => next,
            },
            None => random_tail(),
        };

        return TicketId::new(bucket, tail).ok_or(Error::Exhausted);
    }
}

/// The bucket a unix timestamp falls in.
///
/// # Errors
///
/// Returns an error when the timestamp predates the epoch ids are measured
/// from, or falls past the last bucket the format can express.
pub fn bucket_at(unix_seconds: u64) -> Result<u32> {
    let bucket = unix_seconds
        .checked_sub(EPOCH_SECS)
        .ok_or(Error::ClockBeforeEpoch)?
        / BUCKET_SECS;

    u32::try_from(bucket)
        .ok()
        .filter(|bucket| *bucket < MAX_BUCKET)
        .ok_or(Error::Exhausted)
}

/// What a reassignment changed.
#[derive(Debug)]
pub struct Reassigned {
    /// The reference token the ticket is leaving behind, as other documents
    /// write it.
    ///
    /// A string rather than a [`TicketId`], so a ticket from an older format
    /// can report `T0005`.
    pub old: String,
    pub new: TicketId,
    pub path: Utf8PathBuf,
}

/// Give the ticket at `path` a new id by renaming its file.
///
/// `bucket` places the new id in time; pass the one the ticket was created in
/// so it keeps its position relative to everything around it.
/// The slug is kept, and references held by other files are the caller's to fix
/// — nothing here knows which of them meant this ticket.
///
/// The document is not touched: a ticket names itself nowhere inside its own
/// file, so moving the file is the whole operation.
///
/// `path` must name a ticket file directly inside `dir`, or an unrelated file
/// would be renamed into the ticket directory.
pub fn reassign(dir: &Utf8Path, path: &Utf8Path, bucket: u32) -> Result<Reassigned> {
    let (old, slug) =
        ticket_filename(dir, path).ok_or_else(|| Error::NotATicketFile(path.to_path_buf()))?;

    let new = allocate_in(dir, bucket)?;
    let target = dir.join(format!("{}{slug}.md", new.file_prefix()));
    if target != path {
        fs::rename(path, &target)?;
    }

    Ok(Reassigned {
        old,
        new,
        path: target,
    })
}

/// The bucket the current time falls in.
///
/// # Errors
///
/// Returns an error when the clock reads before the epoch, or past the last
/// bucket the format can express.
pub fn current_bucket() -> Result<u32> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::ClockBeforeEpoch)?
        .as_secs();

    bucket_at(now)
}

/// A random tail.
///
/// `RandomState` is seeded by the OS once per thread and advances per call, so
/// two processes starting in the same second draw independently.
/// Independent is not distinct: ten bits leave a 1-in-1,024 chance they land on
/// the same tail, which is the collision `store::list` and the docs build exist
/// to catch.
/// Ten bits do not warrant a dependency on a generator.
fn random_tail() -> u16 {
    let value = RandomState::new().hash_one(SystemTime::now()) % u64::from(TAIL_SPACE);

    u16::try_from(value).unwrap_or_default()
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
