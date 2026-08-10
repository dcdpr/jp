//! The form for starting a conversation.

use jp_plugin::message::ConfigEntry;
use maud::{Markup, html};

use super::layout;

/// Render the new-conversation form.
///
/// `configs` are grouped by namespace in the order the host listed them, which
/// is sorted by segment — so a namespace's entries arrive together and the
/// groups come out alphabetically.
///
/// `error` is shown above the form when a previous attempt was refused; the
/// fields keep what was typed so nothing has to be entered twice.
pub(crate) fn render(
    configs: &[ConfigEntry],
    content: &str,
    title: &str,
    selected: &[String],
    error: Option<&str>,
) -> Markup {
    layout::page("New conversation", html! {
        header class="page-header" {
            a href="/conversations" class="back" { "← Conversations" }
            h1 { "New conversation" }
        }

        main class="conversation-detail" {
            @if let Some(error) = error {
                p class="composer-error" { (error) }
            }

            form class="new-conversation" method="post" action="/conversations/new" {
                label {
                    span class="field-label" { "Title" }
                    input
                        type="text"
                        name="title"
                        value=(title)
                        placeholder="Optional; named from the first turn if left blank";
                }

                @for group in group_by_namespace(configs) {
                    fieldset {
                        legend { (group.label()) }
                        @for entry in group.entries {
                            label class="config-option" {
                                input
                                    type="checkbox"
                                    name="cfg"
                                    value=(entry.segment)
                                    checked[selected.contains(&entry.segment)];
                                span { (entry.name) }
                            }
                        }
                    }
                }

                label {
                    span class="field-label" { "Message" }
                    textarea
                        name="content"
                        rows="5"
                        placeholder="What do you want to ask?"
                        required { (content) }
                }

                div {
                    button type="submit" { "Start" }
                }
            }
        }
    })
}

/// Configurations sharing a namespace, in the order the host listed them.
struct Group<'a> {
    namespace: &'a str,
    entries: Vec<&'a ConfigEntry>,
}

impl Group<'_> {
    /// The heading for the group, naming the load-path directory it came from.
    ///
    /// Entries at the load path's root have no namespace to show, so they are
    /// labelled generically rather than with an empty heading.
    fn label(&self) -> &str {
        if self.namespace.is_empty() {
            "General"
        } else {
            self.namespace
        }
    }
}

/// Split a sorted list into runs sharing a namespace.
///
/// Relies on the host's sort by segment: entries in one namespace share a
/// prefix, so they are already adjacent and no grouping map is needed.
fn group_by_namespace(configs: &[ConfigEntry]) -> Vec<Group<'_>> {
    let mut groups: Vec<Group<'_>> = Vec::new();

    for entry in configs {
        match groups.last_mut() {
            Some(group) if group.namespace == entry.namespace => group.entries.push(entry),
            _ => groups.push(Group {
                namespace: &entry.namespace,
                entries: vec![entry],
            }),
        }
    }

    groups
}
