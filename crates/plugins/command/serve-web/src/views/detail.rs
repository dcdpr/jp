//! Conversation detail page: renders a single conversation's chat history.

use maud::{Markup, PreEscaped, html};

use crate::{
    render::{self, RenderedEvent},
    views::{configs, layout},
};

/// Render the conversation's messages.
///
/// Separate from the page so the poll endpoint can re-render just this list
/// into a live page.
pub(crate) fn messages(events: &[RenderedEvent]) -> Markup {
    html! {
        @for event in events {
                @match event {
                    RenderedEvent::TurnSeparator => {
                        hr class="turn-separator";
                    }
                    RenderedEvent::UserMessage { html } => {
                        div class="message user" {
                            div class="role" { "You" }
                            div class="content" { (PreEscaped(html)) }
                        }
                    }
                    RenderedEvent::AssistantMessage { html } => {
                        div class="message assistant" {
                            div class="role" { "Assistant" }
                            div class="content" { (PreEscaped(html)) }
                        }
                    }
                    RenderedEvent::Reasoning { html } => {
                        details class="reasoning" {
                            summary { "Reasoning" }
                            div class="content" { (PreEscaped(html)) }
                        }
                    }
                    RenderedEvent::Structured { json } => {
                        div class="message assistant structured" {
                            div class="role" { "Assistant (structured)" }
                            pre class="content" { code { (json) } }
                        }
                    }
                    RenderedEvent::ToolCall { name, arguments, result } => {
                        details class="tool-call" {
                            summary { "Tool: " (name) }
                            @if !arguments.is_empty() {
                                div class="tool-args" {
                                    h4 { "Arguments" }
                                    pre { code { (arguments) } }
                                }
                            }
                            @if let Some(result) = result {
                                div class="tool-result" {
                                    h4 { "Result" }
                                    pre { code { (result) } }
                                }
                            }
                        }
                    }
                }
        }
    }
}

/// An upward arrow, the chat convention for sending.
///
/// Inline rather than a font glyph or an image: it inherits `currentColor`,
/// needs no request, and cannot arrive after the button it belongs to.
fn send_icon() -> Markup {
    html! {
        svg
            viewBox="0 0 24 24"
            width="20"
            height="20"
            fill="none"
            stroke="currentColor"
            stroke-width="2.5"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
        {
            path d="M12 19V5" {}
            path d="M5 12l7-7 7 7" {}
        }
    }
}

/// A chevron, pointing where the button goes.
///
/// `doubled` stacks a second one for the ends of the conversation, the usual
/// way to distinguish "as far as this goes" from "one step".
fn chevron(down: bool, doubled: bool) -> Markup {
    // Two chevrons drawn at the same offsets, flipped as a whole for direction, so
    // the pair stays symmetric rather than being two hand-placed paths.
    let rotate = if down { "rotate(180 12 12)" } else { "" };

    html! {
        svg
            viewBox="0 0 24 24"
            width="18"
            height="18"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
        {
            g transform=(rotate) {
                @if doubled {
                    path d="M6 16l6-6 6 6" {}
                    path d="M6 9l6-6 6 6" {}
                } @else {
                    path d="M6 15l6-6 6 6" {}
                }
            }
        }
    }
}

/// The toggle for the navigation menu: stacked lines, as for any list of jumps.
fn navigate_icon() -> Markup {
    html! {
        svg
            viewBox="0 0 24 24"
            width="18"
            height="18"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            aria-hidden="true"
        {
            path d="M5 7h14" {}
            path d="M5 12h14" {}
            path d="M5 17h14" {}
        }
    }
}

