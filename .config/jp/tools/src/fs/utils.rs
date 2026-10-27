use camino::{Utf8Component, Utf8Path};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use jp_process::{ProcessOutput, ProcessRunner, SystemProcessRunner};
use jp_tool::{
    AccessPolicy, Capability, FsAccessError, Resolve, ResolvedPath, lexical_workspace_relative,
    resolve_path,
};

use crate::Error;

/// Matcher for the `suppress` tool option: paths a tool may read but must not
/// return.
///
/// Suppression is about disclosure, not reach.
/// A suppressed path stays readable to the tool process — which is what lets
/// `fs_modify_file` run `git status` against a suppressed `.git` to check for
/// uncommitted work — and is kept out of what the tool hands back.
/// Reach is [`AccessPolicy`]'s question.
///
/// Patterns use `.ignore` syntax, so `**/target/` matches at any depth, and are
/// matched against workspace-relative paths.
/// An empty list suppresses nothing, which is the default: what counts as noise
/// or as sensitive is a property of the project, not of a directory's name.
///
/// Patterns should name real locations rather than symlinks pointing at them.
/// A path is matched in both its canonical and its as-written form, so naming a
/// link does close that spelling — but only that one, and any other route to
/// the same target stays open.
///
/// # Errors
///
/// Returns an error naming the offending pattern if one cannot be parsed as a
/// glob.
/// A disclosure control that quietly drops the rule it could not read is worse
/// than one that refuses to start.
pub fn suppress_matcher(root: &Utf8Path, patterns: &[String]) -> Result<Gitignore, String> {
    let mut builder = GitignoreBuilder::new(root);
    for pattern in patterns {
        builder
            .add_line(None, pattern)
            .map_err(|error| format!("Invalid `suppress` pattern '{pattern}': {error}"))?;
    }
    builder
        .build()
        .map_err(|error| format!("Could not compile the `suppress` patterns: {error}"))
}

/// Whether `suppress` keeps `relative` out of a tool's results.
///
/// Parents are matched too, so a pattern naming a directory also covers the
/// files inside it — otherwise suppression is one path component away from
/// being bypassed.
///
/// The path is matched as a directory whatever it is on disk.
/// A pattern written `secrets/` is meant to cover that name, and wrongly
/// suppressing a file that happens to share it costs far less than returning a
/// directory's contents.
///
/// Pass [`ResolvedPath::relative`] and [`ResolvedPath::lexical`] so a pattern
/// matches whether it names the real location or the symlink a request arrived
/// through.
pub fn is_suppressed(suppress: &Gitignore, forms: &[&Utf8Path]) -> bool {
    forms
        .iter()
        .any(|form| suppress.matched_path_or_any_parents(form, true).is_ignore())
}

/// Report that a path was suppressed from a tool's results.
///
/// Reporting an empty result as though the path had been examined is what makes
/// a search read as evidence of absence.
/// The tool will not return this content however it is asked, so the way
/// forward is the user: naming that here is what lets the reader ask for the
/// contents instead of concluding they do not exist.
pub fn suppressed_note(path: &str) -> String {
    format!(
        "'{path}' is suppressed from this tool's results. If you need it, ask the user to provide \
         it."
    )
}

/// Enforce an access-policy capability on a resolved target.
///
/// The target arrives as the resolver produced it, so the path the policy sees
/// has its symlinked ancestors canonicalized and the reach of the operation is
/// read from the entry the resolver found: a mutating capability on a directory
/// is answered for everything beneath it, since renaming or removing a
/// directory takes its contents along.
///
/// A `None` policy (or an unrestricted one) permits everything.
/// A restricted policy permits only what a matching rule grants; on denial the
/// paths that do grant the capability are listed, so the reader can see where
/// it may go instead.
///
/// The refusal names the user as the way forward.
/// Only the policy can open the path, and the policy is the user's to change —
/// without that, a denial reads as a dead end.
pub fn authorize(
    access: Option<&AccessPolicy>,
    capability: Capability,
    target: &ResolvedPath,
) -> Result<(), String> {
    access
        .map_or(Ok(()), |policy| policy.authorize(capability, target))
        .map_err(describe)
}

/// Enforce a capability on a path the caller derived rather than resolved.
///
/// [`authorize`] is the way in for a path that came from a tool argument, and
/// reads `is_dir` off the resolved entry.
/// Use this only where there is no [`ResolvedPath`] to read it from — the
/// parent directory a delete leaves empty, or a destination whose reach the
/// source decides — and say at the call site why the value passed is the right
/// one.
///
/// `relative` must be a resolver output (`ResolvedPath::relative`) or a path
/// derived from one, never the raw tool input.
pub fn authorize_entry(
    access: Option<&AccessPolicy>,
    capability: Capability,
    relative: &Utf8Path,
    is_dir: bool,
) -> Result<(), String> {
    access
        .map_or(Ok(()), |policy| {
            policy.authorize_entry(capability, relative, is_dir)
        })
        .map_err(describe)
}

pub fn is_file_dirty(root: &Utf8Path, file: &Utf8Path) -> Result<bool, Error> {
    is_file_dirty_impl(root, file, &SystemProcessRunner)
}

pub(super) fn is_file_dirty_impl<R: ProcessRunner>(
    root: &Utf8Path,
    file: &Utf8Path,
    runner: &R,
) -> Result<bool, Error> {
    let ProcessOutput {
        stdout,
        stderr,
        status,
    } = runner.run("git", &["status", "--porcelain", "--", file.as_str()], root)?;

    if stderr.contains("fatal: not a git repository") {
        return Ok(false);
    }

    if !status.is_success() {
        return Err(format!("Git command failed ({status}): {stderr}").into());
    }

    // The second column is the non-staged status indicator.
    Ok(stdout.chars().nth(1) == Some('M'))
}

