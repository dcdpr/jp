//! The chooser for the configurations a message runs under.
//!
//! Rendered into the new-conversation form and fetched into the composer's
//! configuration dialog, so both offer the same choices, group them the same
//! way, and post the same fields.

use std::collections::BTreeMap;

use jp_plugin::message::ConfigEntry;
use maud::{Markup, html};

/// Render the chooser: free key/value assignments, then the configurations
/// available by name.
///
/// `selected` are the segments to tick and `pairs` the assignments to fill in,
/// which is what a refused submission hands back so nothing has to be typed
/// twice.
pub(crate) fn chooser(
    configs: &[ConfigEntry],
    selected: &[String],
    pairs: &[(String, String)],
) -> Markup {
    html! {
        (assignments(pairs))

        div class="config-named" {
            @if configs.is_empty() {
                p class="config-note" { "No configurations found on the load paths." }
            }

            @for (namespace, entries) in group_by_namespace(configs) {
                fieldset {
                    legend { (label(namespace)) }
                    @for entry in entries {
                        label class="config-option" {
                            input
                                type="checkbox"
                                name="cfg"
                                value=(entry.segment)
                                checked[selected.iter().any(|s| s == &entry.segment)];
                            span { (entry.name) }
                        }
                    }
                }
            }
        }
    }
}

/// The script the `+` button needs.
///
/// Rendered by both pages that show the chooser.
pub(crate) const SCRIPT: &str = include_str!("configs.js");

/// The rows of free `--cfg` assignments.
///
/// The key and the value are separate fields, paired by position when the form
/// is read: a row posts both, so the two lists stay in step.
/// There is always one blank row below the filled ones, so writing a first
/// assignment does not start by hunting for the button that makes room for it.
fn assignments(pairs: &[(String, String)]) -> Markup {
    let mut rows: Vec<(&str, &str)> = pairs
        .iter()
        .filter(|(key, _)| !key.trim().is_empty())
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();

    rows.push(("", ""));

    html! {
        div class="config-pairs" {
            p class="config-note" {
                "Set a value directly, as "
                code { "--cfg key=value" }
                " does. Applied after the configurations below."
            }

            @for (key, value) in rows {
                div class="config-pair" {
                    input
                        type="text"
                        name="cfg_key"
                        value=(key)
                        placeholder="assistant.model.id"
                        autocomplete="off"
                        autocapitalize="off"
                        spellcheck="false"
                        aria-label="Configuration key";
                    input
                        type="text"
                        name="cfg_value"
                        value=(value)
                        placeholder="value"
                        autocomplete="off"
                        autocapitalize="off"
                        spellcheck="false"
                        aria-label="Configuration value";
                    button
                        type="button"
                        class="config-add"
                        title="Add another"
                        aria-label="Add another assignment"
                    {
                        "+"
                    }
                }
            }
        }
    }
}

/// Group the entries by the directory they live in.
///
/// A map rather than runs over the order they arrive in: the host sorts by full
/// segment, which does not keep a directory's entries together.
/// With `.jp/config` and `.jp/config/personas` both on the load path,
/// `personas/dev` sorts between `default` and `po`, and a directory's entries
/// end up split across as many blocks as there are names sorting between them.
///
/// Namespaces come out alphabetically, which puts the load path's own root
/// first.
fn group_by_namespace(configs: &[ConfigEntry]) -> BTreeMap<&str, Vec<&ConfigEntry>> {
    let mut groups: BTreeMap<&str, Vec<&ConfigEntry>> = BTreeMap::new();

    for entry in configs {
        groups
            .entry(entry.namespace.as_str())
            .or_default()
            .push(entry);
    }

    groups
}

/// The heading for a group, naming the directory its entries live in.
///
/// Entries at a load path's root have no directory to name, so they are
/// labelled generically rather than under an empty heading.
fn label(namespace: &str) -> &str {
    if namespace.is_empty() {
        "General"
    } else {
        namespace
    }
}

#[cfg(test)]
#[path = "configs_tests.rs"]
mod tests;