/// Which configurations the next message runs under.
///
/// A native dialog: the backdrop, focus trapping and Escape are the element's
/// job, and doing them by hand is how they end up subtly wrong.
///
/// The dialog takes focus itself on open.
/// Left to the element, focus goes to the first field inside it, which on a
/// touch device raises the keyboard over the dialog before the reader has asked
/// for it.
/// `tabindex` is what makes the dialog a focus target of its own.
fn config_modal() -> Markup {
    html! {
        dialog id="config-modal" class="config-modal" tabindex="-1" autofocus {
            form method="dialog" class="config-form" {
                h2 { "Configuration" }
                p class="config-note" { "Applies from the next message onward." }

                // Filled when the dialog is first opened, so the page does not pay
                // for a list most visits never look at.
                div id="config-groups" class="config-groups" {
                    p class="config-note" { "Loading…" }
                }

                div class="config-actions" {
                    button type="submit" value="cancel" { "Cancel" }
                    button type="submit" value="apply" class="config-apply" { "Apply" }
                }
            }
        }
    }
}

/// Arrows to opposite corners: the usual sign for a larger view of this.
fn expand_icon() -> Markup {
    html! {
        svg
            viewBox="0 0 24 24" width="16" height="16" fill="none"
            stroke="currentColor" stroke-width="2" stroke-linecap="round"
            stroke-linejoin="round" aria-hidden="true"
        {
            path d="M9 3H3v6" {}
            path d="M3 3l7 7" {}
            path d="M15 21h6v-6" {}
            path d="M21 21l-7-7" {}
        }
    }
}

/// A quotation mark, for pulling a passage into a reply.
fn quote_icon() -> Markup {
    html! {
        svg
            viewBox="0 0 24 24" width="16" height="16" fill="none"
            stroke="currentColor" stroke-width="2" stroke-linecap="round"
            stroke-linejoin="round" aria-hidden="true"
        {
            path d="M4 6h16" {}
            path d="M4 18h10" {}
            path d="M4 12h13" {}
            path d="M20 10v8" {}
        }
    }
}

/// The composer again, with room to write in.
fn expand_modal() -> Markup {
    html! {
        dialog id="expand-modal" class="expand-modal" {
            form method="dialog" class="expand-form" {
                textarea id="expanded" placeholder="Reply to this conversation…" {}
                div class="config-actions" {
                    button type="submit" class="config-apply" { "Done" }
                }
            }
        }
    }
}

/// The conversation's name, and the means to change it.
///
/// The heading and the form swap rather than the heading becoming editable: a
/// form brings Enter-to-submit and a real input with it, and a title is short
/// enough that losing the heading's styling for a moment costs nothing.
fn title_bar(id: &str, title: &str) -> Markup {
    html! {
        h1 id="title" { (title) }

        button type="button" id="rename" title="Rename" aria-label="Rename" {
            (pencil_icon())
        }

        form
            id="rename-form"
            class="rename-form"
            method="post"
            action={ "/conversations/" (id) "/title" }
            hidden
        {
            input id="title-field" name="title" type="text" value=(title)
                autocomplete="off" aria-label="Conversation title";
            button type="submit" title="Save" aria-label="Save" { (tick_icon()) }
            button type="button" id="rename-cancel" title="Cancel" aria-label="Cancel" {
                (cross_icon())
            }
        }
    }
}

/// A pencil, for editing what is beside it.
fn pencil_icon() -> Markup {
    html! {
        svg
            viewBox="0 0 24 24" width="14" height="14" fill="none"
            stroke="currentColor" stroke-width="2" stroke-linecap="round"
            stroke-linejoin="round" aria-hidden="true"
        {
            path d="M12 20h9" {}
            path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z" {}
        }
    }
}

/// A tick: accept.
fn tick_icon() -> Markup {
    html! {
        svg
            viewBox="0 0 24 24" width="14" height="14" fill="none"
            stroke="currentColor" stroke-width="2.5" stroke-linecap="round"
            stroke-linejoin="round" aria-hidden="true"
        { path d="M20 6L9 17l-5-5" {} }
    }
}

/// A cross: back out.
fn cross_icon() -> Markup {
    html! {
        svg
            viewBox="0 0 24 24" width="14" height="14" fill="none"
            stroke="currentColor" stroke-width="2.5" stroke-linecap="round"
            aria-hidden="true"
        {
            path d="M18 6L6 18" {}
            path d="M6 6l12 12" {}
        }
    }
}

