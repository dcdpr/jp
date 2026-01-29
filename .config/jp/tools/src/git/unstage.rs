use camino::Utf8Path;

use crate::{
    to_xml_with_root,
    util::{
        OneOrMany, ToolResult,
        runner::{DuctProcessRunner, ProcessRunner},
    },
};

pub(crate) async fn git_unstage(root: &Utf8Path, paths: OneOrMany<String>) -> ToolResult {
    git_unstage_impl(root, &paths, &DuctProcessRunner)
}

fn git_unstage_impl<R: ProcessRunner>(root: &Utf8Path, paths: &[String], runner: &R) -> ToolResult {
    let mut results = vec![];

    for path in paths {
        let output = runner.run("git", &["restore", "--staged", "--", path], root)?;

        results.push(output);
    }

    if results.iter().any(|v| !v.success()) {
        return to_xml_with_root(&results, "failed_to_unstage").map(Into::into);
    }

    Ok("Changes unstaged.".into())
}

#[cfg(test)]
mod tests {
    use camino_tempfile::tempdir;

    use super::*;
    use crate::util::runner::MockProcessRunner;

    #[test]
    fn test_git_unstage_single_file() {
        let dir = tempdir().unwrap();

        let runner = MockProcessRunner::success("");

        let result = git_unstage_impl(dir.path(), &["test.rs".to_string()], &runner)
            .unwrap()
            .into_content()
            .unwrap();

        assert_eq!(result, "Changes unstaged.");
    }

    #[test]
    fn test_git_unstage_multiple_files() {
        let dir = tempdir().unwrap();

        let runner = MockProcessRunner::builder()
            .expect("git")
            .args(&["restore", "--staged", "--", "file1.rs"])
            .returns_success("")
            .expect("git")
            .args(&["restore", "--staged", "--", "file2.rs"])
            .returns_success("");

        let result = git_unstage_impl(
            dir.path(),
            &["file1.rs".to_string(), "file2.rs".to_string()],
            &runner,
        )
        .unwrap()
        .into_content()
        .unwrap();

        assert_eq!(result, "Changes unstaged.");
    }
}
