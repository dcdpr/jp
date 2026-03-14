use camino::Utf8Path;

use crate::{
    Error,
    util::runner::{DuctProcessRunner, ProcessOutput, ProcessRunner},
};

pub fn is_file_dirty(root: &Utf8Path, file: &Utf8Path) -> Result<bool, Error> {
    is_file_dirty_impl(root, file, &DuctProcessRunner)
}

pub(super) fn is_file_dirty_impl<R: ProcessRunner>(
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
#[path = "utils_tests.rs"]
mod tests;
