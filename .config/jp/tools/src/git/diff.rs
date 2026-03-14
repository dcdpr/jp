use camino::{Utf8Path, Utf8PathBuf};
use serde_json::{Map, Value};

use crate::{
    to_simple_xml_with_root,
    util::{
        OneOrMany, ToolResult,
        runner::{DuctProcessRunner, ProcessRunner},
    },
};

fn git_diff_impl<R: ProcessRunner>(
    root: &Utf8Path,
    paths: &[String],
    cached: bool,
    runner: &R,
    env: &[(&str, &str)],
) -> ToolResult {
    let mut args = vec!["diff-index"];
    if cached {
        args.push("--cached");
    }
    args.extend_from_slice(&["-p", "HEAD"]);

    let path_refs: Vec<&str> = paths.iter().map(String::as_str).collect();
    args.extend(path_refs);

    let output = runner.run_with_env("git", &args, root, env)?;

    to_simple_xml_with_root(&output, "git_diff").map(Into::into)
}

pub(crate) async fn git_diff(
    root: Utf8PathBuf,
    paths: OneOrMany<String>,
    cached: Option<bool>,
    options: &Map<String, Value>,
) -> ToolResult {
    let cached = cached.unwrap_or(false);
    let env = super::env_from_options(options);
    git_diff_impl(&root, &paths, cached, &DuctProcessRunner, &env)
}

#[cfg(test)]
#[path = "diff_tests.rs"]
mod tests;
