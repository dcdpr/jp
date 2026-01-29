use camino::Utf8Path;

use crate::{
    Error,
    util::runner::{DuctProcessRunner, ProcessOutput, ProcessRunner},
};

pub fn is_file_dirty(root: &Utf8Path, file: &Utf8Path) -> Result<bool, Error> {
    is_file_dirty_impl(root, file, &DuctProcessRunner)
}

fn is_file_dirty_impl<R: ProcessRunner>(
    root: &Utf8Path,
    file: &Utf8Path,
    runner: &R,
) -> Result<bool, Error> {
    let ProcessOutput {
        stdout,
        stderr,
        status,
    } = runner.run("git", &["status", "--porcelain", "--", file.as_str()], root)?;

    if stderr.contains("fatal: not a git repository") {
        return Ok(false);
    }

    if !status.is_success() {
        return Err(format!("Git command failed ({status}): {stderr}").into());
    }

    // The second column is the non-staged status indicator.
    Ok(stdout.chars().nth(1) == Some('M'))
}

#[cfg(test)]
mod tests {
    use camino::Utf8PathBuf;
    use camino_tempfile::tempdir;

    use super::*;
    use crate::util::runner::MockProcessRunner;

    #[test]
    fn test_is_file_dirty_modified() {
        let dir = tempdir().unwrap();
        let file = Utf8PathBuf::from("test.rs");

        // Second column 'M' indicates modified
        let runner = MockProcessRunner::success(" M test.rs\n");

        let result = is_file_dirty_impl(dir.path(), &file, &runner).unwrap();

        assert!(result);
    }

    #[test]
    fn test_is_file_dirty_not_modified() {
        let dir = tempdir().unwrap();
        let file = Utf8PathBuf::from("test.rs");

        // No output means no changes
        let runner = MockProcessRunner::success("");

        let result = is_file_dirty_impl(dir.path(), &file, &runner).unwrap();

        assert!(!result);
    }

    #[test]
    fn test_is_file_dirty_not_a_git_repo() {
        let dir = tempdir().unwrap();
        let file = Utf8PathBuf::from("test.rs");

        let runner = MockProcessRunner::error("fatal: not a git repository");

        let result = is_file_dirty_impl(dir.path(), &file, &runner).unwrap();

        // Should return false when not in a git repo
        assert!(!result);
    }
}
