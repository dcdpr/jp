//! Configuration utilities.

use std::{
    collections::HashMap,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

use camino::Utf8Path;
use glob::glob;
use indexmap::IndexMap;
use schematic::{ConfigLoader, MergeError, MergeResult, PartialConfig, TransformResult};
use tracing::{debug, error, info, trace, warn};

use crate::{
    AppConfig, BoxedError, PartialAppConfig, error::Error,
    types::extending_path::ExtendingRelativePath,
};

/// Valid file extensions for configuration files.
const VALID_CONFIG_FILE_EXTS: &[&str] = &["toml", "json", "json5", "yaml", "yml"];

/// Maximum `extends` recursion depth.
///
/// The ancestor-stack cycle check is the primary defense against runaway
/// recursion; this cap is a belt-and-braces safety net for the unlikely case
/// where path canonicalization fails and lets two logically identical paths
/// compare unequal.
const MAX_EXTENDS_DEPTH: u8 = u8::MAX;

/// DFS ancestor stack used to detect `extends` cycles and enforce a depth cap.
///
/// Each nested file load pushes its canonicalized path before recursing into
/// the file's `extends`, and pops on return.
/// Re-entry into a file already on the stack is a cycle; hitting `max_depth` is
/// a (defensive) overflow.
struct ExtendsStack {
    /// Canonicalized paths currently being loaded, outermost first.
    ancestors: Vec<PathBuf>,
    /// Hard cap on nesting depth.
    max_depth: u8,
}

impl ExtendsStack {
    /// Create an empty stack with the given depth cap.
    const fn new(max_depth: u8) -> Self {
        Self {
            ancestors: Vec::new(),
            max_depth,
        }
    }

    /// Push `canonical` onto the stack after checking for cycles and depth.
    ///
    /// Returns [`Error::ExtendsCycle`] if `canonical` is already on the stack,
    /// or [`Error::ExtendsDepthExceeded`] if pushing would exceed `max_depth`.
    fn try_push(&mut self, canonical: PathBuf) -> Result<(), Error> {
        if self.ancestors.iter().any(|p| p == &canonical) {
            let mut chain = self.ancestors.clone();
            chain.push(canonical);
            return Err(Error::ExtendsCycle { chain });
        }

        if self.ancestors.len() >= self.max_depth as usize {
            let mut chain = self.ancestors.clone();
            chain.push(canonical);
            return Err(Error::ExtendsDepthExceeded {
                limit: self.max_depth,
                chain,
            });
        }

        self.ancestors.push(canonical);
        Ok(())
    }

    /// Pop the top of the stack, unwinding one level of recursion.
    fn pop(&mut self) {
        self.ancestors.pop();
    }
}

/// Load multiple partial configurations, starting with the first.
/// Later partials override earlier ones, until one of the partials disables
/// inheritance.
///
/// # Errors
///
/// Returns an error if merging the partials fails, which returns a
/// [`schematic::MergeError`].
pub fn load_partials_with_inheritance(
    partials: Vec<PartialAppConfig>,
) -> Result<PartialAppConfig, Error> {
    // Start with an empty partial.
    let mut partial = PartialAppConfig::empty();

    // Apply all partials in reverse order (most general to most specific),
    // until we find a partial that has `inherit = false`.
    for p in partials {
        if partial.inherit.is_some_and(|v| !v) {
            break;
        }

        partial.merge(&(), p)?;
    }

    Ok(partial)
}

/// Load environment variables into a partial configuration.
///
/// # Errors
///
/// Returns an error if merging the partials fails, which returns a
/// [`schematic::MergeError`].
pub fn load_envs(mut base: PartialAppConfig) -> Result<PartialAppConfig, BoxedError> {
    trace!("Loading environment variable configuration.");
    let envs = PartialAppConfig::from_envs()?;
    base.merge(&(), envs)?;

    Ok(base)
}

/// Tries to find a configuration file in a load path.
pub fn find_file_in_load_path(
    segment: &dyn AsRef<Path>,
    load_path: &dyn AsRef<Path>,
) -> Option<PathBuf> {
    let segment = segment.as_ref();
    let load_path = load_path.as_ref();

    // Segment has to be relative to a load path.
    if segment.has_root() {
        return None;
    }

    let path = load_path.join(segment);

    // If the segment matches a file, return the path as-is.
    if path.is_file() {
        return Some(path);
    }

    // Try and find the file in the load path, trying all valid extensions.
    for ext in VALID_CONFIG_FILE_EXTS {
        let path = path.with_extension(ext);
        if !path.is_file() {
            continue;
        }

        info!(path = %path.display(), "Found configuration file in load path.");
        return Some(path);
    }

    None
}

/// Load a partial configuration from a file at `path`, if it exists.
///
/// This loads either the file directly, or tries to load a file with the same
/// name, but the extension replaced with one of the valid
/// `VALID_CONFIG_FILE_EXTS`.
///
/// # Errors
///
/// See [`resolve_extends_graph`].
pub fn load_partial_at_path<P: Into<PathBuf>>(path: P) -> Result<Option<PartialAppConfig>, Error> {
    load_partial_at_path_with_max_depth(path, MAX_EXTENDS_DEPTH)
}

/// Testable variant of [`load_partial_at_path`] that takes the `extends` depth
/// cap by parameter.
///
/// # Errors
///
/// See [`load_partial_at_path`].
fn load_partial_at_path_with_max_depth<P: Into<PathBuf>>(
    path: P,
    max_depth: u8,
) -> Result<Option<PartialAppConfig>, Error> {
    let mut stack = ExtendsStack::new(max_depth);
    let mut entries = Vec::new();

    match resolve_extends_graph(path, false, &mut stack, &mut entries) {
        Ok(()) => {}
        Err(Error::Schematic(schematic::ConfigError::MissingFile(_))) => return Ok(None),
        Err(error) => return Err(error),
    }

    let mut loader = ConfigLoader::<AppConfig>::new();
    for entry in dedup_keep_last(entries) {
        if entry.optional {
            loader.file_optional(&entry.path)?;
        } else {
            loader.file(&entry.path)?;
        }
    }

    loader.load_partial(&()).map(Some).map_err(Into::into)
}

/// Load a partial configuration from a file at `path`, walking upwards until
/// either the filesystem root or `root` is reached.
///
/// At each directory level, it attempts to load a config file with the same
/// file name (e.g. `config.toml`).
/// All found configs are merged together, with deeper (more specific) paths
/// taking precedence over shallower ones.
///
/// # Errors
///
/// Can error if file parsing fails, or if partial validation fails.
pub fn load_partial_at_path_recursive<P: Into<PathBuf>>(
    path: P,
    root: Option<&Utf8Path>,
) -> Result<Option<PartialAppConfig>, Error> {
    let path: PathBuf = path.into();

    // Extract the file name component (e.g. `config.toml`) that we'll look
    // for at every ancestor directory.
    let Some(file_name) = path.file_name().map(OsStr::to_os_string) else {
        return load_partial_at_path(&path).map(|p| p.filter(|_| path.is_file()));
    };

    // Collect candidate paths from deepest to shallowest.
    //
    // Uses `Path::parent()` to walk up the tree instead of manual iterator
    // manipulation, which avoids an infinite loop on Windows where
    // `Prefix("C:")` and `RootDir("\\"`) are separate components in
    // `Path::iter()` — stripping the root dir leaves the prefix, and
    // re-joining with the file name recreates the original absolute path.
    let mut candidates = vec![path.clone()];
    let mut dir = path.parent();

    while let Some(current) = dir {
        // Stop if we've reached the configured root.
        if root.is_some_and(|root| root == current) {
            break;
        }

        let Some(parent) = current.parent() else {
            break;
        };

        candidates.push(parent.join(&file_name));
        dir = Some(parent);
    }

    // Load and merge from shallowest to deepest, so that deeper (more specific)
    // paths take precedence.
    let mut result: Option<PartialAppConfig> = None;

    for candidate in candidates.into_iter().rev() {
        let partial = load_partial_at_path(&candidate)?;

        result = match (result, partial) {
            (Some(mut base), Some(specific)) => {
                base.merge(&(), specific)?;
                Some(base)
            }
            (base, specific) => base.or(specific),
        };
    }

    Ok(result)
}

/// Build a final configuration from merged partial configurations.
///
/// # Errors
///
/// Can error if partial validation fails.
pub fn build(partial: PartialAppConfig) -> Result<AppConfig, Error> {
    debug!("Loading configuration.");
    trace!(
        config = %trace_to_tmpfile("jp-config", &partial),
        "Configuration details."
    );

    for (name, tool) in &partial.conversation.tools.tools {
        if tool.source.is_none() {
            tracing::error!(
                tool = %name,
                "Tool config is missing required `source` field."
            );
        }
    }

    let mut config = AppConfig::from_partial_with_defaults(partial)?;

    // Resolve model aliases so downstream code can assume all model IDs are
    // concrete `ModelIdOrAliasConfig::Id` variants.
    config.resolve_aliases()?;

    // Sort instructions by position.
    config.assistant.instructions.sort_by_key(|a| a.position);

    // Sort sections by position.
    config
        .assistant
        .system_prompt_sections
        .sort_by_key(|a| a.position);

    Ok(config)
}

/// One config file in a resolved `extends` graph.
struct ExtendsEntry {
    /// Path to the file, with a valid extension resolved.
    path: PathBuf,

    /// Canonicalized path, used to recognize the same file reached through more
    /// than one `extends` branch.
    canonical: PathBuf,

    /// Whether the loader tolerates the file going missing.
    ///
    /// True for every file reached through `extends`, false for the file the
    /// walk started from.
    optional: bool,
}

/// Walk the `extends` graph rooted at `path`, collecting every file to load in
/// merge order (least specific first).
///
/// A file reached through several branches appears once per branch; use
/// [`dedup_keep_last`] to reduce the result to one entry per file.
///
/// If the file does not exist, the same file name is retried with each of the
/// valid `VALID_CONFIG_FILE_EXTS` extensions.
///
/// # Errors
///
/// Returns [`Error::ExtendsCycle`] if a file extends itself through any chain,
/// [`Error::ExtendsDepthExceeded`] if nesting exceeds `stack`'s cap, and a
/// [`schematic::ConfigError::MissingFile`] if `path` does not resolve to a
/// file.
/// Parse failures in any visited file propagate.
fn resolve_extends_graph<P: Into<PathBuf>>(
    path: P,
    optional: bool,
    stack: &mut ExtendsStack,
    entries: &mut Vec<ExtendsEntry>,
) -> Result<(), Error> {
    let mut path: PathBuf = path.into();

    trace!(path = %path.display(), "Trying to open configuration file.");
    let found = path.is_file()
        || VALID_CONFIG_FILE_EXTS.iter().any(|ext| {
            path.set_extension(ext);
            path.is_file()
        });

    if !found {
        return Err(Error::Schematic(schematic::ConfigError::MissingFile(path)));
    }

    info!(path = %path.display(), "Found configuration file.");

    // Canonicalize so that `./a.toml` and `a.toml` compare equal. If
    // canonicalization fails we fall back to the raw path; the depth cap in
    // `ExtendsStack` protects against cycles slipping through in that case.
    let canonical = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
    stack.try_push(canonical.clone())?;
    let result = resolve_extends_of(&path, canonical, optional, stack, entries);
    stack.pop();
    result
}

/// Resolve the `extends` declarations of the file at `path`, then record the
/// file itself between its `before` and `after` extensions.
fn resolve_extends_of(
    path: &Path,
    canonical: PathBuf,
    optional: bool,
    stack: &mut ExtendsStack,
    entries: &mut Vec<ExtendsEntry>,
) -> Result<(), Error> {
    let root = path.parent().map(Path::to_path_buf);

    let (before, after): (Vec<_>, Vec<_>) = ConfigLoader::<AppConfig>::new()
        .file(path)?
        .load_partial(&())?
        .extends
        .into_iter()
        .flatten()
        .partition(ExtendingRelativePath::is_before);

    resolve_extended_paths(before, root.as_deref(), stack, entries)?;

    entries.push(ExtendsEntry {
        path: path.to_path_buf(),
        canonical,
        optional,
    });

    resolve_extended_paths(after, root.as_deref(), stack, entries)?;

    Ok(())
}

/// Resolve a list of `extends` declarations, expanding globs.
fn resolve_extended_paths(
    extends: impl IntoIterator<Item = ExtendingRelativePath>,
    root: Option<&Path>,
    stack: &mut ExtendsStack,
    entries: &mut Vec<ExtendsEntry>,
) -> Result<(), Error> {
    for path in extends {
        let Some(root) = &root else {
            continue;
        };

        let path = path.to_logical_path(root);
        let Some(path_str) = path.as_os_str().to_str() else {
            continue;
        };

        // Path without glob patterns, warn if it is not a file.
        if !path_str.contains(['*', '?', '[']) && !path.is_file() {
            warn!(path = %path.display(), "Unable to extend with non-existing file");
            continue;
        }

        for entry in glob(path_str)? {
            let path = match entry {
                Ok(path) => path,
                Err(error) => {
                    error!(path = %path.display(), error = %error, "Unable to read glob pattern");
                    continue;
                }
            };

            resolve_extends_graph(&path, true, stack, entries)?;
        }
    }

    Ok(())
}

/// Reduce a resolved `extends` graph to one entry per file, keeping each file's
/// last position.
///
/// A file reached through two `extends` branches is loaded once.
/// Loading it twice re-applies `append` and `prepend` merge strategies,
/// duplicating prompt text, sections, instructions, and attachments.
///
/// The last position is kept rather than the first so that an `extends` array
/// keeps its declared precedence: in `extends = ["a.toml", "b.toml"]` where
/// `a.toml` also extends `b.toml`, `b.toml`'s values still override `a.toml`'s.
fn dedup_keep_last(entries: Vec<ExtendsEntry>) -> Vec<ExtendsEntry> {
    let mut last_index: HashMap<PathBuf, usize> = HashMap::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        last_index.insert(entry.canonical.clone(), index);
    }

    entries
        .into_iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            if last_index.get(&entry.canonical) == Some(&index) {
                return Some(entry);
            }

            debug!(
                path = %entry.path.display(),
                "Skipping repeat visit to extended configuration file."
            );
            None
        })
        .collect()
}

