//! Two-phase config loading pipeline.
//!
//! Config sources are loaded from disk once and cached.
//! The pipeline can then produce partial configs with or without the
//! per-conversation layer, ensuring correct precedence in both cases:
//!
//! - **Pre-resolution** (for `conversation.default_id`): `files + env + --cfg`
//! - **Final**: `files + env + conversation + --cfg`
//!
//! Command-specific CLI overrides (`apply_cli_config`) are applied by the
//! caller after each build — they're not part of the pipeline because they
//! depend on the specific command struct.

use std::path::Path;

use camino::Utf8PathBuf;
use jp_config::{
    PartialAppConfig,
    assignment::{AssignKeyValue as _, KvAssignment},
    fs::{load_partial, user_global_config_dir},
    loader::PartialLoaderConfig,
    util::{find_file_in_load_path, load_loader_directives, load_partial_at_path},
};
use jp_storage::backend::FsStorageBackend;
use jp_workspace::Workspace;
use relative_path::RelativePath;
use tracing::{debug, error};

use super::{CfgKeyword, KeyValueOrPath};
use crate::error::{Error, Result};

/// A config reset point encountered in the `--cfg` directive stream.
///
/// Reset points share one mechanism: discard the accumulated config state, then
/// layer a known state on top ([RFD 038]).
///
/// [RFD 038]: https://jp.computer/rfd/038
#[derive(Debug, Clone)]
pub(crate) enum ConfigReset {
    /// `--cfg=NONE`: reset to program defaults.
    Defaults,

    /// `--cfg=WORKSPACE`: reset to the workspace's fully-resolved config.
    ///
    /// Carries the workspace partial as it resolved at invocation time, so the
    /// reset is value-stable once persisted.
    Workspace(Box<PartialAppConfig>),
}

impl ConfigReset {
    /// The state this reset point returns the accumulated config to.
    ///
    /// Program defaults live in the empty partial (they are injected when a
    /// partial is finalized into an `AppConfig`), so `NONE` resets to the empty
    /// partial and `WORKSPACE` to the workspace partial.
    pub fn state(&self) -> PartialAppConfig {
        match self {
            Self::Defaults => PartialAppConfig::default(),
            Self::Workspace(workspace) => (**workspace).clone(),
        }
    }
}

/// The reset a continuing conversation must persist into its event stream.
///
/// Assembled by `resolve_config` when the `--cfg` list contains a reset
/// keyword; consumed by the query command, which appends the corresponding
/// `ConfigDelta` events ([RFD 038]):
///
/// 1. `Reset` (both keywords),
/// 2. `Apply(workspace partial)` (`WORKSPACE` only),
/// 3. `Apply(post)` (whatever the invocation layered on top of the reset point,
///    if anything).
///
/// The `post` partial is a partial-level diff from the reset point's state to
/// the invocation's final partial, so it captures post-keyword `--cfg`
/// directives and command CLI overrides without pinning program defaults.
/// It is computed directly instead of routing through the empty-diff
/// suppression path, because that path resolves the stream's current config —
/// which is not a valid configuration between a `Reset` and whichever `Apply`
/// restores the required fields.
///
/// [RFD 038]: https://jp.computer/rfd/038
#[derive(Debug, Clone)]
pub(crate) struct ConfigResetEvents {
    /// The reset point itself.
    pub reset: ConfigReset,

    /// State layered on top of the reset point by this invocation.
    pub post: Box<PartialAppConfig>,
}

/// Presence of reserved `--cfg` keywords, detected by [`scan_cfg_keywords`].
#[derive(Debug, Clone, Copy, Default)]
struct CfgKeywords {
    /// An exact `NONE` value appears in the `--cfg` list.
    pub none: bool,

    /// An exact `WORKSPACE` value appears in the `--cfg` list.
    pub workspace: bool,
}

/// Pre-scan the `--cfg` list for reset keywords.
///
/// This runs in [`ConfigPipeline::new`] before any directive is processed:
/// `NONE` gates implicit config loading, and the `NONE`/`WORKSPACE` combination
/// is rejected independent of position — `NONE` skips the implicit-loading
/// step that `WORKSPACE` expands to, so combining them is internally
/// inconsistent.
fn scan_cfg_keywords(overrides: &[KeyValueOrPath]) -> Result<CfgKeywords> {
    let mut keywords = CfgKeywords::default();
    for field in overrides {
        match field {
            KeyValueOrPath::Keyword(CfgKeyword::None) => keywords.none = true,
            KeyValueOrPath::Keyword(CfgKeyword::Workspace) => keywords.workspace = true,
            _ => {}
        }
    }

    if keywords.none && keywords.workspace {
        return Err(Error::CliConfig(
            "--cfg=NONE and --cfg=WORKSPACE are mutually exclusive.".into(),
        ));
    }

    Ok(keywords)
}

/// A `--cfg` argument with file contents already resolved from disk.
///
/// File I/O happens once during [`resolve_cfg_args`], and the results are
/// reused for each build.
#[derive(Debug, Clone)]
enum ResolvedCfgArg {
    /// A key=value assignment (e.g. `--cfg conversation.default_id=last`).
    KeyValue(KvAssignment),

