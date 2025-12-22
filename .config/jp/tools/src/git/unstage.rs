use std::{path::PathBuf, process::Command};

use serde::Serialize;

use crate::{Error, to_xml_with_root, util::OneOrMany};

#[derive(Serialize)]
struct CommandResult {
    status: i32,
    stdout: String,
    stderr: String,
}

pub(crate) async fn git_unstage(
    root: PathBuf,
    paths: OneOrMany<String>,
) -> std::result::Result<String, Error> {
    let mut outputs = vec![];
    for path in paths {
        let output = Command::new("git")
            .args(["restore", "--staged", "--", &path])
            .current_dir(&root)
            .output()?;

        outputs.push(output);
    }

    let results = outputs
        .into_iter()
        .map(|output| CommandResult {
            status: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8(output.stdout).unwrap_or_default(),
            stderr: String::from_utf8(output.stderr).unwrap_or_default(),
        })
        .collect::<Vec<_>>();

    to_xml_with_root(&results, "results")
}
