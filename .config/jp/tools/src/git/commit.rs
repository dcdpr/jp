use camino::{Utf8Path, Utf8PathBuf};
use serde_json::{Map, Value};

use crate::{
    to_simple_xml_with_root,
    util::{
        ToolResult,
        runner::{DuctProcessRunner, ProcessRunner},
    },
};

pub(crate) async fn git_commit(
    root: Utf8PathBuf,
    message: String,
    options: &Map<String, Value>,
) -> ToolResult {
    let env = super::env_from_options(options);
    git_commit_impl(&root, &message, &DuctProcessRunner, &env)
}

fn git_commit_impl<R: ProcessRunner>(
    root: &Utf8Path,
    message: &str,
    runner: &R,
    env: &[(&str, &str)],
) -> ToolResult {
    let output = runner.run_with_env(
        "git",
        &["commit", "--signoff", "--message", message],
        root,
        env,
    )?;

    to_simple_xml_with_root(&output, "git_commit").map(Into::into)
}

#[cfg(test)]
#[path = "commit_tests.rs"]
mod tests;
