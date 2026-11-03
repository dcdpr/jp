//! Ticket files on disk: the create, comment, close, and list operations.
//!
//! Ids carry a time bucket and a random tail rather than a counter, so two
//! checkouts that cannot see each other still hand out different ids.
//! Nothing coordinates allocation; see
//! `docs/rfd/102-collision-resistant-ticket-identifiers.md`.
//!
//! Allocation is `doc::files`'s: the time component counts from the directory's
//! own [`Epoch`], which the first ticket in an empty directory writes.

use std::{fmt, fs, io};

use camino::{Utf8Path, Utf8PathBuf};
use doc::files;
use jp_label::Clash;

use crate::{
    Comment, Epoch, LABEL_KEY, Labels, NewTicket, PARENT_KEY, ParseError, Status, Ticket, TicketId,
    Vocabulary,
    check::{self, Directory, Problem, Report, TicketFile},
    epoch::{EPOCH_FILE, EpochError},
    id::TicketKind,
    import::{Import, escaped, github_marker},
    parent::{self, Member, Refusal},
    parse, render,
};

/// Directory holding the ticket files, relative to the workspace root.
pub const DEFAULT_DIR: &str = "docs/ticket";

pub use doc::files::MAX_SLUG_SIZE;

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
    ClockBeforeEpoch(Epoch),
    /// The directory holds tickets but no epoch file, so a new id can't be
    /// placed in time against theirs.
    MissingEpoch(Utf8PathBuf),
    /// The epoch file is there but doesn't hold an epoch.
    MalformedEpoch {
        path: Utf8PathBuf,
        error: EpochError,
    },
    /// The time component has no bucket left to express.
    ///
    /// Allocation refuses rather than wrapping, which would reuse old time
    /// prefixes, or widening, which would break the fixed-width form.
    Exhausted,
    /// A write named labels the board won't accept.
    Rejected(jp_label::Rejected),
    /// A write named labels the board lets one ticket carry, and another ticket
    /// already does.
    Taken(Vec<(Clash, TicketId)>),
    /// A write named labels the board lets one ticket carry, and gives them to
    /// another ticket it writes too.
    Shared(Vec<(Clash, TicketId)>),
    /// A write to several tickets the board refused, with each ticket refused
    /// and why.
    Refused(Vec<(TicketId, Error)>),
    /// A ticket file that is there but isn't a well-formed ticket.
    Malformed { id: TicketId, error: ParseError },
    /// The board's order file can't be read as one.
    Board { path: Utf8PathBuf, reason: String },
    /// A ticket was ranked against itself.
    SameTicket(TicketId),
    /// Two tickets ranked against each other sit in different columns.
    OtherColumn {
        id: TicketId,
        status: Status,
        other: TicketId,
        other_status: Status,
    },
    /// A ticket was to be placed next to one outside the column it moves to.
    NotInColumn {
        other: TicketId,
        other_status: Status,
        status: Status,
    },
    /// A write the parent relation's rules refuse.
    Parent(Refusal),
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
            Self::ClockBeforeEpoch(epoch) => {
                write!(
                    f,
                    "The system clock reads before {epoch}, when ticket ids start."
                )
            }
            Self::MissingEpoch(path) => write!(
                f,
                "{path} is missing, so a new ticket id can't be ordered against the existing \
                 ones. Restore it from version control."
            ),
            Self::MalformedEpoch { path, error } => write!(f, "{path}: {error}"),
            Self::Exhausted => f.write_str(
                "The ticket id format has no time buckets left; it needs a wider time component.",
            ),
            Self::Rejected(error) => error.fmt(f),
            Self::Taken(taken) => {
                let sentences: Vec<String> = taken
                    .iter()
                    .map(|(clash, id)| {
                        format!("`{clash}` is already on {id}, and {}.", held_to_one(clash))
                    })
                    .collect();
                f.write_str(&sentences.join(" "))
            }
            Self::Shared(shared) => {
                let sentences: Vec<String> = shared
                    .iter()
                    .map(|(clash, id)| {
                        format!(
                            "`{clash}` would also go on {id}, and {}.",
                            held_to_one(clash)
                        )
                    })
                    .collect();
                f.write_str(&sentences.join(" "))
            }
            Self::Refused(refused) => {
                let lines: Vec<String> = refused
                    .iter()
                    .map(|(id, error)| format!("{id}: {error}"))
                    .collect();
                f.write_str(&lines.join("\n"))
            }
            Self::Malformed { id, error } => write!(f, "{id} is not a well-formed ticket: {error}"),
            Self::Board { path, reason } => write!(f, "{path} is not a board order: {reason}."),
            Self::SameTicket(id) => write!(f, "{id} can't be ranked against itself."),
            Self::OtherColumn {
                id,
                status,
                other,
                other_status,
            } => write!(
                f,
                "{id} is {status} and {other} is {other_status}; a ticket is ranked within its \
                 own column."
            ),
            Self::NotInColumn {
                other,
                other_status,
                status,
            } => write!(
                f,
                "{other} is {other_status}, not {status}; a ticket is placed next to one in the \
                 column it moves to."
            ),
            Self::Parent(refusal) => refusal.fmt(f),
            Self::Io(error) => error.fmt(f),
        }
    }
}

