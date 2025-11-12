use std::{
    fs::{self, File},
    io::Write as _,
    path::PathBuf,
};

use jp_tool::{AnswerType, Outcome, Question};
use serde_json::{Map, Value};

use crate::Error;

pub(crate) async fn fs_create_file(
    root: PathBuf,
    answers: &Map<String, Value>,
    path: String,
    content: Option<String>,
) -> std::result::Result<Outcome, Error> {
    let p = PathBuf::from(&path);

    if p.is_absolute() {
        return Err("Path must be relative.".into());
    }

    if p.iter().any(|c| c.len() > 30) {
        return Err("Individual path components must be less than 30 characters long.".into());
    }

    if p.iter().count() > 20 {
        return Err("Path must be less than 20 components long.".into());
    }

    let absolute_path = root.join(path.trim_start_matches('/'));
    if absolute_path.is_dir() {
        return Err("Path is an existing directory.".into());
    }

    if absolute_path.exists() {
        match answers.get("overwrite_file").and_then(Value::as_bool) {
            Some(true) => {}
            Some(false) => {
                return Err("Path points to existing file".into());
            }
            None => {
                return Ok(Outcome::NeedsInput {
                    question: Question {
                        id: "overwrite_file".to_string(),
                        text: format!("File '{path}' exists. Overwrite?"),
                        answer_type: AnswerType::Boolean,
                        default: Some(Value::Bool(false)),
                    },
                });
            }
        }
    }

    let Some(parent) = absolute_path.parent() else {
        return Err("Path has no parent".into());
    };

    fs::create_dir_all(parent)?;
    let mut file = File::options()
        .write(true)
        .truncate(true)
        .create(true)
        .open(&absolute_path)?;

    if let Some(content) = content {
        file.write_all(content.as_bytes())?;
    }

    Ok(format!(
        "File '{}' created. File size: {}",
        path,
        file.metadata()?.len()
    )
    .into())
}
