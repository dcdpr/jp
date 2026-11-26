//! On-disk storage for reviews.
//!
//! ```text
//! <store>/
//!   .epoch
//!   .serve
//!   .serve.lock
//!   1h4k2m9/
//!     review.json
//!     .lock
//!     .pass
//!     blobs/<sha256>
//! ```
//!
//! Each review is stored in a directory named after its 7-character ID body.
//! `review.json` holds the review metadata, versions, comments, and passes;
//! file contents are stored in `blobs/` named by their lowercase hex SHA-256
//! digest.
//!
//! [`Store::update`] serializes mutations with an exclusive lock on `.lock`,
//! writing blobs before atomically replacing `review.json` via a temporary file
//! so lockless readers always observe a complete file.
//!
//! While a pass is running, its owning process holds an exclusive lock on
//! `.pass` ([`Store::lock_pass`]) from before recording the pass until after
//! recording its outcome.
//! If a review has a pass with `ended == None` and `.pass` is unlocked, the
//! process that started that pass has exited, and [`Store::read`] /
//! [`Store::update`] mark the pass `interrupted`.
//!
//! The `jp review serve` serving the store holds `.serve.lock` while it runs
//! ([`Store::claim_server`]), and writes the address it serves at to `.serve`.
//! A store has at most one server, and [`Store::server`] finds it.

use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{self, Write as _},
};

use camino::{Utf8Path, Utf8PathBuf};
use chrono::{DateTime, SubsecRound as _, Utc};
use doc::{files, lock};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};

use crate::{
    id::{ReviewId, ReviewKind},
    model::{FORMAT, Files, Review},
};

/// The file holding a review, inside its directory.
pub const REVIEW_FILE: &str = "review.json";

/// The file a review's writers lock, inside its directory.
const LOCK_FILE: &str = ".lock";

/// The file the process running a review's pass holds locked, inside its
/// directory.
const PASS_FILE: &str = ".pass";

/// What a new `review.json` is written to before it replaces the old one.
const TEMP_FILE: &str = ".review.json.tmp";

/// The file `jp review serve` writes the address it serves at to, in the
/// store's directory.
const SERVER_FILE: &str = ".serve";

/// The file the running `jp review serve` holds locked, in the store's
/// directory.
///
/// Apart from [`SERVER_FILE`] because on Windows no other process can read a
/// file one holds `File::lock` on.
const SERVER_LOCK: &str = ".serve.lock";

/// The directory holding a review's blobs, inside its directory.
const BLOB_DIR: &str = "blobs";

/// Hex digits in a SHA-256 hash, the name of a blob.
const HASH_DIGITS: usize = 64;

/// The directory of reviews.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Store {
    dir: Utf8PathBuf,
}

impl Store {
    pub fn new(dir: Utf8PathBuf) -> Self {
        Self { dir }
    }

    /// Store `review` under a new id, with the file `contents` its versions
    /// name, returning the id.
    pub fn create(&self, review: &Review, contents: &[&str]) -> Result<ReviewId, String> {
        let (id, dir) = files::create_dir::<ReviewKind>(&self.dir)
            .map_err(|error| format!("{}: {error}", self.dir))?;

        if let Err(error) = fill(&dir, review, contents) {
            // A directory without its `review.json` is no review, so the id is
            // given up rather than left to a directory nothing can read.
            drop(fs::remove_dir_all(&dir));
            return Err(error);
        }

        Ok(id)
    }

    /// Every review in the store, in the order they were created.
    pub fn list(&self) -> Result<Vec<Entry>, String> {
        let dirs = files::list_dirs::<ReviewKind>(&self.dir)
            .map_err(|error| format!("{}: {error}", self.dir))?;

        Ok(dirs
            .into_iter()
            .map(|(id, _)| Entry {
                id,
                review: self.read(id),
            })
            .collect())
    }

    /// The review `id`, with any pass left by a process that is gone recorded
    /// as interrupted.
    pub fn read(&self, id: ReviewId) -> Result<Review, String> {
        let review = self.peek(id)?;
        if review.passes.running().is_none() || !self.settle(id)? {
            return Ok(review);
        }

        self.peek(id)
    }

    /// The review `id` as `review.json` holds it, with a pass it shows running
    /// left as it is: what the process running that pass reads.
    pub fn peek(&self, id: ReviewId) -> Result<Review, String> {
        read(&self.dir.join(id.as_str()), id)
    }

    /// Whether the store holds the review `id`.
    pub fn exists(&self, id: ReviewId) -> bool {
        self.dir.join(id.as_str()).is_dir()
    }