/// Why the board refuses `clash` on a second ticket.
pub(crate) fn held_to_one(clash: &Clash) -> String {
    match clash.value {
        Some(_) => format!("the board lets one ticket carry each `{}` value", clash.key),
        None => "the board lets one ticket carry it".to_owned(),
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Parse(error) | Self::Malformed { error, .. } => Some(error),
            Self::MalformedEpoch { error, .. } => Some(error),
            Self::Rejected(error) => Some(error),
            Self::Parent(refusal) => Some(refusal),
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

impl From<jp_label::Rejected> for Error {
    fn from(error: jp_label::Rejected) -> Self {
        Self::Rejected(error)
    }
}

impl From<Refusal> for Error {
    fn from(refusal: Refusal) -> Self {
        Self::Parent(refusal)
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<files::Error<TicketKind>> for Error {
    fn from(error: files::Error<TicketKind>) -> Self {
        match error {
            files::Error::NotFound(id) => Self::NoSuchTicket(id),
            files::Error::Duplicate { id, paths } => Self::Duplicate { id, paths },
            files::Error::Contended => Self::Contended,
            files::Error::ClockBeforeEpoch(epoch) => Self::ClockBeforeEpoch(epoch),
            files::Error::MissingEpoch(path) => Self::MissingEpoch(path),
            files::Error::MalformedEpoch { path, error } => Self::MalformedEpoch { path, error },
            files::Error::Exhausted => Self::Exhausted,
            files::Error::Io(error) => Self::Io(error),
        }
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

/// Create a ticket at `Todo`, returning its id and path.
///
/// `new.labels` are resolved already; `vocabulary` holds them to the keys it
/// lets one ticket carry, which only the board can check.
/// `new.parent` is held to the rules of the parent relation.
///
/// Creating the file exclusively is what claims the id: two processes drawing
/// in the same bucket can land on one tail, and the loser draws again rather
/// than overwriting the winner's ticket.
pub fn create(
    dir: &Utf8Path,
    vocabulary: &Vocabulary,
    new: &NewTicket<'_>,
) -> Result<(TicketId, Utf8PathBuf)> {
    check_unique(dir, vocabulary, new.labels, None)?;
    if new.parent.is_some() {
        check_relations(dir, None, Status::Todo, new.parent)?;
    }

    Ok(files::create(
        dir,
        &files::slug(new.title),
        &render::ticket(new),
    )?)
}

/// Record that a ticket became an RFD, and close it.
///
/// The work item is finished; the work moved.
/// Writing the link before the status means a retry after a failure finds the
/// link already there.
/// Closing is held to the rules of the parent relation, so a parent with open
/// children is refused before anything is written.
pub fn promote(dir: &Utf8Path, id: TicketId, rfd: &str) -> Result<Utf8PathBuf> {
    let path = locate(dir, id)?;
    let source = fs::read_to_string(&path)?;
    // Read leniently, as the writes below are: a ticket whose header doesn't
    // parse still has children to wait on.
    let parent = parse::document(&source)
        .ok()
        .and_then(|ticket| ticket.metadata.parent);
    check_relations(dir, Some(id), Status::Done, parent)?;

    let updated =
        render::set_metadata(&source, "Promoted to", rfd).ok_or(ParseError::MissingMetadata)?;
    let updated = render::set_metadata(&updated, "Status", &Status::Done.to_string())
        .ok_or(ParseError::MissingMetadata)?;
    fs::write(&path, updated)?;

    Ok(path)
}

/// Where an edit left a ticket.
#[derive(Debug)]
pub struct Edited {
    /// The path the ticket was read from.
    pub from: Utf8PathBuf,
    /// The path it now lives at, which differs from `from` when a new title
    /// produced a different slug.
    pub to: Utf8PathBuf,
}

/// Rewrite a ticket's title and description, keeping its metadata and comments.
///
/// `None` leaves that part as it was.
///
/// A new title also moves the file to the slug that title produces, so the
/// filename never disagrees with the heading.
/// The id half of the name is kept, so references to the ticket still resolve.
/// An edit that leaves the title alone leaves the filename alone too, whatever
/// it says.
///
/// The content is written before the file moves, so an interrupted edit leaves
/// a ticket whose heading is right and whose slug is stale, and repeating the
/// call finishes the move.
pub fn edit(
    dir: &Utf8Path,
    id: TicketId,
    title: Option<&str>,
    description: Option<&str>,
) -> Result<Edited> {
    let from = locate(dir, id)?;
    let source = fs::read_to_string(&from)?;
    let ticket = parse::document(&source)?;

    let updated = render::replace_content(
        &source,
        title.unwrap_or(&ticket.title),
        description.unwrap_or(&ticket.description),
        &ticket.comments,
    )
    .ok_or(ParseError::MissingMetadata)?;
    fs::write(&from, updated)?;

    let to = match title {
        Some(title) => dir.join(format!("{}{}.md", id.file_prefix(), files::slug(title))),
        None => from.clone(),
    };

    // `fs::rename` replaces whatever sits at the target, but the target carries
    // this id's prefix and `locate` refused above if a second file claimed it,
    // so the only file it can land on is the one being moved.
    if to != from {
        fs::rename(&from, &to)?;
    }

    Ok(Edited { from, to })
}

/// Move a ticket's file to the name its title produces, after the file was
/// edited by hand.
///
/// The id half of the name is kept, so references to the ticket still resolve.
/// `from` and `to` are the same when the title still produces the old name.
pub fn rename_to_title(dir: &Utf8Path, id: TicketId) -> Result<Edited> {
    let from = locate(dir, id)?;
    let ticket = parse::document(&fs::read_to_string(&from)?)?;

    let to = dir.join(format!(
        "{}{}.md",
        id.file_prefix(),
        files::slug(&ticket.title)
    ));
    // As in `edit`: the target carries this id's prefix, and `locate` refused
    // above if a second file claimed it.
    if to != from {
        fs::rename(&from, &to)?;
    }

    Ok(Edited { from, to })
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

/// A ticket's labels before and after [`relabel`] wrote them.
#[derive(Debug)]
pub struct Relabeled {
    pub id: TicketId,
    pub path: Utf8PathBuf,
    /// What the ticket carried, as its label lines said.
    pub before: Labels,
    /// What it carries now, in the board's spelling.
    pub after: Labels,
}

/// Change the labels of every ticket `ids` names, all of them or none.
///
/// `change` maps the labels a ticket carries to the whole set it should end up
/// with, as tokens.
/// Each set is checked as [`Vocabulary::resolve_against`] checks one, so a
/// retired value the ticket already carries can be kept, and held to the keys
/// the board lets one ticket carry: against the rest of the board, and against
/// the other tickets `ids` names.
///
/// Every ticket is checked before any is written, so a refusal changes nothing.
/// With one ticket, the error is the board's own refusal; with several, it is
/// [`Error::Refused`], naming each ticket refused.
/// A ticket whose labels don't change isn't rewritten.
///
/// The writes go one file at a time: a failure partway through, such as a full
/// disk, leaves the tickets before it written.
pub fn relabel(
    dir: &Utf8Path,
    vocabulary: &Vocabulary,
    ids: &[TicketId],
    change: impl Fn(&Labels) -> Vec<String>,
) -> Result<Vec<Relabeled>> {
    let mut named: Vec<TicketId> = vec![];
    for id in ids {
        if !named.contains(id) {
            named.push(*id);
        }
    }

    let mut planned = vec![];
    let mut refused = vec![];
    for id in named.iter().copied() {
        let path = locate(dir, id)?;
        let source = fs::read_to_string(&path)?;
        let before = parse::labels(&source);
        let after = match vocabulary.resolve_against(&change(&before), &before) {
            Ok(after) => after,
            Err(rejected) => {
                refused.push((id, Error::Rejected(rejected)));
                continue;
            }
        };

        // Rendered here rather than at the write, so a ticket that can't take
        // its new labels refuses the batch before any ticket is written.
        let updated = if after == before {
            None
        } else if let Some(updated) =
            render::set_repeated_metadata(&source, LABEL_KEY, &after.to_tokens())
        {
            Some(updated)
        } else {
            refused.push((id, Error::Malformed {
                id,
                error: ParseError::MissingMetadata,
            }));
            continue;
        };

        planned.push((
            Relabeled {
                id,
                path,
                before,
                after,
            },
            updated,
        ));
    }

    let writing: Vec<(TicketId, &Labels)> = planned
        .iter()
        .map(|(ticket, _)| (ticket.id, &ticket.after))
        .collect();
    refused.extend(unique_refusals(dir, vocabulary, &writing)?);

    if !refused.is_empty() {
        // Reported in the order the tickets were named, whichever check
        // refused each.
        refused.sort_by_key(|(id, _)| named.iter().position(|named| named == id));

        return Err(if named.len() == 1 {
            refused.swap_remove(0).1
        } else {
            Error::Refused(refused)
        });
    }

    let mut done = vec![];
    for (ticket, updated) in planned {
        if let Some(updated) = updated {
            fs::write(&ticket.path, updated)?;
        }
        done.push(ticket);
    }

    Ok(done)
}

/// What the keys the board lets one ticket carry refuse among `writing`, the
/// tickets a write gives labels to: a label another ticket in `dir` already
/// carries, and one two of them would both carry.
///
/// Nothing is read when none of them carries a unique key.
fn unique_refusals(
    dir: &Utf8Path,
    vocabulary: &Vocabulary,
    writing: &[(TicketId, &Labels)],
) -> Result<Vec<(TicketId, Error)>> {
    if !writing
        .iter()
        .any(|(_, labels)| carries_unique(vocabulary, labels))
    {
        return Ok(vec![]);
    }

    let others = board_labels(dir, |id| writing.iter().any(|(written, _)| *written == id))?;

    let mut refused = vec![];
    for (index, (id, labels)) in writing.iter().enumerate() {
        let taken = clashes_with(
            vocabulary,
            labels,
            others.iter().map(|(other, theirs)| (*other, theirs)),
        );
        if !taken.is_empty() {
            refused.push((*id, Error::Taken(taken)));
        }

        // Each pair once, against the tickets named after this one.
        let shared = clashes_with(vocabulary, labels, writing[index + 1..].iter().copied());
        if !shared.is_empty() {
            refused.push((*id, Error::Shared(shared)));
        }
    }

    Ok(refused)
}

/// Refuse `labels` when another ticket in `dir` already carries one the
/// vocabulary lets one ticket carry.
///
/// `except` is the ticket being written, which can't clash with itself.
/// The other tickets' labels are read as written, whether or not the rest of
/// each file parses, so a ticket edited by hand still holds what it carries.
/// Nothing is read when `labels` carries no unique key.
///
/// # Errors
///
/// Returns [`Error::Taken`], naming each label and the ticket holding it.
pub fn check_unique(
    dir: &Utf8Path,
    vocabulary: &Vocabulary,
    labels: &Labels,
    except: Option<TicketId>,
) -> Result<()> {
    if !carries_unique(vocabulary, labels) {
        return Ok(());
    }

    let others = board_labels(dir, |id| Some(id) == except)?;
    let taken = clashes_with(
        vocabulary,
        labels,
        others.iter().map(|(other, theirs)| (*other, theirs)),
    );

    if taken.is_empty() {
        Ok(())
    } else {
        Err(Error::Taken(taken))
    }
}

/// Whether `labels` carries a key the vocabulary lets one ticket carry.
fn carries_unique(vocabulary: &Vocabulary, labels: &Labels) -> bool {
    vocabulary.facets().any(|(key, facet)| {
        facet.unique().is_some() && labels.keys().any(|label| label.eq_ignore_ascii_case(key))
    })
}

/// Every ticket in `dir` but the ones `skip` picks, with the labels its label
/// lines carry, read as written whether or not the rest of the file parses.
fn board_labels(
    dir: &Utf8Path,
    skip: impl Fn(TicketId) -> bool,
) -> Result<Vec<(TicketId, Labels)>> {
    let mut tickets = vec![];
    for (id, path) in files::list::<TicketKind>(dir)? {
        if !skip(id) {
            tickets.push((id, parse::labels(&fs::read_to_string(&path)?)));
        }
    }

    Ok(tickets)
}

/// The labels of `labels` that each of `others` carries too, among the keys the
/// vocabulary lets one ticket carry, paired with the ticket carrying it.
fn clashes_with<'a>(
    vocabulary: &Vocabulary,
    labels: &Labels,
    others: impl Iterator<Item = (TicketId, &'a Labels)>,
) -> Vec<(Clash, TicketId)> {
    others
        .flat_map(|(other, theirs)| {
            vocabulary
                .clashes(labels, theirs)
                .into_iter()
                .map(move |clash| (clash, other))
        })
        .collect()
}

/// Delete a ticket, returning the path that held it.
///
/// Unlike an RFD, a ticket can go: one carrying false claims or imported spam
/// is removed outright so nothing reads it as true.
/// Its id is not retired: a later creation in the same time bucket can draw it
/// again.
/// A ticket with children is refused, so none is left naming a parent that
/// isn't there.
pub fn delete(dir: &Utf8Path, id: TicketId) -> Result<Utf8PathBuf> {
    let path = locate(dir, id)?;
    parent::check_delete(&members(dir)?, id)?;
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

/// The ticket an earlier import of upstream issue `number` created, found by
/// its `GitHub` field.
pub fn imported(dir: &Utf8Path, number: u64) -> Result<Option<Entry>> {
    let marker = github_marker(number);

    Ok(list(dir)?.into_iter().find(|entry| {
        entry
            .ticket
            .as_ref()
            .is_ok_and(|ticket| ticket.metadata.github.as_deref() == Some(marker.as_str()))
    }))
}

/// Replace a ticket's content from an upstream issue, creating it if this is
/// the first import.
///
/// The ticket is found by its `GitHub` field, so the local id and the upstream
/// number stay independent.
/// Re-importing is safe and idempotent: the metadata block survives, so triage
/// done here is never undone from there.
/// `vocabulary` holds a new ticket's labels to the keys it lets one ticket
/// carry, as [`create`] does.
pub fn import(dir: &Utf8Path, vocabulary: &Vocabulary, upstream: &Import<'_>) -> Result<Imported> {
    let marker = upstream.marker();
    let (title, description, comments) = escaped(upstream);

    let existing = imported(dir, upstream.number)?;

    let (id, path, created) = if let Some(entry) = existing {
        (entry.id, entry.path, false)
    } else {
        // The repository owns the metadata block, so the labels are the
        // importer's, not upstream's.
        let (id, path) = create(dir, vocabulary, &NewTicket {
            title: &title,
            authors: upstream.authors,
            date: upstream.date,
            labels: upstream.labels,
            description: "",
            parent: None,
        })?;

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
    set_status(dir, id, Status::Done)
}

/// Move a ticket to the column `status` names, returning its path and the
/// status it held before.
///
/// Its place within the new column is the board's: a ticket the board file
/// doesn't rank there sorts below those it does.
/// The move is held to the rules of the parent relation, as [`relate`] holds
/// it.
pub fn set_status(dir: &Utf8Path, id: TicketId, status: Status) -> Result<(Utf8PathBuf, Status)> {
    let related = relate(dir, id, Some(status), None)?;

    Ok((related.path, related.previous_status))
}

/// Put a ticket under `parent`, or under nothing, returning its path and the
/// parent it had before.
///
/// Held to the rules of the parent relation, as [`relate`] holds it.
pub fn set_parent(
    dir: &Utf8Path,
    id: TicketId,
    parent: Option<TicketId>,
) -> Result<(Utf8PathBuf, Option<TicketId>)> {
    let related = relate(dir, id, None, Some(parent))?;

    Ok((related.path, related.previous_parent))
}

/// What [`relate`] changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Related {
    pub path: Utf8PathBuf,
    pub previous_status: Status,
    pub previous_parent: Option<TicketId>,
}

/// Change a ticket's status, its parent, or both, in one write.
///
/// `None` leaves that part as it is; `Some(None)` takes the parent away.
/// The ticket's state after the change is held to the rules of the parent
/// relation before anything is written, so moving a ticket and its parent at
/// once is judged by where it ends up rather than by the step between.
pub fn relate(
    dir: &Utf8Path,
    id: TicketId,
    status: Option<Status>,
    parent: Option<Option<TicketId>>,
) -> Result<Related> {
    let path = locate(dir, id)?;
    let source = fs::read_to_string(&path)?;
    let metadata = parse::document(&source)?.metadata;
    let next_status = status.unwrap_or(metadata.status);
    let next_parent = parent.unwrap_or(metadata.parent);

    let related = Related {
        path,
        previous_status: metadata.status,
        previous_parent: metadata.parent,
    };
    if next_status == metadata.status && next_parent == metadata.parent {
        return Ok(related);
    }

    check_relations(dir, Some(id), next_status, next_parent)?;

    let mut updated = source;
    if next_status != metadata.status {
        // The status line is required, and `parse::document` above rejected any
        // ticket that lacks one.
        updated = render::set_metadata(&updated, "Status", &next_status.to_string())
            .ok_or(ParseError::MissingField("Status"))?;
    }
    if next_parent != metadata.parent {
        updated = match next_parent {
            Some(parent) => render::set_metadata(&updated, PARENT_KEY, &parent.to_string())
                .ok_or(ParseError::MissingMetadata)?,
            None => render::remove_metadata(&updated, PARENT_KEY),
        };
    }
    fs::write(&related.path, updated)?;

    Ok(related)
}

/// Check what [`relate`] would do, without writing anything.
///
/// For a caller about to make several writes, [`relate`] among them, so it can
/// refuse before the first rather than after it.
///
/// # Errors
///
/// Returns [`Error::Parent`] with the rule the change breaks, or an error
/// reading the ticket or the directory.
pub fn check_relate(
    dir: &Utf8Path,
    id: TicketId,
    status: Option<Status>,
    parent: Option<Option<TicketId>>,
) -> Result<()> {
    let metadata = parse::document(&fs::read_to_string(locate(dir, id)?)?)?.metadata;
    let next_status = status.unwrap_or(metadata.status);
    let next_parent = parent.unwrap_or(metadata.parent);
    if next_status == metadata.status && next_parent == metadata.parent {
        return Ok(());
    }

    check_relations(dir, Some(id), next_status, next_parent)
}

/// Hold the ticket `id`, at `status` and under `parent`, to the rules of the
/// parent relation against the rest of `dir`, without writing anything.
///
/// `id` is `None` for a ticket not filed yet.
/// For a caller about to make several writes, so it can refuse before the first
/// rather than after it.
///
/// # Errors
///
/// Returns [`Error::Parent`] with the rule the change breaks, or an error
/// reading the directory.
pub fn check_relations(
    dir: &Utf8Path,
    id: Option<TicketId>,
    status: Status,
    parent: Option<TicketId>,
) -> Result<()> {
    Ok(parent::check(&members(dir)?, id, status, parent)?)
}

/// The tickets in `dir` that parse, as the parent relation's rules see them.
///
/// One that doesn't parse can't be placed, and `check` reports it as malformed.
/// Two files claiming one id are both read rather than refused: the write
/// asking is to one ticket, which `locate` has already told apart.
fn members(dir: &Utf8Path) -> Result<Vec<Member>> {
    let mut members = vec![];
    for (id, path) in files::list::<TicketKind>(dir)? {
        if let Ok(ticket) = parse::document(&fs::read_to_string(&path)?) {
            members.push(Member::of(id, &ticket));
        }
    }

    Ok(members)
}

/// Read every ticket in `dir`, ordered by id.
///
/// A directory that doesn't exist yet holds no tickets.
/// Two files claiming one id is an error: nothing downstream can tell which of
/// them a reference means, so the board refuses to render rather than pick.
pub fn list(dir: &Utf8Path) -> Result<Vec<Entry>> {
    let found = files::list::<TicketKind>(dir)?;
    if let Some(error) = files::duplicate(&found) {
        return Err(error.into());
    }

    found
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

/// Read every ticket in `dir` and hold it to `vocabulary`, as [`check::report`]
/// does, and report an epoch file that is missing while tickets exist, or that
/// doesn't hold an epoch.
///
/// A directory that doesn't exist yet holds no tickets, and so no problems.
pub fn check(dir: &Utf8Path, vocabulary: &Vocabulary) -> Result<Report> {
    let mut tickets = vec![];
    for (id, path) in files::list::<TicketKind>(dir)? {
        let source = fs::read_to_string(&path)?;
        tickets.push(TicketFile { id, path, source });
    }

    let directory = Directory {
        legacy: legacy_files(dir)?,
        tickets,
    };

    let mut report = check::report(&directory, vocabulary);

    // The epoch is a directory-wide problem, which a report lists first.
    let epoch_problem = match read_epoch(dir) {
        Ok(Some(_)) => None,
        Ok(None) if directory.tickets.is_empty() => None,
        Ok(None) => Some(Problem::MissingEpoch(dir.join(EPOCH_FILE))),
        Err(Error::MalformedEpoch { path, error }) => Some(Problem::MalformedEpoch { path, error }),
        Err(error) => return Err(error),
    };
    if let Some(problem) = epoch_problem {
        report.problems.insert(0, problem);
    }

    Ok(report)
}

/// Every file in `dir` named in the four-digit `NNNN-slug.md` form, sorted.
///
/// [`files`] skips them, since their names hold no id, so they need a pass of
/// their own to be seen at all.
fn legacy_files(dir: &Utf8Path) -> Result<Vec<Utf8PathBuf>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => return Err(error.into()),
    };

    let mut legacy = vec![];
    for entry in entries {
        let Ok(path) = Utf8PathBuf::try_from(entry?.path()) else {
            continue;
        };
        let numbered = path
            .file_stem()
            .and_then(|stem| stem.split_once('-'))
            .is_some_and(|(number, slug)| {
                number.len() == 4 && number.bytes().all(|b| b.is_ascii_digit()) && !slug.is_empty()
            });
        if path.extension() == Some("md") && numbered {
            legacy.push(path);
        }
    }
    legacy.sort();

    Ok(legacy)
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
    Ok(files::locate(dir, id)?)
}

/// The id and slug of `path`, when it is a ticket file directly inside `dir`.
fn ticket_filename<'a>(dir: &Utf8Path, path: &'a Utf8Path) -> Option<(TicketId, &'a str)> {
    if !same_directory(dir, path.parent()?) || path.extension() != Some("md") {
        return None;
    }

    let (id, slug) = path.file_stem()?.split_once('-')?;
    if slug.is_empty() {
        return None;
    }

    id.parse::<TicketId>().ok().map(|id| (id, slug))
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

/// The epoch `dir`'s ids count time from, written as the start of today when
/// the directory holds no tickets yet.
///
/// Creates `dir` if it doesn't exist.
/// A directory holding tickets but no epoch file is an error rather than a
/// fresh start: a new epoch would sort every new id against the existing ones
/// by an unrelated clock.
///
/// # Errors
///
/// Returns an error when the epoch file is missing beside existing tickets or
/// doesn't hold an epoch, or when the filesystem refuses.
pub fn establish_epoch(dir: &Utf8Path) -> Result<Epoch> {
    Ok(files::establish_epoch::<TicketKind>(dir)?)
}

/// The epoch `dir`'s ids count time from, as its epoch file records it.
///
/// # Errors
///
/// Returns an error when there is no epoch file, or it doesn't hold an epoch.
pub fn load_epoch(dir: &Utf8Path) -> Result<Epoch> {
    Ok(files::load_epoch::<TicketKind>(dir)?)
}

/// The epoch `dir`'s epoch file holds, or `None` when there is no such file.
fn read_epoch(dir: &Utf8Path) -> Result<Option<Epoch>> {
    Ok(files::read_epoch::<TicketKind>(dir)?)
}

/// The bucket a unix timestamp falls in.
///
/// # Errors
///
/// Returns an error when the timestamp predates `epoch`, or falls past the last
/// bucket the format can express.
pub fn bucket_at(epoch: Epoch, unix_seconds: u64) -> Result<u32> {
    Ok(files::bucket_at::<TicketKind>(epoch, unix_seconds)?)
}

/// What a reassignment changed.
#[derive(Debug)]
pub struct Reassigned {
    /// The id the ticket is leaving behind.
    pub old: TicketId,
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

    let new = files::allocate_in(dir, bucket)?;
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
/// Returns an error when the clock reads before `epoch`, or past the last
/// bucket the format can express.
pub fn current_bucket(epoch: Epoch) -> Result<u32> {
    Ok(files::current_bucket::<TicketKind>(epoch)?)
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
