use std::{path::PathBuf, process::Command};

use serde::Serialize;

use crate::{Error, to_xml, util::OneOrMany};

#[derive(Serialize)]
struct CommandResult {
    status: i32,
    stdout: String,
    stderr: String,
}

pub(crate) async fn git_diff(
    root: PathBuf,
    paths: OneOrMany<String>,
    cached: Option<bool>,
) -> std::result::Result<String, Error> {
    let cached = cached.unwrap_or(false);
    let output = Command::new("git")
        .arg("diff-index")
        .args(if cached { vec!["--cached"] } else { vec![] })
        .args(["-p", "HEAD"])
        .args(paths.to_vec())
        .current_dir(root)
        .output()?;

    to_xml(CommandResult {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8(output.stdout).unwrap_or_default(),
        stderr: String::from_utf8(output.stderr).unwrap_or_default(),
    })
}
