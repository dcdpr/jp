use camino::{Utf8Path, Utf8PathBuf};
use ignore::{WalkBuilder, WalkState};
use jp_tool::{AccessPolicy, Capability};

use super::utils::clean_workspace_path;
use crate::{Error, util::OneOrMany};

#[derive(Debug)]
pub(crate) enum Files {
    Empty,
    List(Vec<String>),
}

impl Files {
    pub(crate) fn into_files(self) -> Vec<String> {
        match self {
            Files::Empty => vec![],
            Files::List(files) => files,
        }
    }
}

impl serde::Serialize for Files {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Files::Empty => serializer.serialize_str("No files found."),
            Files::List(files) => files.serialize(serializer),
        }
    }
}

pub(crate) async fn fs_list_files(
    root: &Utf8Path,
    access: Option<&AccessPolicy>,
    prefixes: Option<OneOrMany<String>>,
    extensions: Option<OneOrMany<String>>,
) -> std::result::Result<Files, Error> {
    let prefixes = prefixes.unwrap_or(OneOrMany::One(String::new())).into_vec();

    let mut entries = vec![];
    for prefix in &prefixes {
        let spec = walk_spec(root, prefix, access)?;
        entries.extend(collect_files(
            &spec.walk_root,
            &spec.display_prefix,
            spec.path_filter.as_deref(),
            extensions.as_ref(),
            access,
        ));
    }

    if entries.is_empty() {
        return Ok(Files::Empty);
    }

    entries.sort();
    entries.dedup();

    Ok(Files::List(entries))
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
}

/// Resolve a prefix into a [`WalkSpec`].
///
/// An empty prefix or bare `.` walks the whole workspace.
/// A prefix that names an approved external mount walks the mount's canonical
/// target (bounded by the approved target) and presents results under the mount
/// name.
/// Any other prefix scopes the workspace walk with a path filter rather than
/// re-rooting: the walk always starts at the workspace root so the root
/// `.ignore` whitelist applies consistently (its anchored patterns like
/// `docs/.vitepress/dist/` only prune when the walk is rooted at the `.ignore`
/// file).
fn walk_spec(
    root: &Utf8Path,
    prefix: &str,
    access: Option<&AccessPolicy>,
) -> std::result::Result<WalkSpec, Error> {
    if prefix.is_empty() || prefix == "." {
        return Ok(WalkSpec {
            walk_root: root.to_owned(),
            display_prefix: Utf8PathBuf::new(),
            path_filter: None,
        });
    }

    let cleaned = clean_workspace_path(root, prefix, access)?;

    // A prefix naming an approved external mount walks the mount's canonical
    // target and presents results under the mount name. `follow_links(false)`
    // keeps nested symlinks inside the target from escaping the approved
    // boundary.
    if let Some(rule) = access.and_then(|policy| policy.matching_fs_rule(&cleaned))
        && rule.external()
        && let Some(target) = rule.approved_target()
    {
        return Ok(WalkSpec {
            walk_root: target.to_owned(),
            display_prefix: rule.lexical_path().to_owned(),
            path_filter: Some(prefix_filter(&cleaned, root)),
        });
    }

    Ok(WalkSpec {
        walk_root: root.to_owned(),
        display_prefix: Utf8PathBuf::new(),
        path_filter: Some(prefix_filter(&cleaned, root)),
    })
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

/// Walk `walk_root` and return display paths that pass the extension, prefix,
/// and read-access filters.
///
/// Each result is `display_prefix` joined with the entry's path relative to
/// `walk_root`, so callers see workspace-relative (or mount-relative) paths.
/// When a policy is supplied, only files it grants `read` on are returned.
fn collect_files(
    walk_root: &Utf8Path,
    display_prefix: &Utf8Path,
    path_filter: Option<&str>,
    extensions: Option<&OneOrMany<String>>,
    access: Option<&AccessPolicy>,
) -> Vec<String> {
    let (tx, matches) = crossbeam_channel::unbounded();
    WalkBuilder::new(walk_root)
        // Include hidden and otherwise ignored files.
        .standard_filters(false)
        .follow_links(false)
        // Respect `.ignore` files (also in parent directories).
        .ignore(true)
        .parents(true)
        .build_parallel()
        .run(|| {
            let tx = tx.clone();
            let extensions = extensions.cloned();
            let path_filter = path_filter.map(str::to_owned);
            let display_prefix = display_prefix.to_owned();
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
                    entry.path().extension().is_some_and(|ext| {
                        !extensions.contains(&ext.to_string_lossy().into_owned())
                    })
                }) {
                    return WalkState::Continue;
                }

                let Ok(path) = Utf8PathBuf::try_from(entry.into_path()) else {
                    return WalkState::Continue;
                };

                let Ok(relative) = path.strip_prefix(walk_root) else {
                    return WalkState::Continue;
                };

                // Present results under the display prefix (mount name, or
                // empty for the workspace itself).
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
