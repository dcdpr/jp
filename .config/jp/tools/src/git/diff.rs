use camino::{Utf8Path, Utf8PathBuf};
use serde_json::{Map, Value};

use crate::{
    to_simple_xml_with_root,
    util::{
        OneOrMany, ToolResult,
        runner::{DuctProcessRunner, ProcessRunner},
    },
};

/// Which changes to include in the diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiffStatus {
    /// All changes from HEAD (staged + unstaged combined).
    All,
    /// Staged changes only (HEAD vs index).
    Staged,
    /// Unstaged changes only (index vs working tree).
    Unstaged,
}

impl DiffStatus {
    fn parse(s: &str) -> Result<Self, String> {
        match s {
            "all" => Ok(Self::All),
            "staged" => Ok(Self::Staged),
            "unstaged" => Ok(Self::Unstaged),
            other => Err(format!(
                "Invalid diff status '{other}', expected 'all', 'staged', or 'unstaged'"
            )),
        }
    }
}

fn git_diff_impl<R: ProcessRunner>(
    root: &Utf8Path,
    paths: &[String],
    status: DiffStatus,
    runner: &R,
    env: &[(&str, &str)],
) -> ToolResult {
    let mut args = match status {
        DiffStatus::All => vec!["diff-index", "--ita-invisible-in-index", "-p", "HEAD"],
        DiffStatus::Staged => vec!["diff-index", "--cached", "--ita-invisible-in-index", "-p", "HEAD"],
        DiffStatus::Unstaged => vec!["diff-files", "-p"],
    };

    let path_refs: Vec<&str> = paths.iter().map(String::as_str).collect();
    args.extend(path_refs);

    let output = runner.run_with_env("git", &args, root, env)?;

    to_simple_xml_with_root(&output, "git_diff").map(Into::into)
}

pub(crate) async fn git_diff(
    root: Utf8PathBuf,
    paths: Option<OneOrMany<String>>,
    status: String,
    options: &Map<String, Value>,
) -> ToolResult {
    let status = DiffStatus::parse(&status)?;
    let paths = paths.unwrap_or_default();
    let env = super::env_from_options(options);
    git_diff_impl(&root, &paths, status, &DuctProcessRunner, &env)
}

#[cfg(test)]
#[path = "diff_tests.rs"]
mod tests;
