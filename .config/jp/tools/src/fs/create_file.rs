use std::{
    fs::{self, File},
    io::Write as _,
    path::PathBuf,
};

use jp_tool::{AnswerType, Outcome, Question};
use serde_json::{Map, Value};

use crate::{
    Context,
    util::{ToolResult, error, fail},
};

pub(crate) async fn fs_create_file(
    ctx: Context,
    answers: &Map<String, Value>,
    path: String,
    content: Option<String>,
) -> ToolResult {
    if ctx.format_parameters {
        let lang = match path.split('.').next_back().unwrap_or_default() {
            "rs" => "rust",
            "js" => "javascript",
            "ts" => "typescript",
            "c" => "c",
            "cpp" => "cpp",
            "go" => "go",
            "php" => "php",
            "py" => "python",
            "rb" => "ruby",
            lang => lang,
        };

        let mut response = format!("Create file '{path}'");
        if let Some(content) = content {
            response.push_str(&format!(" with content:\n\n```{lang}\n{content}\n```"));
        }

        return Ok(response.into());
    }

    let p = PathBuf::from(&path);

    if p.is_absolute() {
        return error("Path must be relative.");
    }

    if p.iter().any(|c| c.len() > 30) {
        return error("Individual path components must be less than 30 characters long.");
    }

    if p.iter().count() > 20 {
        return error("Path must be less than 20 components long.");
    }

    let absolute_path = ctx.root.join(path.trim_start_matches('/'));
    if absolute_path.is_dir() {
        return error("Path is an existing directory.");
    }

    if absolute_path.exists() {
        match answers.get("overwrite_file").and_then(Value::as_bool) {
            Some(true) => {}
            Some(false) => {
                return error("Path points to existing file");
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
        return fail("Path has no parent");
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
