//! Conversation list page.

use chrono::{DateTime, Utc};
use jp_plugin::message::ConversationSummary;
use maud::{Markup, PreEscaped, html};

use crate::views::layout;

/// Render the conversation list page.
///
/// Takes the summaries directly from the protocol response.
pub(crate) fn render(conversations: &[ConversationSummary]) -> Markup {
    // Sort by last activity (most recent first). The protocol doesn't
    // guarantee order, so we sort here.
    let mut sorted: Vec<&ConversationSummary> = conversations.iter().collect();
    sorted.sort_by_key(|c| std::cmp::Reverse(c.last_activated_at));

    layout::page("Conversations", html! {
        header class="page-header" {
            h1 { "Conversations" }
            a href="/conversations/new" class="new-conversation-link" { "New" }
        }
        @if sorted.is_empty() {
            main class="conversation-list" {
                p class="empty" { "No conversations yet." }
            }
        } @else {
            // A row of its own above the list, so it stays put while the list
            // scrolls under it.
            div class="list-search" {
                input
                    id="filter"
                    type="search"
                    placeholder="Filter by title…"
                    autocomplete="off"
                    aria-label="Filter conversations by title";
            }
            main class="conversation-list" {
                ul {
                    @for entry in &sorted {
                        li {
                            a href=(format!("/conversations/{}", entry.id)) {
                                span class="title" {
                                    (entry.title.as_deref().unwrap_or("Untitled"))
                                }
                                time class="timestamp"
                                    datetime=(entry.last_activated_at.to_rfc3339()) {
                                    (format_relative_time(entry.last_activated_at))
                                }
                            }
                        }
                    }
                }

                // Shown by the filter when it hides every entry.
                p id="no-matches" class="empty" hidden { "No matching conversations." }
            }
            script { (PreEscaped(FILTER_SCRIPT)) }
        }
    })
}

/// Hide the entries whose title doesn't contain what was typed.
///
/// Enhancement, and only ever subtractive: with JavaScript off the field is
/// inert and the full list is still there.
///
/// Matching reads the rendered title rather than a copy of it, so an untitled
/// conversation matches on the "Untitled" the reader can actually see.
const FILTER_SCRIPT: &str = r"
const field = document.getElementById('filter');
const entries = [...document.querySelectorAll('.conversation-list li')];
const noMatches = document.getElementById('no-matches');

const apply = () => {
  const needle = field.value.trim().toLowerCase();
  let shown = 0;

  for (const entry of entries) {
    const title = entry.querySelector('.title').textContent.toLowerCase();
    const match = title.includes(needle);
    entry.hidden = !match;
    if (match) shown++;
  }

  noMatches.hidden = shown > 0;
};

field.addEventListener('input', apply);

// Browsers restore a field's value on a back navigation without firing `input`,
// which would otherwise leave the text sitting above an unfiltered list.
apply();
";

/// Format a timestamp as a human-readable relative string.
fn format_relative_time(dt: DateTime<Utc>) -> String {
    let now = Utc::now();
    let duration = now.signed_duration_since(dt);

    let secs = duration.num_seconds();
    if secs < 60 {
        return "just now".to_owned();
    }

    let mins = duration.num_minutes();
    if mins < 60 {
        return format!("{mins}m ago");
    }

    let hours = duration.num_hours();
    if hours < 24 {
        return format!("{hours}h ago");
    }

    let days = duration.num_days();
    if days < 30 {
        return format!("{days}d ago");
    }

    dt.format("%Y-%m-%d").to_string()
}

#[cfg(test)]
#[path = "list_tests.rs"]
mod tests;
