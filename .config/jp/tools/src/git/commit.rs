use std::{path::PathBuf, process::Command};

use serde::Serialize;

use crate::{Error, to_xml};

#[derive(Serialize)]
struct CommandResult {
    status: i32,
    stdout: String,
    stderr: String,
}

pub(crate) async fn git_commit(
    root: PathBuf,
    message: String,
) -> std::result::Result<String, Error> {
    let output = Command::new("git")
        .args(["commit", "--signoff", "--message", &message])
        .current_dir(root)
        .output()?;

    to_xml(CommandResult {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8(output.stdout).unwrap_or_default(),
        stderr: String::from_utf8(output.stderr).unwrap_or_default(),
    })
}
