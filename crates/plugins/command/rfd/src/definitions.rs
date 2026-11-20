//! Definitions for the tools this plugin serves.
//!
//! `jp rfd tools init` writes a starting set, one file per tool, and `jp rfd
//! tools sync` writes the one part that mirrors configuration: the categories,
//! offered in the schemas of the tools that take one.
//! `tool_definitions` runs both; this module says which files there are and
//! which tools take a category.

use camino::Utf8Path;
use serde_json::Value;
use tool_definitions::{Definitions, Mirror};

use crate::Output;

/// The definitions this plugin ships.
pub(crate) const DEFINITIONS: Definitions = Definitions {
    plugin: "rfd",
    files: &[
        ("abandon.toml", include_str!("../tools/abandon.toml")),
        ("draft.toml", include_str!("../tools/draft.toml")),
        ("extend.toml", include_str!("../tools/extend.toml")),
        ("list.toml", include_str!("../tools/list.toml")),
        ("promote.toml", include_str!("../tools/promote.toml")),
        ("rename.toml", include_str!("../tools/rename.toml")),
        ("renumber.toml", include_str!("../tools/renumber.toml")),
        ("require.toml", include_str!("../tools/require.toml")),
        ("show.toml", include_str!("../tools/show.toml")),
        ("supersede.toml", include_str!("../tools/supersede.toml")),
    ],
    offers: Some("categories"),
};

/// What a synced file says about itself, below its header.
const SYNCED_NOTES: &str = "\
#
# Mirrors the configured RFD categories into the schemas of the tools that take
# one, so the assistant is offered them rather than guessing. Run the command
# again after changing the categories or the tools: this file is rewritten
# whole, and edits to it are lost.
";

/// Write the default tool definitions into `dir`, creating it if needed.
///
/// Existing files that git cannot restore are overwritten only when `force` is
/// true.
pub(crate) fn init(dir: &Utf8Path, force: bool) -> Result<Output, String> {
    DEFINITIONS.init(dir, force).map(Output::from)
}

/// Write [`tool_definitions::SYNCED_FILE`] in `dir` with the configured RFD
/// categories.
///
/// An existing synced file that git cannot restore is overwritten only when
/// `force` is true.
pub(crate) fn sync(
    dir: &Utf8Path,
    config: &Value,
    categories: &[String],
    force: bool,
) -> Result<Output, String> {
    DEFINITIONS
        .sync(
            dir,
            config,
            SYNCED_NOTES,
            |tool| mirrors(tool, categories),
            force,
        )
        .map(Output::from)
}

/// The parameters of `tool` that take their values from the configuration.
fn mirrors<'a>(tool: &str, categories: &'a [String]) -> Vec<Mirror<'a>> {
    match tool {
        "draft" | "list" => vec![Mirror::one_of("category", categories)],
        _ => vec![],
    }
}

#[cfg(test)]
#[path = "definitions_tests.rs"]
mod tests;
