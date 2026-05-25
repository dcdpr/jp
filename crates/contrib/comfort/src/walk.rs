//! File discovery for workspace and path-based invocations.

use std::path::{Path, PathBuf};

use cargo_metadata::{MetadataCommand, Package};
use ignore::WalkBuilder;

use crate::{Error, cli::Language};

/// Discover files inside the current cargo workspace, honoring `.gitignore` and
/// friends, filtering by `language`.
/// Returns paths in walker order.
///
/// `include` and `exclude` further filter the workspace by package name.
/// When both are empty, every workspace package is walked.
/// When `include` is non-empty, only those packages are walked.
/// `exclude` always removes packages from the resulting set.
/// Either list having an unknown name produces [`Error::UnknownPackage`].
pub fn workspace_files(
    include: &[String],
    exclude: &[String],
    language: Language,
) -> Result<Vec<PathBuf>, Error> {
    let metadata = MetadataCommand::new().no_deps().exec()?;

    if include.is_empty() && exclude.is_empty() {
        return walk_files(metadata.workspace_root.as_std_path(), language);
    }

    let workspace_packages = metadata.workspace_packages();
    let available: Vec<&str> = workspace_packages.iter().map(|p| p.name.as_str()).collect();

    validate_package_names(&available, include)?;
    validate_package_names(&available, exclude)?;

    let selected = select_packages(&workspace_packages, include, exclude);

    let mut files = Vec::new();
    for pkg in selected {
        let Some(dir) = pkg.manifest_path.parent() else {
            continue;
        };
        files.extend(walk_files(dir.as_std_path(), language)?);
    }
    Ok(files)
}

/// Walk a single directory or accept a single file path.
/// Files are returned as-is (even if their extension doesn't match `language`)
/// — the caller asked for them by name.
/// Directories are walked, respecting `.gitignore`, and filtered by `language`.
/// Returns [`Error::Walk`] for walker errors (unreadable directory, symlink
/// loop, etc.) so a `--check --workspace` run can't silently exit 0 without
/// having inspected every file it was supposed to cover.
pub fn expand_path(input: &Path, language: Language) -> Result<Vec<PathBuf>, Error> {
    if input.is_dir() {
        walk_files(input, language)
    } else {
        Ok(vec![input.to_path_buf()])
    }
}

fn walk_files(root: &Path, language: Language) -> Result<Vec<PathBuf>, Error> {
    let mut out = Vec::new();
    for entry in WalkBuilder::new(root).standard_filters(true).build() {
        let entry = entry?;
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.into_path();
        if matches_language(&path, language) {
            out.push(path);
        }
    }
    Ok(out)
}

/// True if a discovered file's extension falls inside the set selected by the
/// given language.
/// With [`Language::Auto`], both Rust and Markdown extensions are included;
/// with an explicit language, only that one's.
fn matches_language(path: &Path, language: Language) -> bool {
    let ext = path.extension().and_then(|e| e.to_str());
    matches!(
        (language, ext),
        (Language::Auto, Some("rs" | "md" | "markdown"))
            | (Language::Rust, Some("rs"))
            | (Language::Markdown, Some("md" | "markdown"))
    )
}

/// Apply include/exclude filters to a list of workspace packages.
fn select_packages<'a>(
    packages: &'a [&'a Package],
    include: &[String],
    exclude: &[String],
) -> Vec<&'a Package> {
    packages
        .iter()
        .filter(|p| should_include(p.name.as_str(), include, exclude))
        .copied()
        .collect()
}

/// Returns true if a package with the given name should be included given the
/// user's `-p`/`--exclude` selection.
/// Pure; extracted so the resolution logic can be tested without constructing
/// `cargo_metadata` types.
fn should_include(name: &str, include: &[String], exclude: &[String]) -> bool {
    let included = include.is_empty() || include.iter().any(|n| n == name);
    let excluded = exclude.iter().any(|n| n == name);
    included && !excluded
}

/// Confirm every name in `names` matches some entry in `available`.
/// Returns [`Error::UnknownPackage`] for the first mismatch — fail-fast on
/// typos.
fn validate_package_names(available: &[&str], names: &[String]) -> Result<(), Error> {
    for name in names {
        if !available.iter().any(|a| a == name) {
            return Err(Error::UnknownPackage(name.clone()));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "walk_tests.rs"]
mod tests;
