use std::fs;

use camino::Utf8PathBuf;
use jp_tool::{AnswerType, Outcome, Question};
use serde_json::{Map, Value};

use super::utils::is_file_dirty;
use crate::Error;

pub(crate) async fn fs_delete_file(
    root: Utf8PathBuf,
    answers: &Map<String, Value>,
    path: String,
) -> std::result::Result<Outcome, Error> {
    let p = Utf8PathBuf::from(&path);

    if p.is_absolute() {
        return Err("Path must be relative.".into());
    }

    let absolute_path = root.join(path.trim_start_matches('/'));
    if absolute_path.is_dir() {
        return Err(
            "Path is a directory. You can only delete files. Empty directories are automatically \
             deleted."
                .into(),
        );
    }

    if !absolute_path.is_file() {
        return Err("Path points to non-existing file".into());
    }

    let Some(parent) = absolute_path.parent() else {
        return Err("Path has no parent".into());
    };

    if is_file_dirty(&root, &p)? {
        match answers.get("delete_dirty_file").and_then(Value::as_bool) {
            Some(true) => {}
            Some(false) => {
                return Err("File has uncommitted changes. Please stage or discard first.".into());
            }
            None => {
                return Ok(Outcome::NeedsInput {
                    question: Question {
                        id: "delete_dirty_file".to_string(),
                        text: format!("File '{path}' has uncommitted changes. Delete anyway?"),
                        answer_type: AnswerType::Boolean,
                        default: Some(Value::Bool(false)),
                    },
                });
            }
        }
    }

    fs::remove_file(&absolute_path)?;
    let mut msg = "File deleted.".to_owned();

    if parent.read_dir()?.next().is_none() {
        fs::remove_dir(parent)?;
        msg.push_str(" Removed empty parent directory.");
    }

    Ok(msg.into())
}