/// Count the number of dirty entries reported by `git status --porcelain` for
/// the given path.
/// For a file, returns 0 or 1.
/// For a directory, returns the number of changed entries underneath it.
/// Returns 0 when outside a git repository.
///
/// Unlike `is_file_dirty`, this counts any non-empty porcelain line — staged,
/// unstaged, untracked, renamed, all of it.
/// Directory moves have a wider blast radius than file moves, so the dirty
/// check is correspondingly stricter.
///
/// Currently the only caller (`fs_move_file`) flows through a generic
/// `ProcessRunner` for testability, so the production wrapper is omitted — add
/// one if a non-generic caller appears.
pub(super) fn count_dirty_paths_impl<R: ProcessRunner>(
    root: &Utf8Path,
    path: &Utf8Path,
    runner: &R,
) -> Result<usize, Error> {
    let ProcessOutput {
        stdout,
        stderr,
        status,
    } = runner.run("git", &["status", "--porcelain", "--", path.as_str()], root)?;

    if stderr.contains("fatal: not a git repository") {
        return Ok(0);
    }

    if !status.is_success() {
        return Err(format!("Git command failed ({status}): {stderr}").into());
    }

    Ok(stdout.lines().filter(|l| !l.is_empty()).count())
}

/// Maximum byte-length of any individual path component.
const MAX_COMPONENT_LEN: usize = 100;

/// Maximum number of `Normal` components in a user-supplied path.
const MAX_COMPONENT_COUNT: usize = 20;

/// Resolve a tool-call path against the workspace root, following symlinks all
/// the way to the final component.
///
/// Use this when the caller wants to operate on the *content* of the target —
/// reading, modifying, or otherwise touching whatever the path ultimately
/// points to.
/// Read- and modify-style tools belong here (`fs_read_file`, `fs_modify_file`).
///
/// For write tools that should operate on the directory entry itself (create,
/// delete, rename), use [`resolve_workspace_entry`] instead — it does not
/// follow a final symlink and so cannot be tricked into writing or unlinking
/// through one.
///
/// The workspace boundary is [`resolve_path`]'s; this adds the limits on what a
/// tool call may name, and the wording the assistant reads on refusal.
pub fn resolve_workspace_path(
    root: &Utf8Path,
    path: &str,
    access: Option<&AccessPolicy>,
) -> Result<ResolvedPath, String> {
    check_input_limits(path)?;
    resolve_path(root, Utf8Path::new(path), access, Resolve::Target).map_err(describe)
}

/// Resolve a tool-call path as a directory entry, canonicalizing only the
/// *parent*.
///
/// The final component is preserved as-is: if it is an existing symlink, it is
/// left intact rather than followed.
/// This is the right primitive for tools that operate on the entry itself —
/// create, delete, rename — where following the link would silently retarget
/// the operation onto whatever the link points at.
pub fn resolve_workspace_entry(
    root: &Utf8Path,
    path: &str,
    access: Option<&AccessPolicy>,
) -> Result<ResolvedPath, String> {
    check_input_limits(path)?;
    resolve_path(root, Utf8Path::new(path), access, Resolve::Entry).map_err(describe)
}

/// Reject tool-call paths that are empty, or longer or deeper than any real
/// workspace path the assistant has a reason to name.
///
/// Counted on the lexically normalized form, so `a/b/../c` is two components.
/// A path that does not normalize (absolute, or escaping through `..`) is left
/// to the resolver, which names what is wrong with it.
fn check_input_limits(path: &str) -> Result<(), String> {
    let Some(cleaned) = lexical_workspace_relative(Utf8Path::new(path)) else {
        return Ok(());
    };

    let mut count = 0usize;
    for component in cleaned.components() {
        if let Utf8Component::Normal(name) = component {
            if name.len() > MAX_COMPONENT_LEN {
                return Err(format!(
                    "Individual path components must be less than {MAX_COMPONENT_LEN} characters \
                     long."
                ));
            }
            count += 1;
        }
    }

    if count == 0 {
        return Err("Path must not be empty.".to_owned());
    }
    if count > MAX_COMPONENT_COUNT {
        return Err(format!(
            "Path must be less than {MAX_COMPONENT_COUNT} components long."
        ));
    }

    Ok(())
}

/// The message the assistant reads when a path is refused.
///
/// Each names what to do instead where there is something to do.
fn describe(error: FsAccessError) -> String {
    match error {
        FsAccessError::Absolute(_) => "Path must be relative.".to_owned(),
        FsAccessError::InputEscape(_) => "Path must not escape the workspace root.".to_owned(),
        FsAccessError::DanglingSymlink(link) => {
            format!("Path '{link}' is a symlink with a missing or non-workspace target.")
        }
        FsAccessError::Escape(_) => "Path escapes the workspace root.".to_owned(),
        FsAccessError::ThroughMount {
            path,
            mount,
            workspace_path,
        } => format!(
            "Path '{path}' reaches the workspace through mount '{mount}'. Use '{workspace_path}' \
             instead."
        ),
        FsAccessError::Denied {
            capability,
            target,
            grants,
        } => {
            let grants = grants
                .iter()
                .map(|grant| grant.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "Access denied: cannot {capability} '{target}'. Paths granting {capability}: \
                 [{grants}]. If required, ask the user for explicit access."
            )
        }
        FsAccessError::HoldsWorkspace { capability, path } => {
            format!("Access denied: cannot {capability} '{path}': it contains the workspace.")
        }
        FsAccessError::Io { path, message } => {
            format!("Failed to resolve path '{path}': {message}")
        }
    }
}

#[cfg(test)]
#[path = "utils_tests.rs"]
mod tests;
