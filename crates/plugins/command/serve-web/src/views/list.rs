//! Conversation list page.

use chrono::{DateTime, Utc};
use jp_plugin::message::ConversationSummary;
use maud::{Markup, html};

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
        }
        main class="conversation-list" {
            @if sorted.is_empty() {
                p class="empty" { "No conversations yet." }
            } @else {
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
            }
        }
    })
}

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
