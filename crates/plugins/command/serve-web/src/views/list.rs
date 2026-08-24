//! Conversation list page.

use chrono::{DateTime, Utc};
use jp_plugin::message::ConversationSummary;
use maud::{Markup, PreEscaped, html};
use sha2::{Digest as _, Sha256};

use crate::views::layout;

/// A fingerprint of the list as it is shown.
///
/// Covers what a page would redraw for: which conversations there are, what
/// they are called, and when each was last used — which is also what orders
/// them.
///
/// Sorted before hashing, so this says nothing about the order the host happens
/// to list them in.
/// A conversation moving to the top still changes it, through the timestamp
/// that moved it.
pub(crate) fn digest(conversations: &[ConversationSummary]) -> String {
    let mut entries: Vec<String> = conversations
        .iter()
        .map(|entry| {
            format!(
                "{}\u{1f}{}\u{1f}{}",
                entry.id,
                entry.title.as_deref().unwrap_or_default(),
                entry.last_activated_at.to_rfc3339(),
            )
        })
        .collect();

    entries.sort();

    let mut hasher = Sha256::new();
    for entry in entries {
        hasher.update(entry.as_bytes());
        hasher.update([0x1e]);
    }

    let hash = format!("{:x}", hasher.finalize());
    hash[..16].to_owned()
}

/// Render the conversation list page.
///
/// Takes the summaries directly from the protocol response.
pub(crate) fn render(conversations: &[ConversationSummary]) -> Markup {
    // Sort by last activity (most recent first). The protocol doesn't
    // guarantee order, so we sort here.
    let mut sorted: Vec<&ConversationSummary> = conversations.iter().collect();
    sorted.sort_by_key(|c| std::cmp::Reverse(c.last_activated_at));

    // The document scrolls here, unlike the conversation view: there is no
    // composer and no keyboard, so nothing needs the page pinned — and letting it
    // scroll normally is what makes the platform's own gestures work, including
    // tapping the status bar to return to the top.
    layout::scrolling_page("Conversations", html! {
        // The fingerprint travels with the page so it can ask later whether the
        // list has moved on, without re-reading the list to find out.
        header class="page-header" data-digest=(digest(conversations)) {
            h1 { "Conversations" }
            a href="/conversations/new" class="new-conversation-link" { "New" }
        }

        script { (PreEscaped(LIST_SCRIPT)) }
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
                        // The row is a horizontal scroller with two snap points:
                        // the entry, and the action behind its right edge. Swiping
                        // is then the browser's own scrolling — momentum, rubber
                        // band and all — rather than touch handlers imitating it.
                        li data-id=(entry.id) {
                            div class="row-track" {
                                a class="row-entry" href=(format!("/conversations/{}", entry.id)) {
                                    span class="title" {
                                        (entry.title.as_deref().unwrap_or("Untitled"))
                                    }
                                    time class="timestamp"
                                        datetime=(entry.last_activated_at.to_rfc3339()) {
                                        (format_relative_time(entry.last_activated_at))
                                    }
                                }

                                // A plain form, so this works with no script at
                                // all once the row is scrolled aside.
                                form
                                    class="row-actions"
                                    method="post"
                                    action=(format!("/conversations/{}/archive", entry.id))
                                {
                                    button type="submit" class="archive" { "Archive" }
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

/// Keeps the list current, and the header useful.
///
/// Refreshed on returning to the app rather than on a pull, which is the
/// gesture this would otherwise want: installed to a home screen there is no
/// browser chrome to host a pull-to-refresh, and the version a page can build
/// has no access to the haptic that makes the real one feel like anything.
/// Coming back to a list that is already current is better than a gesture that
/// asks for it.
///
/// Only when the list has actually moved on, so a page already showing it keeps
/// its scroll position and its filter rather than being thrown away to arrive
/// at the same list.
const LIST_SCRIPT: &str = include_str!("list.js");

/// Hide the entries whose title doesn't contain what was typed.
///
/// Enhancement, and only ever subtractive: with JavaScript off the field is
/// inert and the full list is still there.
///
/// Matching reads the rendered title rather than a copy of it, so an untitled
/// conversation matches on the "Untitled" the reader can actually see.
const FILTER_SCRIPT: &str = include_str!("filter.js");

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
