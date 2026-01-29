use camino::{Utf8Path, Utf8PathBuf};

use crate::{
    to_simple_xml_with_root,
    util::{
        ToolResult,
        runner::{DuctProcessRunner, ProcessRunner},
    },
};

pub(crate) async fn git_commit(root: Utf8PathBuf, message: String) -> ToolResult {
    git_commit_impl(&root, &message, &DuctProcessRunner)
}

fn git_commit_impl<R: ProcessRunner>(root: &Utf8Path, message: &str, runner: &R) -> ToolResult {
    let output = runner.run("git", &["commit", "--signoff", "--message", message], root)?;

    to_simple_xml_with_root(&output, "git_commit").map(Into::into)
}

#[cfg(test)]
mod tests {
    use camino_tempfile::tempdir;

    use super::*;
    use crate::util::runner::MockProcessRunner;

    #[test]
    fn test_git_commit_success() {
        let dir = tempdir().unwrap();

        let mock_output = "[main abc1234] test commit\n 1 file changed, 1 insertion(+)\n";
        let runner = MockProcessRunner::success(mock_output);

        let content = git_commit_impl(dir.path(), "test commit", &runner)
            .unwrap()
            .into_content()
            .unwrap();

        assert_eq!(content, indoc::indoc! {"
          <git_commit>
              <output>
                  [main abc1234] test commit
                   1 file changed, 1 insertion(+)
              </output>
          </git_commit>"
        });
    }

    #[test]
    fn test_git_commit_failure() {
        let dir = tempdir().unwrap();

        let runner = MockProcessRunner::error("nothing to commit");

        let result = git_commit_impl(dir.path(), "test commit", &runner)
            .unwrap()
            .into_content()
            .unwrap();

        assert_eq!(result, indoc::indoc! {"
          <git_commit>
              <error>nothing to commit</error>
              <status>1</status>
          </git_commit>"
        });
    }
}