    /// One or more config entries loaded from a config file path.
    ///
    /// A single `--cfg` argument can resolve to multiple entries across search
    /// roots; they apply in root precedence order.
    Partials(Vec<CfgEntry>),

    /// A reset keyword (`NONE` or `WORKSPACE`).
    Reset(ConfigReset),
}

/// A config entry resolved from an explicit `--cfg` file argument.
#[derive(Debug, Clone)]
struct CfgEntry {
    /// The entry declares `loader.reset = "none"` in its own `[loader]`
    /// section.
    ///
    /// The declaration is read shallowly from the entry file itself —
    /// `[loader]` in a file reached through `extends` is ignored ([RFD 038]).
    ///
    /// [RFD 038]: https://jp.computer/rfd/038
    reset: bool,

    /// The entry's partial, resolved including its `extends` tree.
    partial: PartialAppConfig,
}

/// Load a `--cfg` file entry from `path`.
///
/// Combines the fully-resolved partial (including the `extends` tree) with the
/// loader directives read shallowly from the entry file's own `[loader]`
/// section.
fn load_cfg_entry<P: AsRef<Path>>(path: P) -> Result<Option<CfgEntry>> {
    let path = path.as_ref();
    let Some(mut partial) = load_partial_at_path(path)? else {
        return Ok(None);
    };

    let reset = load_loader_directives(path)?.reset.is_some();

    // Loader metadata is load-time-only ([RFD 038]): its effect is captured
    // in `reset`, and the section itself (which here may also carry values
    // merged in from `extends`-reached files) must not travel past the
    // pipeline into resolved or persisted state.
    partial.loader = PartialLoaderConfig::default();

    Ok(Some(CfgEntry { reset, partial }))
}

/// Config sources loaded once from disk, reusable for multiple builds.
///
/// Owns the static config layers (files, env vars, `--cfg` args).
/// The per-conversation layer and command-specific CLI overrides are provided
/// at build time by the caller.
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
    /// from disk, and it owns the implicit-loading decision ([RFD 038]): the
    /// `--cfg` list is pre-scanned for reset keywords (rejecting the mutually
    /// exclusive `NONE`/`WORKSPACE` combination), and `load_base` — providing
    /// the files + inheritance + env layer — is invoked only when no `NONE`
    /// keyword is present.
    /// The gate is decided before any config file I/O, which is what keeps
    /// `NONE` usable as an escape hatch when implicit config is broken.
    ///
    /// [RFD 038]: https://jp.computer/rfd/038
    pub fn new(
        overrides: &[KeyValueOrPath],
        workspace: Option<&Workspace>,
        fs: Option<&FsStorageBackend>,
        load_base: impl FnOnce() -> Result<PartialAppConfig>,
    ) -> Result<Self> {
        let keywords = scan_cfg_keywords(overrides)?;

        // `NONE` gate: skip implicit loading entirely and start from program
        // defaults (the empty partial; defaults are injected when the partial
        // is finalized into an `AppConfig`).
        let mut base = if keywords.none {
            PartialAppConfig::default()
        } else {
            load_base()?
        };

        // `[loader]` steers how an explicit `--cfg` entry is loaded; in
        // implicitly-loaded config it has no effect, and it must not linger
        // in the base state that `WORKSPACE` resets capture and new
        // conversations persist ([RFD 038]).
        base.loader = PartialLoaderConfig::default();

        let cfg_args = resolve_cfg_args(overrides, &base, workspace, fs)?;
        Ok(Self { base, cfg_args })
    }

    /// Build a partial with `--cfg` applied on top of the base.
    ///
    /// No per-conversation layer.
    /// Used for pre-resolution reads like `conversation.default_id`.
    pub fn partial_without_conversation(&self) -> Result<PartialAppConfig> {
        apply_cfg_args(self.base.clone(), &self.cfg_args)
    }

    /// Build a partial with the per-conversation layer sandwiched between the
    /// base and `--cfg`, maintaining correct precedence: `files + env <
    /// conversation < --cfg`.
    pub fn partial_with_conversation(
        &self,
        conversation: PartialAppConfig,
    ) -> Result<PartialAppConfig> {
        let mut partial = load_partial(self.base.clone(), conversation)?;
        partial = apply_cfg_args(partial, &self.cfg_args)?;
        Ok(partial)
    }

    /// The effective reset point of this invocation, if any.
    ///
    /// A reset discards everything accumulated before it, so only the last
    /// reset point in the directive stream is effective.
    ///
    /// Reset points come from the `NONE`/`WORKSPACE` keywords and from file
    /// entries declaring `loader.reset = "none"` — the latter is the
    /// entry-local equivalent of `--cfg=NONE` immediately before the entry, so
    /// it maps to a reset to program defaults ([RFD 038]).
    pub fn config_reset(&self) -> Option<ConfigReset> {
        self.cfg_args.iter().rev().find_map(|arg| match arg {
            ResolvedCfgArg::Reset(reset) => Some(reset.clone()),
            ResolvedCfgArg::Partials(entries) if entries.iter().any(|e| e.reset) => {
                Some(ConfigReset::Defaults)
            }
            _ => None,
        })
    }
}