    /// Take the lock the process running a pass of the review `id` holds, until
    /// the returned [`File`] is dropped.
    ///
    /// `None`, without waiting, while another process holds it: a review runs
    /// one pass at a time.
    pub fn lock_pass(&self, id: ReviewId) -> Result<Option<File>, String> {
        let dir = self.dir.join(id.as_str());
        self.peek(id)?;

        lock::try_exclusive(&dir.join(PASS_FILE)).map_err(|error| format!("{dir}: {error}"))
    }

    /// Record a pass of the review `id` left without an outcome as interrupted,
    /// when no process holds the review's `.pass` lock, returning whether one
    /// was.
    fn settle(&self, id: ReviewId) -> Result<bool, String> {
        let dir = self.dir.join(id.as_str());
        let Some(_held) =
            lock::try_exclusive(&dir.join(PASS_FILE)).map_err(|error| format!("{dir}: {error}"))?
        else {
            return Ok(false);
        };

        let at = now();
        self.change(id, |review, _| Ok(!review.passes.interrupt(at).is_empty()))
    }

    /// The contents of the files the review `id` has recorded.
    pub fn blobs(&self, id: ReviewId) -> Blobs {
        Blobs {
            dir: self.dir.join(id.as_str()).join(BLOB_DIR),
        }
    }

    /// Apply `change` to the review `id`, holding the review's lock from
    /// reading it to writing the result.
    ///
    /// `change` gets the review as `review.json` holds it, once any pass left
    /// by a process that is gone is recorded as interrupted, and the blobs to
    /// write file contents to.
    /// Nothing is written when `change` fails, or leaves the review as it was;
    /// otherwise the review's revision goes up by one.
    /// A failure to read or write the review reaches the caller as an `E` made
    /// from its message.
    pub fn update<T, E: From<String>>(
        &self,
        id: ReviewId,
        change: impl FnOnce(&mut Review, &Blobs) -> Result<T, E>,
    ) -> Result<T, E> {
        self.read(id)?;
        self.change(id, change)
    }

    /// [`Store::update`], leaving a pass without an outcome as it is.
    fn change<T, E: From<String>>(
        &self,
        id: ReviewId,
        change: impl FnOnce(&mut Review, &Blobs) -> Result<T, E>,
    ) -> Result<T, E> {
        let dir = self.dir.join(id.as_str());
        let Some(_lock) =
            lock::exclusive(&dir.join(LOCK_FILE)).map_err(|error| format!("{dir}: {error}"))?
        else {
            return Err(format!("No review {id}.").into());
        };

        let mut review = read(&dir, id)?;
        let before = review.clone();
        let value = change(&mut review, &self.blobs(id))?;
        if review != before {
            review.revision += 1;
            write(&dir, &review)?;
        }

        Ok(value)
    }
}

/// Whether a `jp review serve` serves the store.
#[derive(Debug)]
pub enum Serving {
    /// None did, and this process holds the store's server lock until the
    /// [`File`] is dropped.
    Claimed(File),

    /// Another process does, at this address; empty while it has not written
    /// one yet.
    Running(String),
}

impl Store {
    /// Take the lock a store's one `jp review serve` holds, creating the store
    /// when there is none yet; [`Serving::Running`] when another process holds
    /// it.
    pub fn claim_server(&self) -> Result<Serving, String> {
        fs::create_dir_all(&self.dir).map_err(|error| format!("{}: {error}", self.dir))?;
        let held = lock::try_exclusive(&self.dir.join(SERVER_LOCK))
            .map_err(|error| format!("{}: {error}", self.dir))?;

        Ok(match held {
            Some(lock) => Serving::Claimed(lock),
            None => Serving::Running(self.server_address()),
        })
    }

    /// Write `url` as the address the store's server serves at.
    ///
    /// The caller holds the lock [`Store::claim_server`] took.
    pub fn serving_at(&self, url: &str) -> Result<(), String> {
        replace(
            &self.dir.join(format!("{SERVER_FILE}.tmp")),
            &self.dir.join(SERVER_FILE),
            format!("{url}\n").as_bytes(),
        )
    }

    /// The address of the `jp review serve` serving the store; `None` when none
    /// runs.
    ///
    /// The address is empty while the server has not written it yet.
    pub fn server(&self) -> Result<Option<String>, String> {
        if !self.dir.is_dir() {
            return Ok(None);
        }
        let free = lock::try_exclusive(&self.dir.join(SERVER_LOCK))
            .map_err(|error| format!("{}: {error}", self.dir))?;

        Ok(free.is_none().then(|| self.server_address()))
    }

