use std::{
    fs,
    io::{BufWriter, Write as _},
    path::Path,
};

use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::error::Result;

pub fn merge_files<T: DeserializeOwned>(
    base: impl AsRef<Path>,
    overlay: impl AsRef<Path>,
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

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let file = fs::File::open(path)?;
    serde_json::from_reader(file).map_err(Into::into)
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let file = fs::File::create(path)?;
    let mut buf = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut buf, value)?;
    buf.write_all(b"\n")?;
    buf.flush()?;

    Ok(())
}