/// Resolve `--cfg` arguments into their in-memory representations.
///
/// File paths are searched and loaded from disk here, exactly once.
/// Key-value assignments are stored as-is.
fn resolve_cfg_args(
    overrides: &[KeyValueOrPath],
    base: &PartialAppConfig,
    workspace: Option<&Workspace>,
    fs: Option<&FsStorageBackend>,
) -> Result<Vec<ResolvedCfgArg>> {
    let home = std::env::home_dir().and_then(|p| Utf8PathBuf::from_path_buf(p).ok());
    let mut resolved = Vec::with_capacity(overrides.len());

    // A `NONE` keyword positionally discards everything before it, so the
    // directive loop skips processing pre-`NONE` values entirely: their
    // file-load and merge effects are not executed ([RFD 038]).
    // This keeps `NONE` usable as an escape hatch even when an earlier `--cfg`
    // value references a broken or missing file.
    //
    // `WORKSPACE` gets no such exemption: pre-`WORKSPACE` values are processed
    // normally (a missing file still errors), and their contribution is
    // discarded by the reset during the merge.
    //
    // [RFD 038]: https://jp.computer/rfd/038
    let skip_until = overrides
        .iter()
        .rposition(|f| matches!(f, KeyValueOrPath::Keyword(CfgKeyword::None)))
        .unwrap_or_default();

    for field in &overrides[skip_until..] {
        match field {
            KeyValueOrPath::Path(path) if path.exists() => {
                let mut entries = Vec::new();
                if let Some(entry) = load_cfg_entry(path.as_std_path())? {
                    entries.push(entry);
                }
                if !entries.is_empty() {
                    resolved.push(ResolvedCfgArg::Partials(entries));
                }
            }
            KeyValueOrPath::Path(path) => {
                // Build search roots in precedence order (lowest first).
                //
                // 1. User-global:    $XDG_CONFIG_HOME/jp/config/
                // 2. Workspace:      <workspace_root>/
                // 3. User-workspace: $XDG_DATA_HOME/jp/workspace/<id>/config/
                let mut roots: Vec<Utf8PathBuf> = Vec::new();

                if let Some(global_dir) = user_global_config_dir(home.as_deref()) {
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

                let mut matches: Vec<CfgEntry> = Vec::new();
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
                            if let Some(entry) = load_cfg_entry(&file)? {
                                matches.push(entry);
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
            KeyValueOrPath::Keyword(CfgKeyword::None) => {
                resolved.push(ResolvedCfgArg::Reset(ConfigReset::Defaults));
            }
            KeyValueOrPath::Keyword(CfgKeyword::Workspace) => {
                // `base` is the workspace's fully-resolved config: when
                // `WORKSPACE` appears, the `NONE` gate is off (the keywords
                // are mutually exclusive), so implicit loading ran.
                resolved.push(ResolvedCfgArg::Reset(ConfigReset::Workspace(Box::new(
                    base.clone(),
                ))));
            }
        }
    }

    Ok(resolved)
}

/// Apply pre-resolved `--cfg` args onto a partial config.
/// Pure in-memory merge.
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
            ResolvedCfgArg::Partials(entries) => {
                for entry in entries {
                    if entry.reset {
                        // Entry-local reset ([RFD 038]): equivalent to
                        // `--cfg=NONE` immediately before this entry.
                        // Discards the accumulated state — including earlier
                        // entries resolved from the same argument.
                        partial = PartialAppConfig::default();
                    }
                    partial = load_partial(partial, entry.partial.clone())?;
                }
            }
            ResolvedCfgArg::Reset(reset) => {
                // Reset-then-layer: discard the accumulated state (including
                // the base and per-conversation layers), restart from the
                // reset point's state, and let subsequent args layer on top.
                partial = reset.state();
            }
        }
    }

    // Defensive cleanup: loader metadata must not leak into resolved or
    // persisted state ([RFD 038]). No route writes it here today —
    // assignments reject `loader.*` as an unknown key (see
    // `loader_assignment_is_rejected`), and file entries strip their own
    // `[loader]` section at load time in `load_cfg_entry` — so this strip
    // only guards future routes that forget to.
    partial.loader = PartialLoaderConfig::default();

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

    // Reset keywords describe a state transition in a conversation's config
    // stream; they have no meaning as a value to persist.
    if args.iter().any(|a| matches!(a, KeyValueOrPath::Keyword(_))) {
        return Err(Error::CliConfig(
            "--cfg reset keywords (NONE, WORKSPACE) are not supported here.".into(),
        ));
    }

    let resolved = resolve_cfg_args(args, base, workspace, fs)?;
    apply_cfg_args(PartialAppConfig::empty(), &resolved)
}

#[cfg(test)]
#[path = "config_pipeline_tests.rs"]
mod tests;
