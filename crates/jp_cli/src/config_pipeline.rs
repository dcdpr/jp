//! Two-phase config loading pipeline.
//!
//! Config sources are loaded from disk once and cached. The pipeline can then
//! produce partial configs with or without the per-conversation layer, ensuring
//! correct precedence in both cases:
//!
//! - **Pre-resolution** (for `conversation.default_id`): `files + env + --cfg`
//! - **Final**: `files + env + conversation + --cfg`
//!
//! Command-specific CLI overrides (`apply_cli_config`) are applied by the
//! caller after each build — they're not part of the pipeline because they
//! depend on the specific command struct.

use camino::Utf8PathBuf;
use jp_config::{
    PartialAppConfig,
    assignment::{AssignKeyValue as _, KvAssignment},
    fs::{load_partial, user_global_config_path},
    util::{find_file_in_load_path, load_partial_at_path},
};
use jp_storage::backend::FsStorageBackend;
use jp_workspace::Workspace;
use relative_path::RelativePath;
use tracing::{debug, error};

use super::KeyValueOrPath;
use crate::error::{Error, Result};

/// A `--cfg` argument with file contents already resolved from disk.
///
/// File I/O happens once during [`resolve_cfg_args`], and the results are
/// reused for each build.
#[derive(Debug, Clone)]
enum ResolvedCfgArg {
    /// A key=value assignment (e.g. `--cfg conversation.default_id=last`).
    KeyValue(KvAssignment),

    /// One or more partials loaded from a config file path.
    Partials(Vec<PartialAppConfig>),
}

/// Config sources loaded once from disk, reusable for multiple builds.
///
/// Owns the static config layers (files, env vars, `--cfg` args). The
/// per-conversation layer and command-specific CLI overrides are provided at
/// build time by the caller.
pub(crate) struct ConfigPipeline {
    /// Files + inheritance + env vars, merged into a single partial.
    base: PartialAppConfig,

    /// `--cfg` args resolved from disk (files loaded, paths searched).
    cfg_args: Vec<ResolvedCfgArg>,
}

impl ConfigPipeline {
    /// Build a pipeline from the workspace and CLI `--cfg` overrides.
    ///
    /// This is the only place where config files and `--cfg` file args are read
    /// from disk.
    pub fn new(
        base: PartialAppConfig,
        overrides: &[KeyValueOrPath],
        workspace: Option<&Workspace>,
        fs: Option<&FsStorageBackend>,
    ) -> Result<Self> {
        let cfg_args = resolve_cfg_args(overrides, &base, workspace, fs)?;
        Ok(Self { base, cfg_args })
    }

    /// Build a partial with `--cfg` applied on top of the base.
    ///
    /// No per-conversation layer. Used for pre-resolution reads like
    /// `conversation.default_id`.
    pub fn partial_without_conversation(&self) -> Result<PartialAppConfig> {
        apply_cfg_args(self.base.clone(), &self.cfg_args)
    }

    /// Build a partial with the per-conversation layer sandwiched between
    /// the base and `--cfg`, maintaining correct precedence:
    /// `files + env < conversation < --cfg`.
    pub fn partial_with_conversation(
        &self,
        conversation: PartialAppConfig,
    ) -> Result<PartialAppConfig> {
        let mut partial = load_partial(self.base.clone(), conversation)?;
        partial = apply_cfg_args(partial, &self.cfg_args)?;
        Ok(partial)
    }
}

