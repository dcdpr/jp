use serde_json::{Map, Value};

use crate::{ast::Document, error::MergeError};

/// Recursively merge a [`serde_json::Value`] into a [`Document`].
///
/// Only keys present in `source` are touched. When both the target and source
/// have an object for the same key, the merge recurses. Otherwise the source
/// value overwrites the target.
///
/// Untouched content (comments, whitespace, key order) is preserved.
pub fn deep_merge(doc: &Document, source: &Value) -> Result<(), MergeError> {
    let root = doc.as_object().ok_or(MergeError::RootNotObject)?;
    let source = source.as_object().ok_or(MergeError::SourceNotObject)?;
    merge_into_object(&root, source);
    Ok(())
}

fn merge_into_object(target: &crate::ast::Object, source: &Map<String, Value>) {
    for (key, value) in source {
        if let (Some(target_obj), Value::Object(source_obj)) = (target.get_object(key), value) {
            merge_into_object(&target_obj, source_obj);
        } else {
            let raw = serde_json::to_string(value).expect("serde_json::to_string");
            target.set(key, &raw);
        }
    }
}

#[cfg(test)]
#[path = "merge_tests.rs"]
mod tests;