/// A cog: settings for what comes next.
fn cog_icon() -> Markup {
    html! {
        svg
            viewBox="0 0 24 24"
            width="18"
            height="18"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
        {
            circle cx="12" cy="12" r="3" {}
            path d="M19.4 15a1.7 1.7 0 0 0 .3 1.9l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-2.9 1.2v.2a2 2 0 1 1-4 0v-.1A1.7 1.7 0 0 0 7 19.4a1.7 1.7 0 0 0-1.9.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0-1.2-2.9H1a2 2 0 1 1 0-4h.1A1.7 1.7 0 0 0 2.6 7a1.7 1.7 0 0 0-.3-1.9l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.9.3H7a1.7 1.7 0 0 0 1-1.5V1a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 2.9 1.2l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.9V7a1.7 1.7 0 0 0 1.5 1H23a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z" {}
        }
    }
}

/// A barred circle: the sign for "stop that".
fn stop_icon() -> Markup {
    html! {
        svg
            viewBox="0 0 24 24"
            width="18"
            height="18"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            aria-hidden="true"
        {
            circle cx="12" cy="12" r="9" {}
            path d="M8 12h8" {}
        }
    }
}

/// Render a submitted message that hasn't reached the transcript yet.
///
/// Built from the same parts as a real request — the turn divider, the `You`
/// header, and markdown run through the same renderer — so that when the poll
/// swaps in the persisted event, nothing moves or reflows.
/// Only the dimming distinguishes them.
pub(crate) fn pending(content: &str) -> Markup {
    html! {
        hr class="turn-separator";
        div class="message user pending" {
            div class="role" { "You" }
            div class="content" { (PreEscaped(render::markdown_to_html(content))) }
        }
    }
}

/// Where the reply will appear, which is where its progress belongs.
///
/// Filled by the poller while a turn runs, and again when one fails; rendered
/// here too, so a reload during a turn doesn't look idle for a second.
/// `stoppable` says whether the turn is this server's to interrupt: an
/// interrupt reaches its own host, and a turn started in a terminal belongs to
/// a process this cannot signal.
fn status_row(id: &str, running: bool, stoppable: bool) -> Markup {
    html! {
        div id="status" class="composer-status" {
            @if running {
                span class="composer-working" role="status" aria-label="Working" {
                    i {} i {} i {}
                }

                @if stoppable {
                    form
                        class="composer-stop"
                        method="post"
                        action={ "/conversations/" (id) "/interrupt" }
                    {
                        button type="submit" title="Stop" aria-label="Stop" {
                            (stop_icon())
                        }
                    }
                } @else {
                    span class="composer-hint" {
                        "Another process is running this turn."
                    }
                }
            }
        }
    }
}

