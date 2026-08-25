//! Loader directives for configuration entries.

use schematic::{Config, ConfigEnum};
use serde::{Deserialize, Serialize};

use crate::{
    fill::FillDefaults,
    partial::{ToPartial, partial_opts},
};

/// Loader directives for the config file declaring them.
///
/// These settings steer *how* the declaring file is loaded instead of
/// contributing to the resolved configuration.
/// The section counts only when read from the declaring file itself, via
/// [`load_loader_directives`]: a file reached through another file's `extends`
/// has its `[loader]` section ignored, and the section never becomes part of
/// resolved or persisted configuration ([RFD 038]).
/// *When* an entry's directives are honored is the loading host's policy, not
/// defined here.
///
/// [RFD 038]: https://jp.computer/rfd/038
/// [`load_loader_directives`]: crate::util::load_loader_directives
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct LoaderConfig {
    /// Reset accumulated config state before applying the declaring entry.
    ///
    /// `reset = "none"` discards whatever configuration state accumulated
    /// before this entry in the load order; the entry then applies on top of
    /// program defaults.
    ///
    /// The reset is positional: it does not prevent earlier configuration from
    /// being *loaded* — the declaring entry must itself be resolved through
    /// the normal loading sequence — it only discards that configuration's
    /// contribution to the accumulated state.
    pub reset: Option<LoaderReset>,
}

/// The state a `loader.reset` declaration resets accumulated config to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ConfigEnum)]
#[serde(rename_all = "snake_case")]
pub enum LoaderReset {
    /// Reset to program defaults, discarding all accumulated config state.
    #[default]
    None,
}

impl FillDefaults for PartialLoaderConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            reset: self.reset.or(defaults.reset),
        }
    }
}

impl ToPartial for LoaderConfig {
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            reset: partial_opts(self.reset.as_ref(), None),
        }
    }
}

#[cfg(test)]
#[path = "loader_tests.rs"]
mod tests;