    /// What [`SERVER_FILE`] holds; empty when it can't be read.
    fn server_address(&self) -> String {
        fs::read_to_string(self.dir.join(SERVER_FILE))
            .map_or_default(|address| address.trim().to_owned())
    }
}

/// A review the store holds, as it reads or with the reason it doesn't.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: ReviewId,
    pub review: Result<Review, String>,
}

/// The contents of a review's files, each in a file named by its SHA-256.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blobs {
    dir: Utf8PathBuf,
}

impl Blobs {
    /// Store `text`, returning the name of its blob.
    ///
    /// A blob stored already is left as it is.
    pub fn put(&self, text: &str) -> Result<String, String> {
        let name = hash(text);
        let path = self.dir.join(&name);
        if path.is_file() {
            return Ok(name);
        }

        fs::create_dir_all(&self.dir).map_err(|error| format!("{}: {error}", self.dir))?;
        replace(
            &self.dir.join(format!(".{name}.tmp")),
            &path,
            text.as_bytes(),
        )?;

        Ok(name)
    }

    /// The text the blob `name` holds.
    ///
    /// A name that isn't a SHA-256 hash is refused, and so is a blob that
    /// doesn't hold what its name says.
    pub fn get(&self, name: &str) -> Result<String, String> {
        if name.len() != HASH_DIGITS || !name.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(format!("`{name}` is not the name of a blob."));
        }

        let path = self.dir.join(name);
        let text = fs::read_to_string(&path).map_err(|error| format!("{path}: {error}"))?;
        if hash(&text) != name {
            return Err(format!(
                "{path} doesn't hold what its name says; the review is damaged."
            ));
        }

        Ok(text)
    }
}

/// The current time, to the second, as a review records it.
pub fn now() -> DateTime<Utc> {
    Utc::now().trunc_subsecs(0)
}

/// The SHA-256 of `text`, in lowercase hex: the name of its blob.
pub fn hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

/// What each file in `contents` is recorded as: the name of its blob, or `None`
/// when it has no contents.
pub fn manifest(contents: &BTreeMap<String, Option<String>>) -> Files {
    contents
        .iter()
        .map(|(path, text)| (path.clone(), text.as_deref().map(hash)))
        .collect()
}

/// Write a new review's blobs into its directory `dir`, then its `review.json`.
fn fill(dir: &Utf8Path, review: &Review, contents: &[&str]) -> Result<(), String> {
    let _lock = lock::exclusive(&dir.join(LOCK_FILE)).map_err(|error| format!("{dir}: {error}"))?;
    let blobs = Blobs {
        dir: dir.join(BLOB_DIR),
    };
    for text in contents {
        blobs.put(text)?;
    }

    write(dir, review)
}

/// The part of `review.json` read before the rest: the format, which decides
/// whether the rest can be read.
#[derive(Deserialize)]
struct Header {
    format: u32,
}

/// Read the review `id` from its directory `dir`.
fn read(dir: &Utf8Path, id: ReviewId) -> Result<Review, String> {
    let path = dir.join(REVIEW_FILE);
    let source = match fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) if error.kind() == io::ErrorKind::NotFound && dir.is_dir() => {
            return Err(format!("{id} has no {REVIEW_FILE}, in {dir}."));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(format!("No review {id}."));
        }
        Err(error) => return Err(format!("{path}: {error}")),
    };

    let header: Header =
        serde_json::from_str(&source).map_err(|error| format!("{path}: {error}"))?;
    if header.format > FORMAT {
        return Err(format!(
            "{path} is in format {}, and this jp-review reads format {FORMAT}; update jp-review.",
            header.format
        ));
    }

    serde_json::from_str(&source).map_err(|error| format!("{path}: {error}"))
}

/// Write `review` into its directory `dir`.
fn write(dir: &Utf8Path, review: &Review) -> Result<(), String> {
    let mut json = serde_json::to_string_pretty(review)
        .map_err(|error| format!("failed to serialize the review: {error}"))?;
    json.push('\n');

    replace(
        &dir.join(TEMP_FILE),
        &dir.join(REVIEW_FILE),
        json.as_bytes(),
    )
}

/// Write `bytes` to `temp`, flushed to disk, then rename it over `path`, so
/// `path` holds either all of its old contents or all of the new ones.
fn replace(temp: &Utf8Path, path: &Utf8Path, bytes: &[u8]) -> Result<(), String> {
    File::create(temp)
        .and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_all()
        })
        .and_then(|()| fs::rename(temp, path))
        .map_err(|error| format!("{path}: {error}"))
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
