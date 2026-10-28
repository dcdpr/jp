//! Resolving tool-call paths against the workspace, and authorizing them.
//!
//! [`resolve_path`] turns the workspace-relative path a tool was given into a
//! [`ResolvedPath`], enforcing the workspace boundary.
//! [`AccessPolicy::authorize`] then decides whether the policy grants a
//! capability on it.
//! [`Context::check_read`] and friends do both in one call.
//!
//! [`Context::check_read`]: crate::Context::check_read

use std::io;

use camino::{Utf8Component, Utf8Path, Utf8PathBuf};

use super::{
    AccessPolicy, Capability, FsAccessError, find_matching_rule, mount_entered, normalize_lexical,
};

/// How [`resolve_path`] treats the final component of a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolve {
    /// Follow symlinks through the final component, to what the path names.
    ///
    /// For operations on content: reading, modifying, executing.
    Target,

    /// Leave the final component as named, so a symlink there is the entry
    /// itself.
    ///
    /// For operations on the directory entry: creating, deleting, renaming.
    /// Removing or renaming a symlink resolved this way acts on the link, not
    /// on what it points at.
    Entry,
}

/// Kind of filesystem entry, read without following a final symlink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// A regular file.
    File,
    /// A directory.
    Dir,
    /// A symlink, live or dangling.
    Symlink,
    /// A block device, fifo, socket, or anything else.
    Other,
}

