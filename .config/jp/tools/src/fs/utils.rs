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

    if !output.status.success() {
        return Err(format!(
            "Git command failed ({}): {}",
            output.status.code().unwrap_or_default(),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    Ok(!output.stdout.is_empty())
}
