//! Default tool definitions shipped by `jp release`.
//!
//! `jp release tools init` writes `add.toml` into the workspace's tool
//! definitions directory.
//! Because fragment types are fixed, no parameters depend on workspace
//! configuration and `jp release` has no `tools sync` subcommand.

use camino::Utf8Path;
use tool_definitions::Definitions;

use crate::Output;

pub(crate) const DEFINITIONS: Definitions = Definitions {
    plugin: "release",
    files: &[("add.toml", include_str!("../tools/add.toml"))],
    offers: None,
};

pub(crate) fn init(dir: &Utf8Path, force: bool) -> Result<Output, String> {
    DEFINITIONS.init(dir, force).map(Output::from)
}

#[cfg(test)]
#[path = "definitions_tests.rs"]
mod tests;
