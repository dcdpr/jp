use camino::Utf8Path;
use jp_process::{ProcessRunner, SystemProcessRunner};
use serde_json::{Map, Value};

use super::Reported;
use crate::{
    to_xml_with_root,
    util::{OneOrMany, ToolResult},
};

pub(crate) async fn git_unstage(
    root: &Utf8Path,
    paths: OneOrMany<String>,
    options: &Map<String, Value>,
) -> ToolResult {
    let env = super::env_from_options(options);
    git_unstage_impl(root, &paths, &SystemProcessRunner, &env)
}

fn git_unstage_impl<R: ProcessRunner>(
    root: &Utf8Path,
    paths: &[String],
    runner: &R,
    env: &[(&str, &str)],
) -> ToolResult {
    let mut results = vec![];

    for path in paths {
        let output = runner.run_with_env("git", &["restore", "--staged", "--", path], root, env)?;

        results.push(output);
    }

    if results.iter().any(|v| !v.success()) {
        let reported: Vec<Reported<'_>> = results.iter().map(Reported::from).collect();
        return to_xml_with_root(&reported, "failed_to_unstage").map(Into::into);
    }

    Ok("Changes unstaged.".into())
}

#[cfg(test)]
#[path = "unstage_tests.rs"]
mod tests;