/// Render the conversation detail page.
///
/// `running` shows the working indicator from the first paint, so a reload
/// during a turn doesn't look idle.
/// `stoppable` says whether the turn is this server's to interrupt.
/// `first` is the index `events` starts at and `total` how many there are, so
/// the page knows whether older ones exist and where to ask for them.
/// `settled` is the first index that can still change, which the page sends
/// back with each poll so an entry that changes after it was drawn is redrawn.
pub(crate) fn render(
    id: &str,
    title: &str,
    events: &[RenderedEvent],
    first: usize,
    total: usize,
    settled: usize,
    running: bool,
    stoppable: bool,
) -> Markup {
    layout::page(title, html! {
        header class="page-header" {
            a href="/conversations" class="back" { "← Conversations" }
            (title_bar(id, title))
        }

        // Holds the transcript and anything that floats over it. The transcript
        // is the only scrolling region on the page; everything else is a fixed
        // row, which is what keeps the composer put while iOS moves its keyboard
        // around — there is no page scroll for the dock to drift against.
        div class="stage" {
            // Raised by the poller when the server it is talking to is not the
            // one this page came from. Floats just under the header rather than
            // taking a row, so it never reflows the conversation.
            div id="reload" class="reload-banner" hidden {
                "The server restarted with a new build. "
                a href="" { "Reload" }
                " to pick it up."
            }

            // Hidden until the page has scrolled to the end, so a long transcript
            // is not watched painting from the top.
            div id="loading" class="loading-veil" { }

            // Jumps within the conversation, over the transcript's bottom-right.
            //
            // A `details` rather than a scripted toggle: opening and closing is
            // what the element is for, and it keeps working if the script does
            // not. The jumps themselves need the script.
            details id="nav" class="nav" {
                summary title="Navigate" aria-label="Navigate" { (navigate_icon()) }

                div class="nav-menu" {
                    button type="button" data-nav="top" title="To the top" aria-label="To the top" {
                        (chevron(false, true))
                    }
                    button type="button" data-nav="prev" title="Previous turn" aria-label="Previous turn" {
                        (chevron(false, false))
                    }
                    button type="button" data-nav="next" title="Next turn" aria-label="Next turn" {
                        (chevron(true, false))
                    }
                    button type="button" data-nav="bottom" title="To the bottom" aria-label="To the bottom" {
                        (chevron(true, true))
                    }
                }
            }

            (config_modal())
            (expand_modal())

            main id="transcript" class="conversation-detail" {
                // Replaced wholesale by the poller when the count changes.
                // `first` is where this window starts and `count` where it ends;
                // older events are fetched when the reader scrolls back to them.
                div id="messages" data-first=(first) data-count=(total) data-settled=(settled) {
                    (messages(events))
                }

                // A message that has been submitted but hasn't reached the
                // transcript yet. The poller fills and clears it.
                div id="pending" {}

                (status_row(id, running, stoppable))

                // What "the end" means, for scrolling to it.
                //
                // Everything above it down here comes and goes — the pending copy,
                // the status row — and an element with no box cannot be scrolled
                // to. This one is always here and always has a height.
                div id="end" {}
            }
        }

        // A row of its own below the transcript, so the input stays reachable in
        // a long conversation and the status never scrolls away from the control
        // it explains.
        div class="composer-dock" {

            // A plain form post: sending a message needs no JavaScript. The
            // response is a redirect back here, issued as soon as the turn is
            // handed to the host rather than when it finishes.
            form id="composer" class="composer" method="post" action={ "/conversations/" (id) "/turn" } {
                // Acting on the field below them, so above it and inside the same
                // frame rather than off in a corner.
                div class="composer-tools" {
                    button type="button" id="expand" data-label="Expand" aria-label="Expand" {
                        (expand_icon())
                    }
                    button type="button" id="quote" data-label="Quote selection" aria-label="Quote selection" {
                        (quote_icon())
                    }
                    button
                        type="button"
                        id="open-config"
                        data-label="Configuration"
                        aria-label="Configuration for the next message"
                    {
                        (cog_icon())
                    }
                }

                // One row by default, grown by the page while focused. An idle
                // composer should cost the conversation as little height as it can.
                textarea
                    name="content"
                    rows="1"
                    placeholder="Reply to this conversation…"
                    required {}

                // Enabled during a turn this server owns — sending then is how you
                // interrupt and respond. Disabled for a turn another process holds,
                // where the lock would refuse it for as long as that turn runs; the
                // status above says so.
                button
                    id="send"
                    type="submit"
                    title="Send"
                    aria-label="Send"
                    disabled[running && !stoppable]
                {
                    (send_icon())
                }

                // Raised when a save was refused because the draft moved on.
                p id="draft-note" class="composer-error" hidden {}
            }

        }

        script { (PreEscaped(configs::SCRIPT)) }
        script { (PreEscaped(LIVE_SCRIPT)) }
    })
}

/// The page's own behaviour: stick to the bottom, and poll for new events and
/// turn status.
///
/// All of it is enhancement.
/// The composer is a plain form post and the transcript is server-rendered, so
/// with JavaScript off the page still works — it just needs a manual refresh
/// to show what arrived since it loaded.
///
/// The poll URL is derived from the page's own path, which keeps this a static
/// string: no per-page formatting, and nothing interpolated into a script tag.
const LIVE_SCRIPT: &str = include_str!("detail.js");

#[cfg(test)]
#[path = "detail_tests.rs"]
mod tests;
