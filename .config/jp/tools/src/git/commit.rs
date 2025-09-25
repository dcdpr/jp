use std::{path::PathBuf, process::Command};

use crate::Error;

pub(crate) async fn git_commit(
    root: PathBuf,
    message: String,
) -> std::result::Result<String, Error> {
    let output = Command::new("git")
        .args(["commit", "--quiet", "--signoff", "--message", &message])
        .current_dir(root)
        .output()?;

    let error = str::from_utf8(&output.stderr).unwrap_or_default();
    if error.contains("fatal: not a git repository") {
        return Err("Not a git repository.".into());
    }

    if !output.status.success() {
        return Err(format!(
            "Git command failed ({}): {}",
            output.status.code().unwrap_or_default(),
            error,
        )
        .into());
    }

    let out = String::from_utf8(output.stdout).unwrap_or_default();

    Ok(indoc::formatdoc! {"
        Commit successful:

        ```
        {out}
        ```
    "})
}
