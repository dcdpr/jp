//! Documents kept one to a file, `<id>-<slug>.md`, in a directory whose [epoch
//! file] says when their ids start counting.
//!
//! Ids carry a time bucket and a random tail rather than a counter, so two
//! checkouts that cannot see each other still hand out different ids.
//! Nothing coordinates allocation; see
//! `docs/rfd/102-collision-resistant-ticket-identifiers.md`.
//!
//! The time component counts from the directory's own [`Epoch`], which the
//! first document in an empty directory writes.
//! [`create`] claims an id by creating its file exclusively, and draws again
//! when another process got there first.
//!
//! [epoch file]: crate::epoch

use std::{
    collections::hash_map::RandomState,
    error, fmt, fs,
    hash::BuildHasher,
    io::{self, Write},
    time::{SystemTime, UNIX_EPOCH},
};

use camino::{Utf8Path, Utf8PathBuf};

use crate::{
    epoch::{EPOCH_FILE, Epoch, EpochError},
    id::{Id, Kind, MAX_BUCKET, TAIL_SPACE},
};

/// Longest slug a file name carries; a title beyond this is cut.
///
/// Slugs are ASCII, so this is both a byte count and a character count.
pub const MAX_SLUG_SIZE: usize = 60;

/// Seconds one id bucket spans.
const BUCKET_SECS: u64 = 5;

/// How many ids to draw before giving up on a contended directory.
///
/// Each attempt sees the file the previous one lost to, so the tail advances
/// every round rather than redrawing into the same clash.
const CLAIM_ATTEMPTS: usize = 16;

/// Something went wrong finding, allocating, or claiming a document's id.
#[derive(Debug)]
pub enum Error<K> {
    /// No file in the directory carries this id.
    NotFound(Id<K>),

    /// Two files claim the same id.
    ///
    /// Two checkouts drew one id and both branches landed.
    Duplicate { id: Id<K>, paths: Vec<Utf8PathBuf> },

    /// Every id drawn was already claimed on disk.
    Contended,

    /// The clock reads before the epoch ids are measured from.
    ClockBeforeEpoch(Epoch),

    /// The directory holds documents but no epoch file, so a new id can't be
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

    /// The filesystem said no.
    Io(io::Error),
}

impl<K: Kind> fmt::Display for Error<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let noun = K::NOUN;

        match self {
            Self::NotFound(id) => write!(f, "No {noun} {id}."),
            Self::Duplicate { id, paths } => {
                let names = paths
                    .iter()
                    .map(|path| path.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(f, "{id} is claimed by more than one file: {names}.")
            }
            Self::Contended => write!(
                f,
                "Every id drawn was already taken; another process is creating {noun}s."
            ),
            Self::ClockBeforeEpoch(epoch) => write!(
                f,
                "The system clock reads before {epoch}, when {noun} ids start."
            ),
            Self::MissingEpoch(path) => write!(
                f,
                "{path} is missing, so a new {noun} id can't be ordered against the existing \
                 ones. Restore it from version control."
            ),
            Self::MalformedEpoch { path, error } => write!(f, "{path}: {error}"),
            Self::Exhausted => write!(
                f,
                "The {noun} id format has no time buckets left; it needs a wider time component."
            ),
            Self::Io(error) => error.fmt(f),
        }
    }
}

impl<K: Kind + fmt::Debug> error::Error for Error<K> {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::MalformedEpoch { error, .. } => Some(error),
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl<K> From<io::Error> for Error<K> {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Every document file directly in `dir`, paired with its id and ordered by it.
///
/// A file counts when its name is an id of kind `K`, a dash, and the rest of
/// the name, ending in `.md`.
/// A directory that doesn't exist yet holds none.
///
/// # Errors
///
/// Returns the I/O error of a directory that can't be read.
pub fn list<K: Kind>(dir: &Utf8Path) -> io::Result<Vec<(Id<K>, Utf8PathBuf)>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => return Err(error),
    };

    let mut files = vec![];
    for entry in entries {
        // A path that isn't UTF-8 can't be a document file: ids and slugs are
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
            .and_then(|(id, _)| id.parse::<Id<K>>().ok())
        else {
            continue;
        };
        files.push((id, path));
    }
    files.sort_unstable();

