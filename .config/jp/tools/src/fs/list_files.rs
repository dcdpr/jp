use camino::{Utf8Path, Utf8PathBuf};
use ignore::{IncrementalIgnore, WalkBuilder, WalkState, gitignore::Gitignore};
use jp_tool::{AccessPolicy, Capability};
use serde::ser::SerializeMap as _;

use super::utils::{is_suppressed, resolve_workspace_path, suppressed_note};
use crate::{Error, util::OneOrMany};

/// Outcome of a listing: the files found, plus any requested path that
/// contributed nothing and why.
#[derive(Debug)]
pub(crate) struct Files {
    files: Vec<String>,
    skipped: Vec<Skipped>,
}

/// A requested path that produced no results, and the reason.
///
/// The two reasons carry different remedies, which is why they are not
/// collapsed: only the policy can open a denied path, and only the user can
/// hand over a suppressed one.
#[derive(Debug)]
enum Skipped {
    /// The access policy does not grant read on it.
    Denied(String),
    /// The tool may read it but never returns it.
    Suppressed(String),
}

impl Skipped {
    fn note(&self) -> String {
        match self {
            Self::Denied(path) => denied_note(path),
            Self::Suppressed(path) => suppressed_note(path),
        }
    }
}

impl Files {
    pub(crate) fn into_files(self) -> Vec<String> {
        self.files
    }

    /// One sentence per requested path that produced no results.
    ///
    /// Empty for a listing that covered everything it was asked about.
    pub(crate) fn notes(&self) -> Vec<String> {
        self.skipped.iter().map(Skipped::note).collect()
    }
}

impl serde::Serialize for Files {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // A listing that reached everything it was asked about serializes as a
        // bare list (or a single sentence when nothing matched). The keyed shape
        // appears only when something was skipped, so the notes cannot be
        // mistaken for part of the file set.
        if self.skipped.is_empty() {
            return if self.files.is_empty() {
                serializer.serialize_str("No files found.")
            } else {
                self.files.serialize(serializer)
            };
        }

        let mut map = serializer.serialize_map(Some(2))?;
        map.serialize_entry("files", &self.files)?;
        map.serialize_entry("notes", &self.notes())?;
        map.end()
    }
}

/// Report that a requested path was withheld by the access policy.
///
/// Reporting an empty result as though the path had been examined is what makes
/// a search read as evidence of absence.
/// The policy is not the reader's to change, so the way forward is the user:
/// naming that here is what lets the reader ask for the contents instead of
/// concluding they do not exist.
fn denied_note(path: &str) -> String {
    format!(
        "'{path}' is not readable by this tool and was skipped. If you need it, ask the user to \
         provide it."
    )
}

pub(crate) async fn fs_list_files(
    root: &Utf8Path,
    access: Option<&AccessPolicy>,
    prefixes: Option<OneOrMany<String>>,
    extensions: Option<OneOrMany<String>>,
    suppress: &Gitignore,
) -> std::result::Result<Files, Error> {
    let prefixes = prefixes.unwrap_or(OneOrMany::One(String::new())).into_vec();
    let mut ignore_rules = IgnoreRules::for_workspace(root);

    let mut files = vec![];
    let mut skipped = vec![];
    for prefix in &prefixes {
        match resolve_target(root, prefix, access, &mut ignore_rules, suppress)? {
            Target::File(path) => files.extend(explicit_file(&path, extensions.as_ref())),
            Target::Walk(spec) => {
                files.extend(collect_files(&spec, extensions.as_ref(), access, suppress));
            }
            Target::Skipped(reason) => skipped.push(reason),
        }
    }

    files.sort();
    files.dedup();

    Ok(Files { files, skipped })
}

/// What a single prefix resolves to.
enum Target {
    /// An existing file, read without a walk.
    File(Utf8PathBuf),
    /// A tree to walk.
    Walk(WalkSpec),
    /// An existing path that produces no results.
    Skipped(Skipped),
}

/// Where to walk for a prefix, and how to present the results.
struct WalkSpec {
    /// Directory the walk is rooted at.
    walk_root: Utf8PathBuf,
    /// Prefix prepended to each result so output is workspace-relative: empty
    /// for in-workspace walks, the mount name for an approved external mount.
    display_prefix: Utf8PathBuf,
    /// Optional partial-prefix filter applied to the display path.
    path_filter: Option<String>,
    /// Whether `.ignore` files prune this walk.
    ///
    /// Disabled for a subtree the caller named outright and `soft_ignore` opted
    /// in: the anchored root patterns do not prune reliably below the workspace
    /// root, so leaving them on would prune unpredictably rather than not at
    /// all.
    apply_ignore: bool,
}

