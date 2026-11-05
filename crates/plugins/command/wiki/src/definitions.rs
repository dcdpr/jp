//! Definitions for the tools this plugin serves.
//!
//! `jp wiki tools init` writes a starting set, one file per tool.
//! `jp wiki tools sync` writes the part that mirrors configuration: the label
//! vocabulary, offered in the schemas of the tools that take labels.
//! `tool_definitions` runs both; this module says which files there are and
//! which tools take what.

use camino::Utf8Path;
use jp_label::Vocabulary;
use serde_json::Value;
use tool_definitions::{Definitions, Mirror, Role};

use crate::{Output, options::Options};

/// The definitions this plugin ships.
pub(crate) const DEFINITIONS: Definitions = Definitions {
    plugin: "wiki",
    files: &[
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
# Mirrors the configured labels into the schemas of the tools that take them,
# so the assistant is offered them rather than guessing. Run the command again
# after changing the labels or the tools: this file is rewritten whole, and
# edits to it are lost.
#
# `create` and `edit` are offered the labels they may add, `edit` in its
# `add_labels`. `list` also gets retired ones, and a bare key per facet, which is
# how a filter asks for any value under it.
#
# A vocabulary with a key that takes any value gets no label lists: a list holds
# only the values it names, so it would refuse that key's. Its labels are
# described in words instead.
";

/// What a synced file adds to its notes when the wiki defines no labels.
const NO_LABELS: &str = "#\n# The wiki defines no labels, so the tools take any label.\n";

/// Write the starting definitions into `dir`, creating it if needed.
pub(crate) fn init(dir: &Utf8Path) -> Result<Output, String> {
    DEFINITIONS.init(dir).map(Output::from)
}

/// Write the configured labels into the schemas of the configured tools that
/// take them, as `dir`'s synced file.
pub(crate) fn sync(dir: &Utf8Path, config: &Value, options: &Options) -> Result<Output, String> {
    let vocabulary = options.vocabulary()?;
    let notes = if vocabulary.is_empty() {
        format!("{SYNCED_NOTES}{NO_LABELS}")
    } else {
        SYNCED_NOTES.to_owned()
    };

    DEFINITIONS
        .sync(dir, config, &notes, |tool| mirrors(tool, &vocabulary))
        .map(Output::from)
}

/// The parameters of `tool` that take their values from the configuration.
fn mirrors<'a>(tool: &str, vocabulary: &'a Vocabulary) -> Vec<Mirror<'a>> {
    let (labels, role) = match tool {
        "create" => ("labels", Role::Create),
        "edit" => ("add_labels", Role::Add),
        "list" => ("labels", Role::Filter),
        _ => return vec![],
    };

    vec![Mirror::labels(labels, vocabulary, "page", role)]
}

#[cfg(test)]
#[path = "definitions_tests.rs"]
mod tests;