    Ok(files)
}

/// The one file in `dir` that `id` names.
///
/// Two files claiming the id is an error rather than a choice: a write by id
/// would otherwise land on whichever one the directory happened to yield first.
///
/// # Errors
///
/// Returns [`Error::NotFound`] when no file carries the id, and
/// [`Error::Duplicate`] when more than one does.
pub fn locate<K: Kind>(dir: &Utf8Path, id: Id<K>) -> Result<Utf8PathBuf, Error<K>> {
    let mut claiming: Vec<Utf8PathBuf> = list(dir)?
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

    claiming.pop().ok_or(Error::NotFound(id))
}

/// The first id more than one of `files` claims, as [`Error::Duplicate`].
///
/// `files` is ordered by id, as [`list`] returns it, so duplicates are
/// adjacent.
#[must_use]
pub fn duplicate<K>(files: &[(Id<K>, Utf8PathBuf)]) -> Option<Error<K>> {
    files
        .windows(2)
        .find(|pair| pair[0].0 == pair[1].0)
        .map(|pair| Error::Duplicate {
            id: pair[0].0,
            paths: vec![pair[0].1.clone(), pair[1].1.clone()],
        })
}

/// The epoch `dir`'s epoch file holds, or `None` when there is no such file.
///
/// # Errors
///
/// Returns [`Error::MalformedEpoch`] when the file doesn't hold an epoch.
pub fn read_epoch<K>(dir: &Utf8Path) -> Result<Option<Epoch>, Error<K>> {
    let path = dir.join(EPOCH_FILE);
    let source = match fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };

    source
        .parse()
        .map(Some)
        .map_err(|error| Error::MalformedEpoch { path, error })
}

/// The epoch `dir`'s ids count time from, as its epoch file records it.
///
/// # Errors
///
/// Returns an error when there is no epoch file, or it doesn't hold an epoch.
pub fn load_epoch<K>(dir: &Utf8Path) -> Result<Epoch, Error<K>> {
    read_epoch(dir)?.ok_or_else(|| Error::MissingEpoch(dir.join(EPOCH_FILE)))
}

/// The epoch `dir`'s ids count time from, written as the start of today when
/// the directory holds no documents of kind `K` yet.
///
/// Creates `dir` if it doesn't exist.
/// A directory holding documents but no epoch file is an error rather than a
/// fresh start: a new epoch would sort every new id against the existing ones
/// by an unrelated clock.
///
/// # Errors
///
/// Returns an error when the epoch file is missing beside existing documents or
/// doesn't hold an epoch, or when the filesystem refuses.
pub fn establish_epoch<K: Kind>(dir: &Utf8Path) -> Result<Epoch, Error<K>> {
    match read_epoch(dir)? {
        Some(epoch) => return Ok(epoch),
        None if !list::<K>(dir)?.is_empty() => {
            return Err(Error::MissingEpoch(dir.join(EPOCH_FILE)));
        }
        None => {}
    }

    let epoch = Epoch::day_of(now_seconds()?).ok_or(Error::Exhausted)?;

    fs::create_dir_all(dir)?;
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join(EPOCH_FILE))
    {
        Ok(mut file) => {
            file.write_all(epoch.render(K::WRITER).as_bytes())?;

            Ok(epoch)
        }
        // Another process creating its first document wrote one first; theirs
        // stands.
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => load_epoch(dir),
        Err(error) => Err(error.into()),
    }
}

/// The bucket a unix timestamp falls in.
///
/// # Errors
///
/// Returns an error when the timestamp predates `epoch`, or falls past the last
/// bucket the format can express.
pub fn bucket_at<K>(epoch: Epoch, unix_seconds: u64) -> Result<u32, Error<K>> {
    let bucket = unix_seconds
        .checked_sub(epoch.unix_seconds())
        .ok_or(Error::ClockBeforeEpoch(epoch))?
        / BUCKET_SECS;

    u32::try_from(bucket)
        .ok()
        .filter(|bucket| *bucket < MAX_BUCKET)
        .ok_or(Error::Exhausted)
}