/// Resolve a prefix into the work it implies.
///
/// An empty prefix or bare `.` walks the whole workspace.
///
/// A prefix naming an approved external mount walks the mount's canonical
/// target (bounded by the approved target) and presents results under the mount
/// name.
///
/// A prefix the access policy withholds, or one the `suppress` list covers, is
/// skipped, so the caller learns its request went unanswered.
///
/// A prefix naming an existing file reads it directly, and a prefix naming an
/// `.ignore`d directory walks that directory as its own root.
/// Ignore rules govern what traversal *surfaces*, not what the caller may name:
/// a path the caller already knows about is one it can already read, so hiding
/// it here would withhold nothing while breaking a legitimate request.
/// Paths that must stay closed however they are named are the access policy's
/// job, not the ignore rules'.
///
/// Any other prefix scopes the workspace walk with a path filter rather than
/// re-rooting: the walk starts at the workspace root so the root `.ignore`
/// whitelist applies consistently (its anchored patterns like
/// `docs/.vitepress/dist/` only prune when the walk is rooted at the `.ignore`
/// file).
fn resolve_target(
    root: &Utf8Path,
    prefix: &str,
    access: Option<&AccessPolicy>,
    ignore_rules: &mut IgnoreRules,
    suppress: &Gitignore,
) -> std::result::Result<Target, Error> {
    if prefix.is_empty() || prefix == "." {
        return Ok(Target::Walk(WalkSpec {
            walk_root: root.to_owned(),
            display_prefix: Utf8PathBuf::new(),
            path_filter: None,
            apply_ignore: true,
        }));
    }

    // The canonical workspace-relative form, which is what access rules match
    // on: a path reached through an in-workspace symlink is checked against the
    // rule for its real location, so a link cannot dodge a rule denying its
    // target. External mount paths have no canonical workspace-relative form and
    // keep their lexical shape, which is what external rules match.
    let cleaned = resolve_workspace_path(root, prefix, access)?.relative;

    // Both checks come before the mount branch below: an approved mount is named
    // by its in-workspace path, which is the form access rules and suppress
    // patterns both match, so a mount can be withheld the same way any other
    // directory can.
    //
    // Access is checked first because it is the harder boundary — a path the tool
    // may not read at all is not merely one it declines to return.
    if access.is_some_and(|policy| !policy.permits(Capability::Read, &cleaned)) {
        return Ok(Target::Skipped(Skipped::Denied(cleaned.into_string())));
    }

    if is_suppressed(suppress, &cleaned) {
        return Ok(Target::Skipped(Skipped::Suppressed(cleaned.into_string())));
    }

    // `follow_links(false)` in `collect_files` keeps nested symlinks inside the
    // target from escaping the approved boundary.
    if let Some(rule) = access.and_then(|policy| policy.matching_fs_rule(&cleaned))
        && rule.external()
        && let Some(target) = rule.approved_target()
    {
        return Ok(Target::Walk(WalkSpec {
            walk_root: target.to_owned(),
            display_prefix: rule.lexical_path().to_owned(),
            path_filter: Some(prefix_filter(&cleaned, root)),
            apply_ignore: true,
        }));
    }

    let full = root.join(&cleaned);
    let is_dir = full.is_dir();
    let is_file = full.is_file();

    // Only an existing path can be classified. A partial prefix like `rfd/D`
    // names nothing on disk and stays a filter over the workspace walk.
    let exists = is_dir || is_file;

    if is_file {
        return Ok(Target::File(cleaned));
    }

    // Walking the subtree as its own root is the only way to reach it: the
    // anchored root patterns do not prune reliably below the workspace root, so
    // scoping to it with a filter would find nothing.
    if exists && ignore_rules.prunes(&cleaned, is_dir) {
        return Ok(Target::Walk(WalkSpec {
            walk_root: full,
            display_prefix: cleaned,
            path_filter: None,
            apply_ignore: false,
        }));
    }

    Ok(Target::Walk(WalkSpec {
        walk_root: root.to_owned(),
        display_prefix: Utf8PathBuf::new(),
        path_filter: Some(prefix_filter(&cleaned, root)),
        apply_ignore: true,
    }))
}

/// Build a partial-prefix filter from a cleaned prefix.
///
/// A prefix naming an existing directory gets a trailing separator so `docs`
/// matches entries under it without also matching a sibling `docs2`.
/// Partial filenames like `rfd/D` keep their input shape and still match.
fn prefix_filter(cleaned: &Utf8Path, root: &Utf8Path) -> String {
    let mut filter = cleaned.as_str().replace('/', std::path::MAIN_SEPARATOR_STR);
    if root.join(cleaned).is_dir() {
        filter.push_str(std::path::MAIN_SEPARATOR_STR);
    }
    filter
}

/// Apply the path-filtering configuration shared by the walk and the standalone
/// ignore matcher.
///
/// Both are configured from here so the matcher's verdict is the walk's
/// behavior: configuring them separately would let a path be reported reachable
/// and then pruned anyway, or the reverse.
fn walk_filters(builder: &mut WalkBuilder, apply_ignore: bool) -> &mut WalkBuilder {
    builder
        // Include hidden and otherwise ignored files.
        .standard_filters(false)
        .follow_links(false)
        // Respect `.ignore` files (also in parent directories).
        .ignore(apply_ignore)
        .parents(apply_ignore)
}

