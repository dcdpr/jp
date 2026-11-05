//! The definition of the tool this plugin serves.
//!
//! `jp release tools init` writes it once, and the project owns it from then
//! on.
//! Nothing in it mirrors the configuration, since the types a fragment takes
//! are fixed, so the plugin has no `tools sync`.

use camino::Utf8Path;
use tool_definitions::Definitions;

use crate::Output;

/// The definitions this plugin ships.
pub(crate) const DEFINITIONS: Definitions = Definitions {
    plugin: "release",
    files: &[("add.toml", include_str!("../tools/add.toml"))],
    offers: None,
};

/// Write the starting definition into `dir`, creating it if needed.
pub(crate) fn init(dir: &Utf8Path) -> Result<Output, String> {
    DEFINITIONS.init(dir).map(Output::from)
}

#[cfg(test)]
#[path = "definitions_tests.rs"]
mod tests;