/// Order-preserving dedup for use as `transform = vec_dedup`.
#[expect(clippy::trivially_copy_pass_by_ref, clippy::unnecessary_wraps)]
pub(crate) fn vec_dedup<T: PartialEq>(v: Vec<T>, _: &()) -> TransformResult<Vec<T>> {
    let mut seen = Vec::with_capacity(v.len());
    for item in v {
        if !seen.contains(&item) {
            seen.push(item);
        }
    }
    Ok(seen)
}

/// Merge [`IndexMap`]s of nested [`PartialConfig`]s.
///
/// # Errors
///
/// Returns an error if merging the partials fails, which returns a
/// [`schematic::MergeError`].
pub fn merge_nested_indexmap<V, C>(
    prev: IndexMap<String, V>,
    mut next: IndexMap<String, V>,
    c: &C,
) -> MergeResult<IndexMap<String, V>>
where
    V: PartialConfig<Context = C>,
    C: Default,
{
    let mut prev = prev
        .into_iter()
        .map(|(name, mut prev)| {
            if let Some(next) = next.shift_remove(&name) {
                prev.merge(c, next).map_err(MergeError::new)?;
            }

            Ok((name, prev))
        })
        .collect::<Result<IndexMap<_, _>, _>>()?;

    prev.append(&mut next);
    Ok(Some(prev))
}