/// The bucket the current time falls in.
///
/// # Errors
///
/// Returns an error when the clock reads before `epoch`, or past the last
/// bucket the format can express.
pub fn current_bucket<K>(epoch: Epoch) -> Result<u32, Error<K>> {
    let now = now_seconds().map_err(|_| Error::ClockBeforeEpoch(epoch))?;

    bucket_at(epoch, now)
}

/// Hand out an id in `bucket`, or the first bucket after it with room.
///
/// The tail is random, except when the bucket already holds an id.
/// Then it continues from the highest one, so documents created back to back
/// stay in creation order rather than shuffling.
/// Pass the current bucket for a new document, or the one a document was
/// created in to give it a fresh id that keeps its place in time.
///
/// Creates `dir` if it doesn't exist.
///
/// # Errors
///
/// Returns [`Error::Exhausted`] when no bucket the format can express has room,
/// or the I/O error of a directory that can't be read.
pub fn allocate_in<K: Kind>(dir: &Utf8Path, mut bucket: u32) -> Result<Id<K>, Error<K>> {
    fs::create_dir_all(dir)?;

    let existing: Vec<Id<K>> = list(dir)?.into_iter().map(|(id, _)| id).collect();

    loop {
        let highest = existing.iter().rev().find(|id| id.bucket() == bucket);
        let tail = match highest {
            Some(id) => match id.tail() + 1 {
                // The bucket is full, which takes 1,024 documents in five
                // seconds. Move to the next one rather than reusing a tail.
                next if next >= TAIL_SPACE => {
                    bucket += 1;
                    continue;
                }
                next => next,
            },
            None => random_tail(),
        };

        return Id::new(bucket, tail).ok_or(Error::Exhausted);
    }
}

/// Write `contents` into a new file in `dir`, named for a freshly allocated id
/// and `slug`, returning the id and the path.
///
/// The id comes from the current bucket of the directory's epoch, which the
/// first document in an empty directory writes.
/// An id sitting in a *future* bucket is ignored rather than continued from:
/// one merged document from a machine with a fast clock would otherwise drag
/// every later local timestamp forward with it.
///
/// Creating the file exclusively is what claims the id: two processes drawing
/// in the same bucket can land on one tail, and the loser finds out here rather
/// than overwriting the winner's document.
///
/// # Errors
///
/// Returns [`Error::Contended`] when every id drawn was taken first, and the
/// errors of [`establish_epoch`] and [`allocate_in`].
pub fn create<K: Kind>(
    dir: &Utf8Path,
    slug: &str,
    contents: &str,
) -> Result<(Id<K>, Utf8PathBuf), Error<K>> {
    let epoch = establish_epoch(dir)?;

    for _ in 0..CLAIM_ATTEMPTS {
        let id = allocate_in(dir, current_bucket(epoch)?)?;
        let path = dir.join(format!("{}{slug}.md", id.file_prefix()));

        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                file.write_all(contents.as_bytes())?;

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

/// The slug a document's file name carries after its id, from its title.
///
/// ASCII letters and digits are kept, lowercased, and so is `_`; every other
/// run of characters becomes one `-`.
/// The slug is cut to [`MAX_SLUG_SIZE`], and a title with nothing to keep
/// produces `untitled`.
#[must_use]
pub fn slug(title: &str) -> String {
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
    let cut = slug.len().min(MAX_SLUG_SIZE);

    match slug[..cut].trim_end_matches('-') {
        "" => "untitled".to_owned(),
        trimmed => trimmed.to_owned(),
    }
}

/// Seconds since the unix epoch, by the system clock.
///
/// A clock set before 1970 reads as an error rather than as zero.
fn now_seconds() -> io::Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .map_err(io::Error::other)
}

/// A random tail.
///
/// `RandomState` is seeded by the OS once per thread and advances per call, so
/// two processes starting in the same second draw independently.
/// Independent is not distinct: ten bits leave a 1-in-1,024 chance they land on
/// the same tail, which is the collision [`duplicate`] and the docs build exist
/// to catch.
/// Ten bits do not warrant a dependency on a generator.
fn random_tail() -> u16 {
    let value = RandomState::new().hash_one(SystemTime::now()) % u64::from(TAIL_SPACE);

    u16::try_from(value).unwrap_or_default()
}

#[cfg(test)]
#[path = "files_tests.rs"]
mod tests;