/// Resolve `--cfg` arguments into their in-memory representations.
///
/// File paths are searched and loaded from disk here, exactly once. Key-value
/// assignments are stored as-is.
fn resolve_cfg_args(
    overrides: &[KeyValueOrPath],
    base: &PartialAppConfig,
    workspace: Option<&Workspace>,
    fs: Option<&FsStorageBackend>,
) -> Result<Vec<ResolvedCfgArg>> {
    let home = std::env::home_dir().and_then(|p| Utf8PathBuf::from_path_buf(p).ok());
    let mut resolved = Vec::with_capacity(overrides.len());

    for field in overrides {
        match field {
            KeyValueOrPath::Path(path) if path.exists() => {
                let mut partials = Vec::new();
                if let Some(p) = load_partial_at_path(path)? {
                    partials.push(p);
                }
                if !partials.is_empty() {
                    resolved.push(ResolvedCfgArg::Partials(partials));
                }
            }
            KeyValueOrPath::Path(path) => {
                // Build search roots in precedence order (lowest first).
                //
                // 1. User-global:    $XDG_CONFIG_HOME/jp/config/
                // 2. Workspace:      <workspace_root>/
                // 3. User-workspace: $XDG_DATA_HOME/jp/workspace/<id>/config/
                let mut roots: Vec<Utf8PathBuf> = Vec::new();

                if let Some(global_dir) = user_global_config_path(home.as_deref()) {
                    roots.push(global_dir.join("config"));
                }
                if let Some(w) = workspace {
                    roots.push(w.root().to_owned());
                }
                if let Some(path) =
                    fs.and_then(|f| f.user_storage_with_path(RelativePath::new("config")))
                {
                    roots.push(path);
                }

                let mut matches: Vec<PartialAppConfig> = Vec::new();
                let mut searched: Vec<Utf8PathBuf> = Vec::new();

                // Search each root independently. Within a single root, the
                // first `config_load_paths` entry that produces a match wins.
                // Across roots, all matches are collected for merging.
                for root in &roots {
                    let load_paths: Vec<Utf8PathBuf> = base
                        .config_load_paths
                        .iter()
                        .flatten()
                        .filter_map(|p| {
                            Utf8PathBuf::try_from(p.to_path(root))
                                .inspect_err(|e| {
                                    error!(
                                        path = p.to_string(),
                                        error = e.to_string(),
                                        "Not a valid UTF-8 path"
                                    );
                                })
                                .ok()
                        })
                        .collect();

                    for load_path in &load_paths {
                        searched.push(load_path.clone());

                        debug!(
                            path = path.as_str(),
                            load_path = load_path.as_str(),
                            root = root.as_str(),
                            "Trying to load partial from config load path"
                        );

                        if let Some(file) = find_file_in_load_path(path, load_path) {
                            if let Some(p) = load_partial_at_path(file)? {
                                matches.push(p);
                            }

                            break; // first match within this root
                        }
                    }
                }

                if matches.is_empty() {
                    return Err(Error::MissingConfigFile {
                        path: path.clone(),
                        searched,
                    });
                }

                resolved.push(ResolvedCfgArg::Partials(matches));
            }
            KeyValueOrPath::KeyValue(kv) => {
                resolved.push(ResolvedCfgArg::KeyValue(kv.clone()));
            }
        }
    }

    Ok(resolved)
}

/// Apply pre-resolved `--cfg` args onto a partial config. Pure in-memory merge.
fn apply_cfg_args(
    mut partial: PartialAppConfig,
    args: &[ResolvedCfgArg],
) -> Result<PartialAppConfig> {
    for arg in args {
        match arg {
            ResolvedCfgArg::KeyValue(kv) => {
                partial
                    .assign(kv.clone())
                    .map_err(|e| Error::CliConfig(e.to_string()))?;
            }
            ResolvedCfgArg::Partials(partials) => {
                for p in partials {
                    partial = load_partial(partial, p.clone())?;
                }
            }
        }
    }

    Ok(partial)
}

/// Build a [`PartialAppConfig`] from raw `--cfg` arguments, using the same
/// search-path resolution as the config pipeline.
///
/// Used by `config set` to persist `--cfg` values to a file or conversation.
pub(crate) fn build_partial_from_cfg_args(
    args: &[KeyValueOrPath],
    base: &PartialAppConfig,
    workspace: Option<&Workspace>,
    fs: Option<&FsStorageBackend>,
) -> Result<PartialAppConfig> {
    if args.is_empty() {
        return Err(Error::CliConfig(
            "No configuration values to set. Use `--cfg` to specify values.".into(),
        ));
    }

    let resolved = resolve_cfg_args(args, base, workspace, fs)?;
    apply_cfg_args(PartialAppConfig::empty(), &resolved)
}

#[cfg(test)]
#[path = "config_pipeline_tests.rs"]
mod tests;
