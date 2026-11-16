//! Definitions for the tools this plugin serves.
//!
//! `jp ticket tools init` writes the default tool definitions (one file per
//! tool), and `jp ticket tools sync` generates the config-dependent schema
//! overrides: the board's label vocabulary on tools that accept labels, and the
//! valid `order` values on `list` (including sortable label keys).
//! `tool_definitions` implements both commands; this module declares the
//! template files and parameter mappings.

use camino::Utf8Path;
use serde_json::Value;
use ticket::Vocabulary;
use tool_definitions::{Definitions, Mirror, Role};

use crate::{Output, order};

/// The definitions this plugin ships.
pub(crate) const DEFINITIONS: Definitions = Definitions {
    plugin: "ticket",
    files: &[
        ("comment.toml", include_str!("../tools/comment.toml")),
        ("create.toml", include_str!("../tools/create.toml")),
        ("edit.toml", include_str!("../tools/edit.toml")),
        ("list.toml", include_str!("../tools/list.toml")),
        ("move.toml", include_str!("../tools/move.toml")),
        ("show.toml", include_str!("../tools/show.toml")),
    ],
    offers: "labels",
};

/// What a synced file says about itself, below its header.
const SYNCED_NOTES: &str = "\
#
# Mirrors the board's labels into the schemas of the tools that take them, so
# the assistant is offered them rather than guessing. Run the command again
# after changing the labels or the tools: this file is rewritten whole, and
# edits to it are lost.
#
# The lists differ. `create` and `edit` are offered the labels they may add,
# `edit` in its `add_labels`. `list` also gets retired ones, and a bare key per
# facet, which is how a filter asks for any value under it.
#
# A key the board bounds with `min` or `max`, or makes `unique`, is described
# beside the lists of `create` and `edit`, and `create` requires labels when a
# key needs one, so the assistant knows before its first call.
#
# A board with a key that takes any value gets no lists at all: a list holds
# only the values it names, so it would refuse that key's. Its labels are
# described in words instead.
#
# `list` is also offered the orders it accepts: `board`, `filed`, and each
# label key the board can sort by, with `:desc` to reverse it.
";

/// What a synced file adds to its notes when the board defines no labels.
const NO_LABELS: &str = "#\n# The board defines no labels, so the tools take any label.\n";

/// Write the default tool definitions into `dir`, creating it if needed.
///
/// Existing files that git cannot restore are overwritten only when `force` is
/// true.
pub(crate) fn init(dir: &Utf8Path, force: bool) -> Result<Output, String> {
    DEFINITIONS.init(dir, force).map(Output::from)
}

/// Write [`tool_definitions::SYNCED_FILE`] in `dir` with the board's configured
/// labels.
///
/// An existing synced file that git cannot restore is overwritten only when
/// `force` is true.
pub(crate) fn sync(
    dir: &Utf8Path,
    config: &Value,
    vocabulary: &Vocabulary,
    force: bool,
) -> Result<Output, String> {
    let notes = if vocabulary.is_empty() {
        format!("{SYNCED_NOTES}{NO_LABELS}")
    } else {
        SYNCED_NOTES.to_owned()
    };

    let orders = order::names(vocabulary);

    DEFINITIONS
        .sync(
            dir,
            config,
            &notes,
            |tool| mirrors(tool, vocabulary, &orders),
            force,
        )
        .map(Output::from)
}

/// Config-mirrored parameter schemas for `tool`: label lists from `vocabulary`,
/// plus `orders` when `tool` is `"list"`.
fn mirrors<'a>(tool: &str, vocabulary: &'a Vocabulary, orders: &'a [String]) -> Vec<Mirror<'a>> {
    match tool {
        "create" => vec![Mirror::labels("labels", vocabulary, "ticket", Role::Create)],
        "edit" => vec![Mirror::labels(
            "add_labels",
            vocabulary,
            "ticket",
            Role::Add,
        )],
        "list" => vec![
            Mirror::labels("labels", vocabulary, "ticket", Role::Filter),
            Mirror::one_of("order", orders),
        ],
        _ => vec![],
    }
}

#[cfg(test)]
#[path = "definitions_tests.rs"]
mod tests;
