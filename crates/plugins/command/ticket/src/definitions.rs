//! Definitions for the tools this plugin serves.
//!
//! `jp ticket tools init` writes a starting set, one file per tool, and `jp
//! ticket tools sync` writes the one part that mirrors configuration: the
//! board's labels, offered in the schemas of the tools that take them.
//! `tool_definitions` runs both; this module says which files there are and
//! which tools take labels.

use camino::Utf8Path;
use serde_json::Value;
use ticket::Vocabulary;
use tool_definitions::{Definitions, Mirror, Role};

use crate::Output;

/// The definitions this plugin ships.
pub(crate) const DEFINITIONS: Definitions = Definitions {
    plugin: "ticket",
    files: &[
        ("comment.toml", include_str!("../tools/comment.toml")),
        ("create.toml", include_str!("../tools/create.toml")),
        ("edit.toml", include_str!("../tools/edit.toml")),
        ("list.toml", include_str!("../tools/list.toml")),
        ("show.toml", include_str!("../tools/show.toml")),
    ],
    offers: Some("labels"),
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
";

/// What a synced file adds to its notes when the board defines no labels.
const NO_LABELS: &str = "#\n# The board defines no labels, so the tools take any label.\n";

/// Write the starting definitions into `dir`, creating it if needed.
pub(crate) fn init(dir: &Utf8Path) -> Result<Output, String> {
    DEFINITIONS.init(dir).map(Output::from)
}

/// Write the board's labels into the schemas of the configured tools that take
/// them, as `dir`'s synced file.
pub(crate) fn sync(
    dir: &Utf8Path,
    config: &Value,
    vocabulary: &Vocabulary,
) -> Result<Output, String> {
    let notes = if vocabulary.is_empty() {
        format!("{SYNCED_NOTES}{NO_LABELS}")
    } else {
        SYNCED_NOTES.to_owned()
    };

    DEFINITIONS
        .sync(dir, config, &notes, |tool| mirrors(tool, vocabulary))
        .map(Output::from)
}

/// The parameters of `tool` that take their values from the configuration.
fn mirrors<'a>(tool: &str, vocabulary: &'a Vocabulary) -> Vec<Mirror<'a>> {
    let (parameter, role) = match tool {
        "create" => ("labels", Role::Create),
        "edit" => ("add_labels", Role::Add),
        "list" => ("labels", Role::Filter),
        _ => return vec![],
    };

    vec![Mirror::labels(parameter, vocabulary, "ticket", role)]
}

#[cfg(test)]
#[path = "definitions_tests.rs"]
mod tests;
