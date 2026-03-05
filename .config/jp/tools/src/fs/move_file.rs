use std::fs;

use camino::Utf8PathBuf;
use jp_tool::{AnswerType, Outcome, Question};
use serde_json::{Map, Value};

use super::utils::is_file_dirty;
use crate::Error;

pub(crate) async fn fs_move_file(
    root: Utf8PathBuf,
    answers: &Map<String, Value>,
    source: String,
    target: String,
) -> std::result::Result<Outcome, Error> {
    let src = Utf8PathBuf::from(&source);
    let dst = Utf8PathBuf::from(&target);

    if src.is_absolute() {
        return Err("Source path must be relative.".into());
    }

    if dst.is_absolute() {
        return Err("Destination path must be relative.".into());
    }

    let abs_src = root.join(source.trim_start_matches('/'));
    let abs_dst = root.join(target.trim_start_matches('/'));

    if !abs_src.is_file() {
        return Err(format!("Source path '{source}' does not exist or is not a file.").into());
    }

    if abs_dst.is_dir() {
        return Err(format!("Destination path '{target}' is an existing directory.").into());
    }

    if abs_dst.is_file() {
        match answers.get("overwrite_file").and_then(Value::as_bool) {
            Some(true) => {}
            Some(false) => {
                return Err(format!("Destination '{target}' already exists.").into());
            }
            None => {
                return Ok(Outcome::NeedsInput {
                    question: Question {
                        id: "overwrite_file".to_string(),
                        text: format!("Destination '{target}' exists. Overwrite?"),
                        answer_type: AnswerType::Boolean,
                        default: Some(Value::Bool(false)),
                    },
                });
            }
        }
    }

    if is_file_dirty(&root, &src)? {
        match answers.get("move_dirty_file").and_then(Value::as_bool) {
            Some(true) => {}
            Some(false) => {
                return Err(
                    "Source file has uncommitted changes. Please stage or discard first.".into(),
                );
            }
            None => {
                return Ok(Outcome::NeedsInput {
                    question: Question {
                        id: "move_dirty_file".to_string(),
                        text: format!("File '{source}' has uncommitted changes. Move anyway?"),
                        answer_type: AnswerType::Boolean,
                        default: Some(Value::Bool(false)),
                    },
                });
            }
        }
    }

    // Ensure destination parent directories exist.
    if let Some(parent) = abs_dst.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::rename(&abs_src, &abs_dst)?;
    let mut msg = format!("Moved '{source}' to '{target}'.");

    // Clean up empty parent directory of source.
    if let Some(parent) = abs_src.parent()
        && parent != root
        && parent.read_dir()?.next().is_none()
    {
        fs::remove_dir(parent)?;
        msg.push_str(" Removed empty parent directory.");
    }

    Ok(msg.into())
}
