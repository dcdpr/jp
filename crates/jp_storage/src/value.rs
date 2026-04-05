use std::{
    fs,
    io::{BufReader, BufWriter, Write as _},
};

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::error::Result;

/// The temp-file suffix used by [`write_json`] for atomic writes.
pub const TMP_SUFFIX: &str = ".tmp";

pub fn merge_files<T: DeserializeOwned>(
    base: impl AsRef<Utf8Path>,
    overlay: impl AsRef<Utf8Path>,
) -> Result<T> {
    let base = base.as_ref();
    let overlay = overlay.as_ref();

    if !overlay.is_file() {
        return read_json(base);
    }

    let base: Value = read_json(base)?;
    let overlay: Value = read_json(overlay)?;

    deep_merge(base, overlay)
}

/// Merge two JSON values, recursively, returning the final deserialized value
/// of type `T`.
pub fn deep_merge<T: DeserializeOwned>(mut base: Value, overlay: Value) -> Result<T> {
    deep_merge_values(&mut base, overlay);
    serde_json::from_value(base).map_err(Into::into)
}

fn deep_merge_values(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Object(a), Value::Object(b)) => {
            for (k, v) in b {
                deep_merge_values(a.entry(k).or_insert(Value::Null), v);
            }
        }
        // anything that isn’t both objects ⇒ overlay wins wholesale
        (base_slot, v) => *base_slot = v,
    }
}

pub fn read_json<T: DeserializeOwned>(path: &Utf8Path) -> Result<T> {
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);
    serde_json::from_reader(reader).map_err(Into::into)
}

/// Write a JSON value to a file atomically.
///
/// Writes to a temporary sibling file (`{path}.tmp`), flushes, then renames
/// over the target. If anything fails before the rename, the original file is
/// left untouched and the temp file is cleaned up on a best-effort basis.
pub fn write_json<T: Serialize>(path: &Utf8Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let tmp_path = tmp_path_for(path);

    // Write to the temp file. On failure, clean up and return the error.
    if let Err(error) = write_json_inner(&tmp_path, value) {
        let _err = fs::remove_file(&tmp_path);
        return Err(error);
    }

    // Atomic rename.
    if let Err(error) = fs::rename(&tmp_path, path) {
        let _err = fs::remove_file(&tmp_path);
        return Err(error.into());
    }

    Ok(())
}

fn write_json_inner<T: Serialize>(path: &Utf8Path, value: &T) -> Result<()> {
    let file = fs::File::create(path)?;
    let mut buf = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut buf, value)?;
    buf.write_all(b"\n")?;
    buf.flush()?;

    Ok(())
}

/// Build the temporary sibling path for a given target path.
fn tmp_path_for(path: &Utf8Path) -> Utf8PathBuf {
    let mut s = path.as_str().to_owned();
    s.push_str(TMP_SUFFIX);
    Utf8PathBuf::from(s)
}

/// Remove orphaned `.tmp` files from a directory.
///
/// These can be left behind if the process crashes after writing the temp file
/// but before the rename completes. Safe to call on any directory; non-`.tmp`
/// entries are ignored.
pub fn cleanup_tmp_files(dir: &Utf8Path) {
    let Ok(entries) = dir.read_dir_utf8() else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.as_str().ends_with(TMP_SUFFIX) && path.is_file() {
            tracing::trace!(%path, "Cleaning up orphaned .tmp file.");
            let _err = fs::remove_file(path);
        }
    }
}

#[cfg(test)]
#[path = "value_tests.rs"]
mod tests;
