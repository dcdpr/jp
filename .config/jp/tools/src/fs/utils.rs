use std::{path::Path, process::Command};

use crate::Error;

pub fn is_file_dirty(root: &Path, file: &Path) -> Result<bool, Error> {
    let output = Command::new("git")
        .args([
            "status",
            "--porcelain",
            "--",
            file.to_str().unwrap_or_default(),
        ])
        .current_dir(root)
        .output()?;

    let error = str::from_utf8(&output.stderr).unwrap_or_default();
    if error.contains("fatal: not a git repository") {
        return Ok(false);
    }

    if !output.status.success() {
        return Err(format!(
            "Git command failed ({}): {}",
            output.status.code().unwrap_or_default(),
            error,
        )
        .into());
    }

    String::from_utf8(output.stdout)
        // The second column is the non-staged status indicator.
        .map(|v| v.chars().nth(1) == Some('M'))
        .map_err(Into::into)
}
