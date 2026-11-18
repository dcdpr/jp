//! Author resolution for documents created from the CLI.
//!
//! Resolves the author name from JP's `user.name`, git's `user.name`, or the OS
//! username (`USER` / `USERNAME`), and appends git's `user.email` to a bare
//! name as `Jane Doe <jane@example.com>`.

use std::{env, process::Command};

use camino::Utf8Path;
use serde_json::Value;

/// Resolve the author identity for the workspace at `root`.
///
/// `config` is the resolved host configuration as JSON.
/// Git commands run in `root`, so a repository's local git config takes
/// precedence over the global git config.
///
/// # Errors
///
/// Returns an error if no source provides an author name.
pub fn resolve(root: &Utf8Path, config: &Value) -> Result<String, String> {
    let configured = config
        .pointer("/user/name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty());
    if let Some(name) = configured {
        return Ok(with_email(root, name.to_owned()));
    }

    if let Some(name) = git_config(root, "user.name") {
        return Ok(with_email(root, name));
    }

    if let Ok(user) = env::var("USER").or_else(|_| env::var("USERNAME"))
        && !user.trim().is_empty()
    {
        return Ok(with_email(root, user.trim().to_owned()));
    }

    Err(
        "Don't know who you are. Set `user.name` in your JP config or `user.name` in your git \
         config."
            .to_owned(),
    )
}

/// Append git's email to a bare name.
fn with_email(root: &Utf8Path, name: String) -> String {
    if name.contains('<') {
        return name;
    }

    match git_config(root, "user.email") {
        Some(email) => format!("{name} <{email}>"),
        None => name,
    }
}

fn git_config(root: &Utf8Path, key: &str) -> Option<String> {
    Command::new("git")
        .current_dir(root)
        .args(["config", "--get", key])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
#[path = "author_tests.rs"]
mod tests;
