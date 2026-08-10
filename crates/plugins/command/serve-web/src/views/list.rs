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

    // The document scrolls here, unlike the conversation view: there is no
    // composer and no keyboard, so nothing needs the page pinned — and letting it
    // scroll normally is what makes the platform's own gestures work, including
    // tapping the status bar to return to the top.
    layout::scrolling_page("Conversations", html! {
        // The count travels with the page so it can ask later whether anything
        // has been added since, without re-reading the list to find out.
        header class="page-header" data-count=(sorted.len()) {
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
/// Only when the count has moved, so a page already showing everything keeps
/// its scroll position and its filter rather than being thrown away to arrive
/// at the same list.
const LIST_SCRIPT: &str = r"
const header = document.querySelector('.page-header');

async function reloadIfStale() {
  try {
    const r = await fetch('/conversations/count');
    if (!r.ok) return;

    const { count } = await r.json();
    if (String(count) !== header.dataset.count) location.reload();
  } catch (e) {
    // Offline, or the server is restarting. The next return tries again.
  }
}

document.addEventListener('visibilitychange', () => {
  if (document.visibilityState === 'visible') reloadIfStale();
});

// The header is a way back to the top on platforms that do not do it themselves.
// Ignores the links inside it, which have somewhere else to go.
header.addEventListener('click', (event) => {
  if (!event.target.closest('a')) scrollTo(0, 0);
});

// Archiving asks first: a swipe is easy to make by accident, and a conversation
// is not something to lose to a stray gesture.
//
// Handled here rather than on the form so the confirmation is one dialog for the
// whole list rather than one per row.
document.addEventListener('submit', async (event) => {
  const form = event.target.closest('.row-actions');
  if (!form) return;

  event.preventDefault();

  const row = form.closest('li');
  const title = row.querySelector('.title').textContent.trim();
  if (!confirm('Archive ' + title + '?')) return;

  try {
    const r = await fetch(form.action, {
      method: 'POST',
      headers: { accept: 'application/json' },
    });
    if (!r.ok) throw new Error(r.status);

    // Removed rather than reloaded: the rest of the list is unchanged, and a
    // reload would lose the filter and the scroll position.
    row.remove();
    header.dataset.count = String(Number(header.dataset.count) - 1);
  } catch (e) {
    alert('Could not archive that conversation.');
  }
});

// A tap on a row that is swiped open should close it rather than follow the
// link, which is what every list with this gesture does.
document.addEventListener('click', (event) => {
  const entry = event.target.closest('.row-entry');
  if (!entry) return;

  const track = entry.closest('.row-track');
  if (track.scrollLeft > 4) {
    event.preventDefault();
    track.scrollTo({ left: 0, behavior: 'smooth' });
  }
});
";

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
