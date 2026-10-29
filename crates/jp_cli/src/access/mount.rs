//! Parsing and resolution for the `--mount` / `--no-mount` CLI flags.
//!
//! A mount spec has the form `[TOOL:][NAME=]PATH[:MODE]`:
//!
//! - `NAME` is a workspace-relative location for the symlink.
//!   When omitted, `PATH` must exist, and `NAME` is its final segment.
//! - `PATH` is the external target the symlink points at.
//! - `MODE` is `ro` (default) or `rw`.
//!   `rw` requires an explicit `TOOL:` prefix.
//! - `TOOL:` scopes the grant to a single tool; without it the grant expands to
//!   all enabled local and command plugin tools.
//!   Without `NAME=`, a leading `TOOL:` could also be part of `PATH`; whichever
//!   reading exists on disk wins, and both existing is an error.

use camino::{Utf8Component, Utf8Path, Utf8PathBuf};
use jp_config::{conversation::tool::access::FsRuleConfig, fs::expand_tilde};

/// Read/write mode for a mount.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountMode {
    /// Read-only (the default).
    Ro,
    /// Read-write.
    Rw,
}

/// A parsed `--mount` specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountSpec {
    /// Single tool the grant applies to, or `None` for all enabled local and
    /// command plugin tools.
    pub tool: Option<String>,
    /// Workspace-relative symlink location, as typed (relative to CWD).
    pub name: String,
    /// External target the symlink points at, as typed.
    pub path: String,
    /// Read/write mode.
    pub mode: MountMode,
}

/// Failure reasons for parsing a mount spec.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MountParseError {
    /// The spec has no `NAME=`, and `PATH` has no final segment to name the
    /// symlink after (e.g. `/`).
    #[error("invalid mount '{0}': cannot derive a mount name from PATH, use NAME=PATH")]
    NoName(String),

    /// The spec has no `NAME=`, and nothing exists at `PATH`.
    #[error("invalid mount '{0}': PATH does not exist")]
    NotFound(String),

    /// The spec has no `NAME=`, and both `TOOL:PATH` as one path and `PATH`
    /// alone exist, so the prefix could be either.
    #[error(
        "invalid mount '{spec}': '{tool}:' could be a tool or part of the path, as both exist; \
         use NAME=PATH or {tool}:NAME=PATH"
    )]
    Ambiguous {
        /// The whole spec.
        spec: String,
        /// The prefix that reads as a tool name.
        tool: String,
    },

    /// `NAME` or `PATH` was empty.
    #[error("invalid mount '{0}': NAME and PATH must both be non-empty")]
    Empty(String),

    /// The tool prefix is not a valid tool identifier.
    #[error("invalid mount '{spec}': '{tool}' is not a valid tool name")]
    InvalidTool {
        /// The whole spec.
        spec: String,
        /// The offending tool segment.
        tool: String,
    },

    /// `:rw` was requested without a `TOOL:` prefix.
    #[error(
        "invalid mount '{0}': ':rw' requires a TOOL: prefix (e.g. fs_modify_file:NAME=PATH:rw)"
    )]
    WriteRequiresTool(String),
}

impl MountSpec {
    /// Parse a `[TOOL:][NAME=]PATH[:MODE]` spec.
    ///
    /// The first `=` splits name from path; a `:` on the left names the tool, a
    /// trailing `:ro`/`:rw` on the right sets the mode.
    /// Windows drive letters only appear in `PATH`, where the mode is peeled
    /// from the tail, so they don't collide with the tool prefix.
    ///
    /// Without an `=`, the spec is `[TOOL:]PATH[:MODE]` and the name is the
    /// final segment of `PATH` after lexical normalization, so
    /// `/code/forks/serde` mounts as `serde` and `..` as the parent's name.
    /// `resolve` maps a `PATH` as typed to its absolute location when something
    /// exists there (see [`absolute_target`]); it is only called for this form,
    /// to require that `PATH` exists and to decide whether a leading `TOOL:` is
    /// a scope or part of the path.
    pub fn parse(
        spec: &str,
        resolve: impl Fn(&str) -> Option<Utf8PathBuf>,
    ) -> Result<Self, MountParseError> {
        let (left, right) = match spec.split_once('=') {
            Some((left, right)) => (Some(left), right),
            None => (None, spec),
        };

        let (path, mode) = match right.strip_suffix(":rw") {
            Some(path) => (path, MountMode::Rw),
            None => match right.strip_suffix(":ro") {
                Some(path) => (path, MountMode::Ro),
                None => (right, MountMode::Ro),
            },
        };

        // Checked before resolving, which would anchor an empty PATH at the CWD.
        if path.is_empty() {
            return Err(MountParseError::Empty(spec.to_owned()));
        }

        let (tool, name, path) = if let Some(left) = left {
            match left.split_once(':') {
                Some((tool, name)) => {
                    if !is_tool_identifier(tool) {
                        return Err(MountParseError::InvalidTool {
                            spec: spec.to_owned(),
                            tool: tool.to_owned(),
                        });
                    }
                    (Some(tool.to_owned()), name.to_owned(), path)
                }
                None => (None, left.to_owned(), path),
            }
        } else {
            parse_bare(spec, path, resolve)?
        };

        if name.is_empty() {
            return Err(MountParseError::Empty(spec.to_owned()));
        }

        if mode == MountMode::Rw && tool.is_none() {
            return Err(MountParseError::WriteRequiresTool(spec.to_owned()));
        }

        Ok(Self {
            tool,
            name,
            path: path.to_owned(),
            mode,
        })
    }

