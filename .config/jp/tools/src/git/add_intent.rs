use camino::Utf8Path;
use serde_json::{Map, Value};

use crate::util::{
    OneOrMany, ToolResult,
    runner::{DuctProcessRunner, ProcessRunner},
};

pub(crate) async fn git_add_intent(
    root: &Utf8Path,
    paths: OneOrMany<String>,
    options: &Map<String, Value>,
) -> ToolResult {
    let env = super::env_from_options(options);
    git_add_intent_impl(root, &paths, &DuctProcessRunner, &env)
}

fn git_add_intent_impl<R: ProcessRunner>(
    root: &Utf8Path,
    paths: &[String],
    runner: &R,
    env: &[(&str, &str)],
) -> ToolResult {
    for path in paths {
        let output =
            runner.run_with_env("git", &["add", "--intent-to-add", "--", path], root, env)?;

        if !output.success() {
            return Err(
                format!("Failed to intent-to-add for '{}': {}", path, output.stderr).into(),
            );
        }
    }

    let count = paths.len();
    let noun = if count == 1 { "file" } else { "files" };
    Ok(format!(
        "Marked {count} {noun} as intent-to-add. They are now visible to `git_list_patches`."
    )
    .into())
}

#[cfg(test)]
#[path = "add_intent_tests.rs"]
mod tests;