/// The ignore rules the workspace walk applies, queryable one path at a time.
///
/// Answers what the walk would do with a path without walking to it, so a
/// pruned path can be named in the result instead of silently vanishing.
/// Rules from `.ignore` files in subdirectories count, not just the root one:
/// the matcher loads each queried path's directory chain on demand.
struct IgnoreRules(Option<IncrementalIgnore>);

impl IgnoreRules {
    fn for_workspace(root: &Utf8Path) -> Self {
        let mut builder = WalkBuilder::new(root);
        walk_filters(&mut builder, true);
        Self(builder.build_matchers().into_iter().next())
    }

    /// Whether the walk prunes `relative`, which must be workspace-relative and
    /// free of `..` components.
    ///
    /// Reports nothing as pruned when the matcher could not be built, so a
    /// matcher failure surfaces files rather than hiding them.
    fn prunes(&mut self, relative: &Utf8Path, is_dir: bool) -> bool {
        self.0
            .as_mut()
            .is_some_and(|rules| rules.matched(relative, is_dir).is_ignore())
    }
}

/// Present an explicitly named file as a listing entry.
///
/// Bypassing the walk also bypasses its per-entry extension filter, so that is
/// applied here on the same terms.
/// Read access is settled before this point, by the caller.
fn explicit_file(cleaned: &Utf8Path, extensions: Option<&OneOrMany<String>>) -> Option<String> {
    if extensions.is_some_and(|extensions| {
        cleaned
            .extension()
            .is_some_and(|ext| !extensions.iter().any(|allowed| allowed == ext))
    }) {
        return None;
    }

    Some(cleaned.as_str().replace('/', std::path::MAIN_SEPARATOR_STR))
}

/// Walk a [`WalkSpec`] and return display paths that pass the extension,
/// prefix, and read-access filters.
///
/// Each result is `display_prefix` joined with the entry's path relative to
/// `walk_root`, so callers see workspace-relative (or mount-relative) paths.
/// When a policy is supplied, only files it grants `read` on are returned.
fn collect_files(
    spec: &WalkSpec,
    extensions: Option<&OneOrMany<String>>,
    access: Option<&AccessPolicy>,
    suppress: &Gitignore,
) -> Vec<String> {
    let walk_root = &spec.walk_root;
    let (tx, matches) = crossbeam_channel::unbounded();
    let mut builder = WalkBuilder::new(walk_root);
    walk_filters(&mut builder, spec.apply_ignore);

    // Prune suppressed paths from traversal too, so one `suppress` entry is enough
    // to keep a tree out of results — with no matching `.ignore` entry to keep in
    // sync, and no way for an `.ignore` un-ignore rule to let it back in.
    if !suppress.is_empty() {
        let suppress = suppress.clone();
        let walk_root = walk_root.to_owned();
        let display_prefix = spec.display_prefix.clone();
        builder.filter_entry(move |entry| {
            let Some(path) = Utf8Path::from_path(entry.path()) else {
                return true;
            };
            let Ok(relative) = path.strip_prefix(&walk_root) else {
                return true;
            };
            // Match the path as the caller sees it, so a pattern covering a mount
            // name reaches the mount's contents even though they live outside the
            // workspace on disk.
            let display = if display_prefix.as_str().is_empty() {
                relative.to_owned()
            } else {
                display_prefix.join(relative)
            };
            !is_suppressed(&suppress, &display)
        });
    }

    builder.build_parallel().run(|| {
        let tx = tx.clone();
        let extensions = extensions.cloned();
        let path_filter = spec.path_filter.clone();
        let display_prefix = spec.display_prefix.clone();
        Box::new(move |entry| {
            // Ignore invalid entries.
            let Ok(entry) = entry else {
                return WalkState::Continue;
            };

            // Ignore non-files.
            if entry.file_type().is_none_or(|ft| !ft.is_file()) {
                return WalkState::Continue;
            }

            // Ignore files that don't match the extension, if any.
            if extensions.as_ref().is_some_and(|extensions| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|ext| !extensions.contains(&ext.to_string_lossy().into_owned()))
            }) {
                return WalkState::Continue;
            }

            let Ok(path) = Utf8PathBuf::try_from(entry.into_path()) else {
                return WalkState::Continue;
            };

            let Ok(relative) = path.strip_prefix(walk_root) else {
                return WalkState::Continue;
            };

            // Present results under the display prefix (mount name, the
            // requested subtree, or empty for the workspace itself).
            let display = if display_prefix.as_str().is_empty() {
                relative.to_owned()
            } else {
                display_prefix.join(relative)
            };

            // Filter by partial prefix if the original prefix wasn't a directory.
            if let Some(filter) = &path_filter
                && !display.as_str().starts_with(filter.as_str())
            {
                return WalkState::Continue;
            }

            // Per-entry read enforcement: only list files the policy grants
            // read on. An absent policy lists everything (unrestricted).
            if let Some(policy) = access
                && !policy.permits(Capability::Read, &display)
            {
                return WalkState::Continue;
            }

            let _result = tx.send(display.to_string());

            WalkState::Continue
        })
    });

    drop(tx);
    matches.into_iter().collect()
}

#[cfg(test)]
#[path = "list_files_tests.rs"]
mod tests;
