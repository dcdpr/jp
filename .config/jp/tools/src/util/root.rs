//! Resolution of the `root` tool option: which directory a tool's subprocess
//! runs in.
//!
//! [`resolve_root`] is the entry point.
//! It turns a configured value into an absolute directory, under the same
//! confinement every other tool path gets, and refuses a target that would make
//! the tool's program silently operate on an enclosing project instead.

use camino::{Utf8Path, Utf8PathBuf};
use jp_tool::{AccessPolicy, Capability, Outcome};
use serde_json::{Map, Value};

use crate::{
    fs::utils::{authorize, resolve_workspace_path},
    util::ToolResult,
};

/// An entry that must sit at a resolved root, and what the program does when it
/// is missing.
pub struct Marker {
    /// Entry that must exist directly under the root.
    entry: &'static str,

    /// Where the program goes instead when the entry is absent.
    ///
    /// Appended to the refusal, so the reader learns what the check prevented
    /// rather than only that it fired.
    upward_search: &'static str,
}

/// Marks a directory cargo can be run in.
pub const CARGO_MANIFEST: Marker = Marker {
    entry: "Cargo.toml",
    upward_search: "Cargo would search parent directories and operate on the enclosing workspace \
                    instead.",
};

/// Marks the top level of a git repository.
///
/// Only existence is checked: `.git` is a directory in an ordinary checkout and
/// a file in a worktree or a submodule.
pub const GIT_DIR: Marker = Marker {
    entry: ".git",
    upward_search: "Git would search parent directories and operate on the enclosing repository \
                    instead.",
};

/// Read the `root` tool option.
///
/// Returns `None` when the option is absent or null, which leaves the tool on
/// the root it was invoked with.
///
/// # Errors
///
/// Returns an error if the value is present but not a string.
/// Deserializing leniently is not an option here: a malformed value reported as
/// an absent one would run the tool against the workspace — writing to it, and
/// reporting success — after the user asked for another directory.
pub fn configured_root(options: &Map<String, Value>) -> Result<Option<&str>, String> {
    match options.get("root") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.as_str())),
        Some(other) => Err(format!(
            "The `root` tool option must be a string, got `{other}`."
        )),
    }
}

/// Resolve which directory a tool's subprocess runs in.
///
/// `None`, an empty value, or `.` all keep `default`, the root the tool was
/// invoked with.
/// Anything else is a workspace-relative path, resolved under the same
/// confinement every other tool path gets: absolute paths and `..` escapes are
/// refused, and symlinks are canonicalized before the workspace check.
/// An approved `external` mount is the only sanctioned way to reach a checkout
/// outside the workspace.
///
/// The confinement matters more here than for a read: the program that runs in
/// the resolved directory is not sandboxed, and for cargo it executes build
/// scripts and proc macros outright.
///
/// The target is required to be a directory holding `marker`, so a typo (or
/// naming a file inside the directory instead of the directory itself) surfaces
/// here rather than as a program that walked up to an enclosing project and
/// operated on that.
///
/// # Errors
///
/// Returns an error naming the configured value when it escapes the workspace,
/// is not a directory, lacks `marker`, or when `capabilities` are not granted
/// outright.
/// `external` only permits a path to resolve outside the workspace; it is not a
/// capability grant, so reaching a mount says nothing about what may be done
/// once there.
pub fn resolve_root(
    default: &Utf8Path,
    configured: Option<&str>,
    access: Option<&AccessPolicy>,
    capabilities: &[Capability],
    marker: &Marker,
) -> Result<Utf8PathBuf, String> {
    // An empty value or `.` resolves to the invocation root, so a config layer
    // can unload a previously-set root without naming the default.
    let configured = configured.filter(|value| !value.is_empty() && *value != ".");

    let Some(configured) = configured else {
        return Ok(default.to_owned());
    };

    let resolved = resolve_workspace_path(default, configured, access)?;

    if !resolved.absolute.is_dir() {
        return Err(format!(
            "The `root` option `{configured}` resolved to `{}`, which is not a directory.",
            resolved.absolute
        ));
    }

    if !resolved.absolute.join(marker.entry).exists() {
        return Err(format!(
            "The `root` option `{configured}` resolved to `{}`, which has no `{}`. {}",
            resolved.absolute, marker.entry, marker.upward_search
        ));
    }

    // Only a configured root is authorized. The invocation root is where these
    // tools have always run, so demanding grants for it here would revoke
    // access this option never handed out.
    for capability in capabilities {
        authorize(access, *capability, &resolved.relative)?;
    }

    Ok(resolved.absolute)
}

/// Name the directory `program` ran in, for failures from a redirected root.
///
/// A redirected root turns an ordinary failure into a baffling one, because
/// nothing in the program's own message hints that it ran somewhere other than
/// the workspace.
/// Successes are left alone: the caller asked for the redirect, so it only
/// needs restating when something goes wrong.
pub fn note_root(outcome: ToolResult, root: &Utf8Path, program: &str) -> ToolResult {
    let note = format!("({program} ran in `{root}`, set by the `root` tool option.)");

    match outcome {
        Ok(Outcome::Error {
            message,
            trace,
            transient,
        }) => Ok(Outcome::Error {
            message: format!("{message}\n\n{note}"),
            trace,
            transient,
        }),
        Err(error) => Err(format!("{error}\n\n{note}").into()),
        ok => ok,
    }
}

#[cfg(test)]
#[path = "root_tests.rs"]
mod tests;
