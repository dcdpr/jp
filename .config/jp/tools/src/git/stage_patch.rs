use std::path::PathBuf;

use duct::cmd;
use jp_tool::{AnswerType, Context, Outcome, Question};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::{
    to_xml_with_root,
    util::{OneOrMany, ToolResult},
};

#[derive(Serialize)]
struct CommandResult {
    status: i32,
    stdout: String,
    stderr: String,
}

pub(crate) async fn git_stage_patch(
    ctx: Context,
    answers: &Map<String, Value>,
    path: PathBuf,
    patch_ids: OneOrMany<usize>,
) -> ToolResult {
    // git ls-files .config/jp/tools/src/git/stage_patch.rs
    let ls = cmd!("git", "ls-files", &path)
        .unchecked()
        .stdout_capture()
        .run()?;
    if !ls.status.success() {
        return Err(format!("Failed to list staged changes: {ls:?}").into());
    }

    let patch = if ls.stdout.is_empty() {
        // Untracked files.
        cmd!(
            "git",
            "diff",
            "--no-index",
            "--minimal",
            "--unified=0",
            "--",
            "/dev/null",
            &path
        )
    } else {
        // Tracked files.
        cmd!(
            "git",
            "diff-files",
            "-p",
            "--minimal",
            "--unified=0",
            "--",
            &path
        )
    }
    .dir(&ctx.root)
    .unchecked()
    .stdout_capture()
    .stderr_capture()
    .run()
    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
    .and_then(|out| {
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        if !out.status.success() {
            return Err(format!("Failed to get patch for `{}`: {stderr}", path.display()).into());
        }

        let mut hunks = vec![];
        for (id, hunk) in stdout.split("\n@@ ").skip(1).enumerate() {
            if !patch_ids.contains(&id) {
                continue;
            }

            hunks.push(format!("@@ {hunk}"));
        }

        if hunks.is_empty() {
            return Err(format!("Failed to find patch for `{}`: {stdout}", path.display()).into());
        }

        Ok(hunks.join("\n"))
    })?;

    let path = path.display().to_string();
    let patch = indoc::formatdoc! {"
            diff --git a/{path} b/{path}
            --- a/{path}
            +++ b/{path}
            {patch}
        "};

    if ctx.action.is_format_arguments() {
        return Ok(patch.into());
    }

    match answers.get("stage_changes").and_then(Value::as_bool) {
        Some(true) => {}
        Some(false) => {
            return Ok("Changes not staged.".into());
        }
        None => {
            return Ok(Outcome::NeedsInput {
                question: Question {
                    id: "stage_changes".to_string(),
                    text: format!("Do you want to stage the following patch?\n\n{patch}"),
                    answer_type: AnswerType::Boolean,
                    default: Some(Value::Bool(true)),
                },
            });
        }
    }

    let output = cmd!("echo", &patch)
        .pipe(cmd!("git", "apply", "--cached", "--unidiff-zero", "-"))
        .dir(&ctx.root)
        .unchecked()
        .stdout_capture()
        .stderr_capture()
        .run()?;

    let results = CommandResult {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8(output.stdout).unwrap_or_default(),
        stderr: format!(
            "{}\n\n{}",
            String::from_utf8(output.stderr).unwrap_or_default(),
            patch
        ),
    };

    to_xml_with_root(&results, "results").map(Into::into)
}