/// Stat an entry without following a final symlink.
///
/// Returns `Ok(None)` when nothing exists at `path`.
/// A dangling symlink is an entry and reports [`EntryKind::Symlink`], where
/// [`Utf8Path::exists`] would report nothing.
///
/// # Errors
///
/// Returns the I/O error for any failure other than the entry being absent.
pub fn entry_kind(path: &Utf8Path) -> io::Result<Option<EntryKind>> {
    match path.symlink_metadata() {
        Ok(metadata) => {
            let file_type = metadata.file_type();
            let kind = if file_type.is_symlink() {
                EntryKind::Symlink
            } else if file_type.is_dir() {
                EntryKind::Dir
            } else if file_type.is_file() {
                EntryKind::File
            } else {
                EntryKind::Other
            };
            Ok(Some(kind))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// A tool-call path resolved against the workspace root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPath {
    /// The absolute path to operate on.
    ///
    /// With [`Resolve::Target`] every symlink on the way is followed.
    /// With [`Resolve::Entry`] the final component is the entry as named.
    pub absolute: Utf8PathBuf,

    /// The path access rules match on.
    ///
    /// Workspace-relative with symlinks resolved for a target inside the
    /// workspace, so a path reached through an in-workspace symlink matches the
    /// rule for its real location.
    /// The lexical path for a target reached through an approved mount, which
    /// is what external rules match on.
    pub relative: Utf8PathBuf,

    /// The path as given, normalized lexically.
    ///
    /// Differs from `relative` when the path passes through a symlink.
    pub lexical: Utf8PathBuf,

    /// What sits at `absolute`, or `None` when nothing does yet.
    pub kind: Option<EntryKind>,

    /// Whether `absolute` is a directory the workspace lies inside.
    ///
    /// Only reachable through a mount whose target contains the workspace.
    /// Moving or deleting such a directory takes the workspace along, whatever
    /// path named it.
    pub holds_workspace: bool,
}

impl ResolvedPath {
    /// Whether a directory sits at the resolved path.
    ///
    /// This decides how far a mutating operation reaches, so a symlink to a
    /// directory is not one: renaming or removing the link leaves the directory
    /// alone.
    #[must_use]
    pub fn is_dir(&self) -> bool {
        self.kind == Some(EntryKind::Dir)
    }
}

/// Resolve a tool-call `input` against the workspace `root`.
///
/// `input` must be workspace-relative; an empty path is the root.
/// Symlinks are followed as `mode` says; a target that does not exist yet
/// resolves through its deepest existing ancestor.
///
/// # Errors
///
/// - [`FsAccessError::Absolute`] for an absolute, rooted, or drive-prefixed
///   input, before any I/O.
/// - [`FsAccessError::InputEscape`] for an input whose `..` climbs out of the
///   workspace, before any I/O.
/// - [`FsAccessError::DanglingSymlink`] when a symlink that has to be followed
///   points at nothing: there is no target to keep inside the workspace, and
///   creating through it would land wherever it points.
/// - [`FsAccessError::Escape`] when the path resolves outside the workspace and
///   no approved external rule in `access` contains where it lands.
/// - [`FsAccessError::ThroughMount`] when the path enters an approved mount and
///   resolves back into the workspace.
/// - [`FsAccessError::Io`] when the filesystem cannot be read.
pub fn resolve_path(
    root: &Utf8Path,
    input: &Utf8Path,
    access: Option<&AccessPolicy>,
    mode: Resolve,
) -> Result<ResolvedPath, FsAccessError> {
    // `has_root` and the prefix check catch the shapes `is_absolute` misses on
    // Windows: `\foo` is rooted without a drive, and `C:foo` has a drive
    // without a root.
    let prefixed = input
        .components()
        .any(|component| matches!(component, Utf8Component::Prefix(_)));
    if input.is_absolute() || input.has_root() || prefixed {
        return Err(FsAccessError::Absolute(input.to_owned()));
    }
    let lexical =
        normalize_lexical(input).ok_or_else(|| FsAccessError::InputEscape(input.to_owned()))?;

    let root_canonical = root.canonicalize_utf8().map_err(|e| io_error(root, &e))?;
    let rules = access.map_or(&[][..], |policy| policy.fs.as_slice());

    // The part of the path whose symlinks are followed, and the final name an
    // entry resolution leaves alone.
    let (followed, entry_name) = match (mode, lexical.file_name()) {
        (Resolve::Entry, Some(name)) => (lexical.parent().unwrap_or(Utf8Path::new("")), Some(name)),
        _ => (lexical.as_path(), None),
    };

    let (ancestor, missing) = canonicalize_existing_ancestor(&root_canonical, followed)?;

    let mut absolute = ancestor.clone();
    for name in missing.iter().rev().map(String::as_str).chain(entry_name) {
        absolute.push(name);
    }

    if !ancestor.starts_with(&root_canonical) {
        // Outside the workspace, only an approved external rule whose target
        // contains where the path lands lets it through. A symlink nested
        // inside the target cannot carry it further.
        let permitted = find_matching_rule(rules, &lexical).is_some_and(|rule| {
            rule.external()
                && rule
                    .approved_target()
                    .is_some_and(|target| ancestor.starts_with(target))
        });
        if !permitted {
            return Err(FsAccessError::Escape(absolute));
        }
    }

    let relative = absolute
        .strip_prefix(&root_canonical)
        .map_or_else(|_| lexical.clone(), Utf8Path::to_owned);

    // Through a mount whose target contains the workspace, a workspace entry
    // has a second path. Only its workspace path is accepted, so the
    // workspace's rules are the only ones that ever apply to it.
    //
    // This looks at the whole path, not the canonical ancestor: an entry
    // resolution of `up/ws` leaves `ws` unresolved, and it is the workspace.
    if absolute.starts_with(&root_canonical)
        && let Some(mount) = mount_entered(rules, followed)
    {
        let workspace_path = if relative.as_str().is_empty() {
            Utf8PathBuf::from(".")
        } else {
            relative
        };
        return Err(FsAccessError::ThroughMount {
            path: lexical,
            mount: mount.lexical_path().to_owned(),
            workspace_path,
        });
    }

    let kind = entry_kind(&absolute).map_err(|e| io_error(&absolute, &e))?;
    // Read off the real location, not the path given, which may reach it
    // through an alias the mount's workspace exclusion does not sit under.
    let holds_workspace = kind == Some(EntryKind::Dir)
        && root_canonical != absolute
        && root_canonical.starts_with(&absolute);

    Ok(ResolvedPath {
        absolute,
        relative,
        lexical,
        kind,
        holds_workspace,
    })
}

impl AccessPolicy {
    /// Check that the policy grants `capability` on a resolved path.
    ///
    /// A mutating capability on a directory is checked for everything beneath
    /// it, since renaming or removing a directory takes its contents along.
    /// On a directory the workspace lies inside, it is refused outright, so no
    /// rule can grant moving or deleting the workspace through a mount.
    ///
    /// # Errors
    ///
    /// Returns [`FsAccessError::HoldsWorkspace`] for a mutating capability on a
    /// directory holding the workspace, and [`FsAccessError::Denied`], listing
    /// the paths that do grant the capability, when no rule grants it.
    pub fn authorize(
        &self,
        capability: Capability,
        target: &ResolvedPath,
    ) -> Result<(), FsAccessError> {
        if capability.mutates() && target.holds_workspace {
            return Err(FsAccessError::HoldsWorkspace {
                capability: capability.as_str(),
                path: target.relative.clone(),
            });
        }

        self.authorize_entry(capability, &target.relative, target.is_dir())
    }

    /// Check that the policy grants `capability` on a path derived from a
    /// resolved one, such as the parent directory a delete leaves empty.
    ///
    /// `relative` must be in the form [`ResolvedPath::relative`] takes, and
    /// `is_dir` says whether the operation reaches the subtree beneath it.
    ///
    /// # Errors
    ///
    /// Returns [`FsAccessError::Denied`], listing the paths that do grant the
    /// capability.
    pub fn authorize_entry(
        &self,
        capability: Capability,
        relative: &Utf8Path,
        is_dir: bool,
    ) -> Result<(), FsAccessError> {
        if self.permits_entry(capability, relative, is_dir) {
            return Ok(());
        }

        Err(FsAccessError::Denied {
            capability: capability.as_str(),
            target: relative.to_owned(),
            grants: self
                .granting_paths(capability)
                .map(Utf8Path::to_owned)
                .collect(),
        })
    }
}

impl crate::Context {
    /// Resolve `input` against the workspace under this context's policy.
    ///
    /// See [`resolve_path`].
    ///
    /// # Errors
    ///
    /// See [`resolve_path`].
    pub fn resolve(&self, input: &Utf8Path, mode: Resolve) -> Result<ResolvedPath, FsAccessError> {
        resolve_path(&self.root, input, self.access.as_ref(), mode)
    }

    /// Check that this context's policy grants `capability` on `target`.
    ///
    /// Without a policy, everything inside the workspace is granted.
    ///
    /// # Errors
    ///
    /// See [`AccessPolicy::authorize`].
    pub fn authorize(
        &self,
        capability: Capability,
        target: &ResolvedPath,
    ) -> Result<(), FsAccessError> {
        self.access
            .as_ref()
            .map_or(Ok(()), |policy| policy.authorize(capability, target))
    }

    /// Check read access to `input`, returning the resolved absolute path.
    ///
    /// `input` must be workspace-relative; the check joins it against the
    /// workspace root and resolves it itself, following every symlink.
    /// Do not pre-resolve paths against the root before calling.
    /// To act on a symlink entry rather than its target, use [`Self::resolve`]
    /// with [`Resolve::Entry`] and [`Self::authorize`].
    ///
    /// # Errors
    ///
    /// See [`resolve_path`] and [`AccessPolicy::authorize`].
    pub fn check_read(&self, input: &Utf8Path) -> Result<Utf8PathBuf, FsAccessError> {
        self.check_capability(input, Capability::Read)
    }

    /// Check create access to `input`, returning the resolved absolute path.
    ///
    /// # Errors
    ///
    /// See [`Self::check_read`].
    pub fn check_create(&self, input: &Utf8Path) -> Result<Utf8PathBuf, FsAccessError> {
        self.check_capability(input, Capability::Create)
    }

    /// Check update access to `input`, returning the resolved absolute path.
    ///
    /// # Errors
    ///
    /// See [`Self::check_read`].
    pub fn check_update(&self, input: &Utf8Path) -> Result<Utf8PathBuf, FsAccessError> {
        self.check_capability(input, Capability::Update)
    }

    /// Check delete access to `input`, returning the resolved absolute path.
    ///
    /// # Errors
    ///
    /// See [`Self::check_read`].
    pub fn check_delete(&self, input: &Utf8Path) -> Result<Utf8PathBuf, FsAccessError> {
        self.check_capability(input, Capability::Delete)
    }

    /// Check execute access to `input`, returning the resolved absolute path.
    ///
    /// # Errors
    ///
    /// See [`Self::check_read`].
    pub fn check_execute(&self, input: &Utf8Path) -> Result<Utf8PathBuf, FsAccessError> {
        self.check_capability(input, Capability::Execute)
    }

    fn check_capability(
        &self,
        input: &Utf8Path,
        capability: Capability,
    ) -> Result<Utf8PathBuf, FsAccessError> {
        let target = self.resolve(input, Resolve::Target)?;
        self.authorize(capability, &target)?;
        Ok(target.absolute)
    }
}

/// Canonicalize the deepest existing ancestor of `lexical` under the root.
///
/// Returns the canonical ancestor and the names of the components that do not
/// exist yet, deepest first.
/// A component that exists only as a dangling symlink is not a missing one: the
/// entry is there, and following it would land wherever it points.
fn canonicalize_existing_ancestor(
    root_canonical: &Utf8Path,
    lexical: &Utf8Path,
) -> Result<(Utf8PathBuf, Vec<String>), FsAccessError> {
    let mut current = if lexical.as_str().is_empty() {
        root_canonical.to_owned()
    } else {
        root_canonical.join(lexical)
    };
    let mut missing: Vec<String> = vec![];

    loop {
        match current.canonicalize_utf8() {
            Ok(canonical) => return Ok((canonical, missing)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if current.symlink_metadata().is_ok() {
                    // `lexical` carries no `..`, so `current` is still written
                    // under the root even when it passes through a mount.
                    let link = current
                        .strip_prefix(root_canonical)
                        .map_or_else(|_| current.clone(), Utf8Path::to_owned);
                    return Err(FsAccessError::DanglingSymlink(link));
                }

                // The root exists, so a missing component always has a parent
                // under it to step back to.
                let (Some(name), Some(parent)) = (current.file_name(), current.parent()) else {
                    return Err(io_error(&current, &error));
                };
                missing.push(name.to_owned());
                current = parent.to_owned();
            }
            Err(error) => return Err(io_error(&current, &error)),
        }
    }
}

fn io_error(path: &Utf8Path, error: &io::Error) -> FsAccessError {
    FsAccessError::Io {
        path: path.to_owned(),
        message: error.to_string(),
    }
}

#[cfg(test)]
#[path = "resolve_tests.rs"]
mod tests;
