//! The ordered list of configurations a message runs under.
//!
//! Rendered into the new-conversation form and fetched into the composer's
//! configuration dialog, so both offer the same choices and post the same
//! field.
//!
//! Every row posts one `cfg` field holding a whole `--cfg` argument, whether
//! the row named a configuration or spelled out an assignment.
//! A form submits its fields in document order, so the list the reader arranges
//! is the argument order the host applies.

use std::collections::BTreeMap;

use jp_plugin::message::ConfigEntry;
use maud::{Markup, html};

/// Render the chooser: the chosen arguments in the order they apply, and the
/// buttons that add another.
///
/// `chosen` is what a refused submission hands back, so nothing has to be
/// entered twice.
/// An argument naming one of `configs` comes back as a picker and anything else
/// as text, which is the same split `--cfg` makes between a configuration to
/// load and a value to assign.
pub(crate) fn chooser(configs: &[ConfigEntry], chosen: &[String]) -> Markup {
    html! {
        div class="config-list" {
            p class="config-note" {
                "Applied top to bottom, as repeated "
                code { "--cfg" }
                " arguments are: a later row wins over an earlier one."
            }

            ol class="config-items" {
                @for argument in chosen {
                    @if names_a_config(configs, argument) {
                        (item(&picker(configs, argument)))
                    } @else {
                        (item(&assignment(argument)))
                    }
                }
            }

            // Cloned by the buttons below. A template's contents are inert, so
            // the fields in here post nothing until a row is made from them.
            template class="config-template" data-kind="named" {
                (item(&picker(configs, "")))
            }
            template class="config-template" data-kind="value" {
                (item(&assignment("")))
            }

            div class="config-new" {
                button
                    type="button"
                    class="config-add"
                    data-kind="named"
                    disabled[configs.is_empty()]
                {
                    "+ Configuration"
                }
                button type="button" class="config-add" data-kind="value" {
                    "+ Value"
                }
            }

            @if configs.is_empty() {
                p class="config-note" { "No configurations found on the load paths." }
            } @else {
                (picker_dialog(configs))
            }
        }
    }
}

/// The script the list needs: adding, reordering and removing rows.
///
/// Rendered by both pages that show the chooser.
pub(crate) const SCRIPT: &str = include_str!("configs.js");

/// One row of the list: the grip it is dragged by, the button that drops it,
/// and what it contributes.
///
/// Both controls lead the row, so they stay in one column however wide the
/// field beside them is.
/// The grip is also the only part of a row that starts a drag: everything else
/// is a field that needs the pointer for itself.
fn item(body: &Markup) -> Markup {
    html! {
        li class="config-item" {
            button
                type="button"
                class="config-grip"
                title="Drag to reorder"
                aria-label="Reorder this row; the arrow keys move it too"
            {
                "⠿"
            }
            button type="button" class="config-remove" title="Remove" aria-label="Remove" {
                "×"
            }
            (body)
        }
    }
}

/// The modal `+ Configuration` opens: the configurations on the load paths,
/// ticked as a set.
///
/// One row is added per ticked box, in the order the boxes were ticked, which
/// the ordinal beside each one shows as it is chosen.
/// Reordering afterwards is the list's job.
///
/// The filter takes the focus the dialog opens with, so a load path holding
/// fifty configurations is a word away from the one that was wanted rather than
/// a scroll.
/// Enter ticks what the filter has left and Cmd+Enter finishes, so the whole
/// dialog is a few words long and the pointer never arrives.
///
/// The boxes carry no `name`, so neither form the chooser sits in posts them: a
/// message runs under the list, not under what was ticked to build it.
fn picker_dialog(configs: &[ConfigEntry]) -> Markup {
    html! {
        dialog class="config-modal config-picker" {
            div class="config-picker-body" {
                h2 { "Add configurations" }
                p class="config-note" {
                    "Added in the order they are ticked. Enter ticks the first match, "
                    "⌘ Enter adds them."
                }

                input
                    type="text"
                    class="config-filter"
                    placeholder="Filter…"
                    autocomplete="off"
                    autocapitalize="off"
                    spellcheck="false"
                    aria-label="Filter configurations"
                    autofocus;

                div class="config-picker-groups" {
                    @for (namespace, entries) in group_by_namespace(configs) {
                        fieldset {
                            legend { (label(namespace)) }
                            @for entry in entries {
                                label class="config-option" {
                                    input type="checkbox" data-segment=(entry.segment);
                                    span { (entry.name) }
                                    span class="config-order" {}
                                }
                            }
                        }
                    }
                }

                div class="config-actions" {
                    button type="button" class="config-picker-cancel" { "Cancel" }
                    button type="button" class="config-picker-add config-apply" { "Add" }
                }
            }
        }
    }
}

/// A row that loads one of the configurations on the load paths.
///
/// The first option is the empty one a fresh row starts on, so adding a row
/// does not quietly apply whichever configuration happens to sort first.
/// It posts an empty argument, which the server drops.
fn picker(configs: &[ConfigEntry], selected: &str) -> Markup {
    html! {
        select class="config-pick" name="cfg" aria-label="Configuration" {
            option value="" { "Choose a configuration…" }

            @for (namespace, entries) in group_by_namespace(configs) {
                optgroup label=(label(namespace)) {
                    @for entry in entries {
                        option value=(entry.segment) selected[entry.segment == selected] {
                            (entry.name)
                        }
                    }
                }
            }
        }
    }
}

/// A row holding a `--cfg` argument written out.
///
/// One field rather than a key and a value: `--cfg` takes more than an
/// assignment — a JSON object, an `@path`, a reset keyword — and splitting
/// the field at an `=` this end would put those out of reach.
fn assignment(value: &str) -> Markup {
    html! {
        input
            type="text"
            class="config-raw"
            name="cfg"
            value=(value)
            placeholder="assistant.model.id=opus"
            autocomplete="off"
            autocapitalize="off"
            spellcheck="false"
            aria-label="Configuration argument";
    }
}

/// Whether an argument is the name of a configuration on the load paths.
fn names_a_config(configs: &[ConfigEntry], argument: &str) -> bool {
    configs.iter().any(|entry| entry.segment == argument)
}

/// Group the entries by the directory they live in.
///
/// A map rather than runs over the order they arrive in: the host sorts by full
/// segment, which does not keep a directory's entries together.
/// With `.jp/config` and `.jp/config/personas` both on the load path,
/// `personas/dev` sorts between `default` and `po`, and a directory's entries
/// end up split across as many groups as there are names sorting between them.
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
