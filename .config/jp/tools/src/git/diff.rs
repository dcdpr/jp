use camino::{Utf8Path, Utf8PathBuf};

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
) -> ToolResult {
    let mut args = vec!["diff-index"];
    if cached {
        args.push("--cached");
    }
    args.extend_from_slice(&["-p", "HEAD"]);

    let path_refs: Vec<&str> = paths.iter().map(String::as_str).collect();
    args.extend(path_refs);

    let output = runner.run("git", &args, root)?;

    to_simple_xml_with_root(&output, "git_diff").map(Into::into)
}

pub(crate) async fn git_diff(
    root: Utf8PathBuf,
    paths: OneOrMany<String>,
    cached: Option<bool>,
) -> ToolResult {
    let cached = cached.unwrap_or(false);
    git_diff_impl(&root, &paths, cached, &DuctProcessRunner)
}

#[cfg(test)]
mod tests {
    use camino_tempfile::tempdir;

    use super::*;
    use crate::util::runner::MockProcessRunner;

    #[test]
    fn test_git_diff_success() {
        let dir = tempdir().unwrap();

        let diff = indoc::indoc! {"
            diff --git a/test.rs b/test.rs
            index abc123..def456 100644
            --- a/test.rs
            +++ b/test.rs
            @@ -1 +1 @@
            -old line
            +new line"
        };

        let runner = MockProcessRunner::success(diff);
        let content = git_diff_impl(dir.path(), &["test.rs".to_string()], false, &runner)
            .unwrap()
            .into_content()
            .unwrap();

        assert_eq!(content, indoc::indoc! {"
            <git_diff>
                <output>
                    diff --git a/test.rs b/test.rs
                    index abc123..def456 100644
                    --- a/test.rs
                    +++ b/test.rs
                    @@ -1 +1 @@
                    -old line
                    +new line
                </output>
            </git_diff>"});
    }

    #[test]
    fn test_git_diff_cached() {
        let dir = tempdir().unwrap();

        let runner = MockProcessRunner::success("no changes");

        let result = git_diff_impl(dir.path(), &["test.rs".to_string()], true, &runner)
            .unwrap()
            .into_content()
            .unwrap();

        assert_eq!(result, indoc::indoc! {"
            <git_diff>
                <output>no changes</output>
            </git_diff>"
        });
    }
}