    /// Resolve `name` to a workspace-relative path.
    ///
    /// `name` is interpreted relative to `cwd`, normalized lexically, and must
    /// land under `workspace_root`.
    /// Returns the workspace-relative symlink location (the `lexical_path` the
    /// resulting rule will carry).
    ///
    /// # Errors
    ///
    /// Returns an error if the resolved location escapes the workspace or lands
    /// under JP-managed storage (`.jp/`).
    pub fn resolve_name(
        &self,
        cwd: &Utf8Path,
        workspace_root: &Utf8Path,
    ) -> Result<Utf8PathBuf, MountResolveError> {
        resolve_workspace_relative(&self.name, cwd, workspace_root)
    }

    /// Build the `access.fs` rule for this mount at the given
    /// workspace-relative rule path.
    #[must_use]
    pub fn rule(&self, rule_path: &str) -> FsRuleConfig {
        FsRuleConfig {
            path: rule_path.to_owned(),
            external: Some(true),
            read: Some(true),
            write: Some(self.mode == MountMode::Rw),
            create: None,
            update: None,
            delete: None,
            execute: None,
        }
    }
}

/// Failure reasons for resolving a mount `NAME`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MountResolveError {
    /// The resolved location escapes the workspace root.
    #[error("mount '{0}' resolves outside the workspace")]
    OutsideWorkspace(String),

    /// The resolved location is under JP-managed storage.
    #[error("mount '{0}' targets JP-managed storage (.jp/)")]
    ManagedStorage(String),
}

fn resolve_workspace_relative(
    name: &str,
    cwd: &Utf8Path,
    workspace_root: &Utf8Path,
) -> Result<Utf8PathBuf, MountResolveError> {
    // `NAME` is interpreted relative to the current directory. An absolute
    // input is accepted only if it falls under the workspace; the
    // strip-prefix below converts it to the same workspace-relative form a
    // relative input produces, and rejects anything outside.
    let joined = if Utf8Path::new(name).is_absolute() {
        Utf8PathBuf::from(name)
    } else {
        cwd.join(name)
    };

    let normalized = normalize_lexical(&joined);

    let relative = normalized
        .strip_prefix(workspace_root)
        .map_err(|_| MountResolveError::OutsideWorkspace(name.to_owned()))?;

    if relative.components().next().is_none() {
        return Err(MountResolveError::OutsideWorkspace(name.to_owned()));
    }

    if relative.starts_with(".jp") {
        return Err(MountResolveError::ManagedStorage(name.to_owned()));
    }

    Ok(relative.to_owned())
}

/// Read a `[TOOL:]PATH` with no `NAME=`, returning the tool, the derived name,
/// and the path.
///
/// A leading tool identifier is a scope only when the rest of `path` exists and
/// `path` as a whole does not.
/// A single-letter prefix is never a scope: it is read as a Windows drive
/// letter, and `\code\x` resolves on the current drive just as `c:\code\x`
/// does, which would make every lowercase drive path ambiguous.
fn parse_bare<'a>(
    spec: &str,
    path: &'a str,
    resolve: impl Fn(&str) -> Option<Utf8PathBuf>,
) -> Result<(Option<String>, String, &'a str), MountParseError> {
    let whole = resolve(path);
    let scoped = path
        .split_once(':')
        .filter(|(tool, rest)| tool.len() > 1 && is_tool_identifier(tool) && !rest.is_empty())
        .and_then(|(tool, rest)| resolve(rest).map(|target| (tool, rest, target)));

    let (tool, path, target) = match (whole, scoped) {
        (Some(_), Some((tool, ..))) => {
            return Err(MountParseError::Ambiguous {
                spec: spec.to_owned(),
                tool: tool.to_owned(),
            });
        }
        (Some(target), None) => (None, path, target),
        (None, Some((tool, rest, target))) => (Some(tool.to_owned()), rest, target),
        (None, None) => return Err(MountParseError::NotFound(spec.to_owned())),
    };

    // Lexical, not canonical: a symlinked PATH keeps the name the user typed
    // rather than taking its target's.
    let name = normalize_lexical(&target)
        .file_name()
        .ok_or_else(|| MountParseError::NoName(spec.to_owned()))?
        .to_owned();

    Ok((tool, name, path))
}

/// Anchor a mount `PATH` as typed at `cwd`, expanding a leading `~` to `home`.
///
/// Touches no filesystem.
/// Returns `None` when `path` starts with `~` and `home` is unset.
#[must_use]
pub fn absolute_target(path: &str, cwd: &Utf8Path, home: Option<&str>) -> Option<Utf8PathBuf> {
    let path = expand_tilde(path, home)?;
    Some(if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    })
}

/// Lexically normalize an absolute path, collapsing `.` and `..` without
/// touching the filesystem.
/// `..` cannot pop above the filesystem root.
fn normalize_lexical(path: &Utf8Path) -> Utf8PathBuf {
    let mut out = Utf8PathBuf::new();
    for component in path.components() {
        match component {
            Utf8Component::CurDir => {}
            Utf8Component::ParentDir => {
                out.pop();
            }
            Utf8Component::RootDir => out.push("/"),
            Utf8Component::Prefix(prefix) => out.push(prefix.as_str()),
            Utf8Component::Normal(segment) => out.push(segment),
        }
    }
    out
}

/// Whether `s` is a valid tool identifier (`[a-z_][a-z0-9_]*`).
fn is_tool_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_lowercase() || c.is_ascii_digit())
}

#[cfg(test)]
#[path = "mount_tests.rs"]
mod tests;
