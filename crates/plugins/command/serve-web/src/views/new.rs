//! The form for starting a conversation.

use jp_plugin::message::ConfigEntry;
use maud::{Markup, PreEscaped, html};

use super::{configs, layout};

/// Render the new-conversation form.
///
/// `selected` and `pairs` are the configuration choices to restore, and `error`
/// is shown above the form when a previous attempt was refused; the fields keep
/// what was typed so nothing has to be entered twice.
pub(crate) fn render(
    entries: &[ConfigEntry],
    content: &str,
    title: &str,
    selected: &[String],
    pairs: &[(String, String)],
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
                // Filled by the script below with this tab's identity, so the
                // turn this starts is recorded as belonging to the page that is
                // about to be redirected to it.
                input type="hidden" id="client" name="client";

                label {
                    span class="field-label" { "Title" }
                    input
                        type="text"
                        name="title"
                        value=(title)
                        placeholder="Optional; named from the first turn if left blank";
                }

                (configs::chooser(entries, selected, pairs))

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

            script { (PreEscaped(CLIENT_SCRIPT)) }
        }

        script { (PreEscaped(configs::SCRIPT)) }
    })
}

/// Carry this tab's identity into the form.
///
/// The conversation this starts is answered with a redirect, and the page that
/// lands there asks the server whether the running turn is its own.
/// A turn recorded without a client belongs to nobody, so that page is told the
/// turn is somebody else's and asks before stopping the turn it just started
/// itself.
///
/// Enhancement, and correct either way: with no script the field stays empty,
/// the turn is unattributed, and that is the truth — a page that cannot store
/// an identity has none to claim a turn with.
const CLIENT_SCRIPT: &str = r"
// The same per-tab key the conversation page reads, because the page that has to
// recognise this turn is the one this redirects to, in this same tab.
try {
  let id = sessionStorage.getItem('jp-client');
  if (!id) {
    id = Math.random().toString(36).slice(2) + Date.now().toString(36);
    sessionStorage.setItem('jp-client', id);
  }
  document.getElementById('client').value = id;
} catch (e) {
  // Private browsing, or storage denied. The turn stays unattributed, which
  // reads as shared and errs toward asking.
}
";