/// Define the name to serialize and deserialize for a unit variant.
#[macro_export]
macro_rules! named_unit_variant {
    ($variant:ident) => {
        $crate::named_unit_variant!(stringify!($variant), $variant);
    };
    ($variant:expr, $mod:ident) => {
        pub mod $mod {
            pub fn serialize<S>(serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                serializer.serialize_str($variant)
            }

            pub fn deserialize<'de, D>(deserializer: D) -> Result<(), D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                struct V;
                impl<'de> serde::de::Visitor<'de> for V {
                    type Value = ();

                    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        f.write_str(concat!("\"", $variant, "\""))
                    }

                    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                        if value == $variant {
                            Ok(())
                        } else {
                            Err(E::invalid_value(serde::de::Unexpected::Str(value), &self))
                        }
                    }
                }

                deserializer.deserialize_str(V)
            }
        }
    };
}

/// Serialize a value to a temporary JSON file and return its path as a string.
///
/// Used by `trace!` fields to avoid dumping massive payloads into the log
/// stream.
/// The file is written to `std::env::temp_dir()` with the given `prefix`.
/// Returns `"<write failed>"` on I/O errors.
fn trace_to_tmpfile(prefix: &str, value: &impl serde::Serialize) -> String {
    let path = std::env::temp_dir().join(format!("{prefix}-{}.json", std::process::id()));
    match std::fs::write(
        &path,
        serde_json::to_string_pretty(value).unwrap_or_default(),
    ) {
        Ok(()) => path.display().to_string(),
        Err(_) => "<write failed>".to_owned(),
    }
}

#[cfg(test)]
pub(crate) struct EnvVarGuard {
    name: String,
    original_value: Option<String>,
}

#[cfg(test)]
impl EnvVarGuard {
    pub fn set(name: &str, value: &str) -> Self {
        let name = name.to_string();
        let original_value = std::env::var(&name).ok();
        unsafe { std::env::set_var(&name, value) };
        Self {
            name,
            original_value,
        }
    }
}

#[cfg(test)]
impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(ref original) = self.original_value {
            unsafe { std::env::set_var(&self.name, original) };
        } else {
            unsafe { std::env::remove_var(&self.name) };
        }
    }
}

#[cfg(test)]
#[path = "util_tests.rs"]
mod tests;
