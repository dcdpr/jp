//! Conversation detail page: renders a single conversation's chat history.

use maud::{Markup, PreEscaped, html};

use crate::{
    render::{self, RenderedEvent},
    views::layout,
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
fn config_modal() -> Markup {
    html! {
        dialog id="config-modal" class="config-modal" {
            form method="dialog" class="config-form" {
                h2 { "Configuration" }
                p class="config-note" {
                    "Applies from the next message onward, as "
                    code { "jp q --cfg" }
                    " does."
                }

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

/// Render the conversation detail page.
///
/// `running` shows the working indicator from the first paint, so a reload
/// during a turn doesn't look idle.
/// `stoppable` says whether the turn is this server's to interrupt.
/// `first` is the index `events` starts at and `total` how many there are, so
/// the page knows whether older ones exist and where to ask for them.
pub(crate) fn render(
    id: &str,
    title: &str,
    events: &[RenderedEvent],
    first: usize,
    total: usize,
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
                div id="messages" data-first=(first) data-count=(total) {
                    (messages(events))
                }

                // A message that has been submitted but hasn't reached the
                // transcript yet. The poller fills and clears it.
                div id="pending" {}

                // Where the reply will appear, which is where its progress
                // belongs. Filled by the poller while a turn runs, and again when
                // one fails.
                div id="status" class="composer-status" {
                    @if running {
                        span class="composer-working" role="status" aria-label="Working" {
                            i {} i {} i {}
                        }
                        @if !stoppable {
                            span class="composer-hint" {
                                "Another process is running this turn."
                            }
                        }
                        // Only when this server is the one running the turn: an
                        // interrupt reaches its own host, and a turn started in a
                        // terminal belongs to a process this cannot signal.
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
                        }
                    }
                }

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
const LIVE_SCRIPT: &str = r"
// The two faces of the send button, matching what the server renders.
// Single-quoted attributes: this script is a Rust string, and a double quote ends
// it.
const SEND_SVG =
  `<svg viewBox='0 0 24 24' width='20' height='20' fill='none'`
  + ` stroke='currentColor' stroke-width='2.5' stroke-linecap='round'`
  + ` stroke-linejoin='round' aria-hidden='true'><path d='M12 19V5'></path>`
  + `<path d='M5 12l7-7 7 7'></path></svg>`;

const CANCEL_SVG =
  `<svg viewBox='0 0 24 24' width='20' height='20' fill='none'`
  + ` stroke='currentColor' stroke-width='2.5' stroke-linecap='round'`
  + ` aria-hidden='true'><path d='M18 6L6 18'></path>`
  + `<path d='M6 6l12 12'></path></svg>`;

const transcript = document.getElementById('transcript');
const end = document.getElementById('end');
const box = document.getElementById('messages');

// The window this page holds: `first` is the index of its oldest event, and
// `data-count` the total the conversation has. Older ones are fetched as the
// reader scrolls back to them.
const older = () => Number(box.dataset.first) > 0;
let loadingOlder = false;
const pending = document.getElementById('pending');
const status = document.getElementById('status');
const composer = document.getElementById('composer');
const send = document.getElementById('send');
const reload = document.getElementById('reload');
const draftNote = document.getElementById('draft-note');
let boot = null;

// Set once the form is on its way, so the draft handlers below stop writing:
// the message belongs to the conversation now, not to the draft.
let submitted = false;

// Whether the send button is currently offering to pull the message back.
let cancelling = false;

function setCancelMode(on) {
  if (on === cancelling) return;
  cancelling = on;

  send.classList.toggle('cancelling', on);
  send.title = on ? 'Cancel' : 'Send';
  send.setAttribute('aria-label', send.title);
  send.innerHTML = on ? CANCEL_SVG : SEND_SVG;
}

// The event count as it was when a message was sent, or null when nothing is in
// flight. The field is emptied once the count moves past it, which is the first
// moment the message is known to have been recorded rather than merely accepted.
let clearWhenLanded = null;

// Sealed while a message is on its way.
//
// The field still holds the text at that point — it is not released until the
// message is recorded — so leaving it editable invites typing into a value that is
// about to be cleared, and leaving Send live invites sending it twice.
function lockComposer(locked) {
  input.readOnly = locked;
  send.disabled = locked;
}
const base = location.pathname.replace(/\/$/, '');
const url = base + '/messages';
const draftUrl = base + '/draft';

// This tab's identity, so the server can tell a turn this window started from one
// another window did. `sessionStorage` is per-tab and survives a reload, which is
// the same lifetime a terminal session has.
//
// Never leaves the server: it exists to answer whether a turn is this window's,
// and no other process has any use for that answer.
const clientId = (() => {
  try {
    let id = sessionStorage.getItem('jp-client');
    if (!id) {
      id = Math.random().toString(36).slice(2) + Date.now().toString(36);
      sessionStorage.setItem('jp-client', id);
    }
    return id;
  } catch (e) {
    // Private browsing, or storage denied. Turns then read as shared, which errs
    // toward asking rather than assuming.
    return '';
  }
})();

// Size the app to what is actually visible.
//
// iOS shrinks the visual viewport for the keyboard without touching the layout
// viewport. Chasing that with a sticky offset always trails by a frame and drifts
// while the page scrolls, because iOS pans the visual viewport during a gesture.
// Sizing the whole app to the visible height instead means the composer is simply
// the last row of a box that fits: nothing to chase, nothing to drift.
const visible = () => (window.visualViewport ? visualViewport.height : innerHeight);

// How long to keep following after a change, and the single-frame jump above which
// iOS is reporting a destination rather than a slide.
const SETTLE_MS = 600;
const STEP_PX = 24;

// How much of the layout viewport the keyboard covers.
//
// The layout viewport keeps its full height on iOS while the visual viewport
// shrinks and pans, so the difference between them is the keyboard.
function keyboardInset() {
  const vv = window.visualViewport;
  if (!vv) return 0;
  return Math.max(0, innerHeight - vv.height - vv.offsetTop);
}

// Publish the keyboard height for the composer to lift itself by.
//
// Compares against what was last written rather than against the previous
// measurement. A change that lands between two frames — or before any loop starts,
// which is what happens when the keyboard closes — is still a change from what is
// on screen, and measuring against the reading would call it settled and leave the
// stale value in place.
let applied = -1;

function fitApp() {
  const inset = keyboardInset();
  if (inset === applied) return;

  // A large jump is iOS reporting the destination rather than the slide. That one
  // gets eased; the small ones are the slide itself and are followed exactly.
  document.documentElement.classList.toggle(
    'eased',
    applied >= 0 && Math.abs(inset - applied) > STEP_PX,
  );

  applied = inset;
  document.documentElement.style.setProperty('--kb', inset + 'px');
}

// The composer's height, so the transcript can reserve room for it.
//
// Measured rather than assumed, because it changes: the field grows on focus, and
// the status row appears while a turn runs. A fixed element takes no space of its
// own, so without this the last message sits underneath it.
// Rounded, and only when it moves by more than a pixel.
//
// The field's reported height wobbles by a pixel as focus comes and goes, and
// writing that through shrinks the space reserved for the dock — which moves the
// whole conversation down by a pixel for no reason anyone asked for.
const dock = document.querySelector('.composer-dock');
let dockHeight = 0;

function fitDock() {
  const height = Math.round(dock.getBoundingClientRect().height);
  if (Math.abs(height - dockHeight) <= 1) return;

  const wasDown = atBottom();
  dockHeight = height;
  document.documentElement.style.setProperty('--dock', height + 'px');
  if (wasDown) toBottom();
}

// Follow the keyboard by sampling it, rather than by modelling it.
//
// iOS animates the keyboard over a duration it reports to native code and not to
// the web, using an easing curve Apple has never published. Any transition here is
// therefore a guess at both, and a guess that is close is still visibly out of
// step with the thing it is imitating.
//
// Reading `visualViewport.height` every frame sidesteps the question: whatever the
// curve and duration are, the height is the truth about where the keyboard is now.
// Where iOS reports the slide in steps, this follows the steps; the eased class
// below smooths the case where it reports the end state in one jump instead.
//
// Runs only in bursts around a viewport change, not continuously.
let tracking = 0;
function trackKeyboard() {
  const until = performance.now() + SETTLE_MS;

  // Extends the window a running loop already covers rather than starting a second
  // one: viewport events arrive in bursts.
  if (tracking > 0) {
    tracking = until;
    return;
  }

  tracking = until;

  // Captured once, at the start: whether to hold the newest message against the
  // composer is a question about where the reader was before the keyboard moved.
  const wasDown = atBottom();

  const step = () => {
    fitApp();

    // Each frame, because the composer is still moving over the content.
    if (wasDown) toBottom();

    if (performance.now() < tracking) {
      requestAnimationFrame(step);
      return;
    }

    tracking = 0;
    if (wasDown) toBottom();
  };

  requestAnimationFrame(step);
}

// `resize` on the visual viewport reports the keyboard taking space; `scroll`
// reports it being panned. The window's own `resize` covers the keyboard closing,
// which iOS does not always report on the visual viewport at all.
if (window.visualViewport) {
  visualViewport.addEventListener('resize', trackKeyboard);
  visualViewport.addEventListener('scroll', trackKeyboard);
}
addEventListener('resize', trackKeyboard);
addEventListener('orientationchange', trackKeyboard);
fitApp();

// Stop iOS panning the page away, rather than trying to put it back.
//
// Tapping a field makes iOS focus it and pan the viewport so it clears the
// keyboard, which carries the header off the top of the screen. Focusing the
// field ourselves first, in the capture phase before the native tap flow gets
// there, means iOS finds it already focused and skips the pan entirely.
//
// This is the part that works. Undoing the pan afterwards cannot: scrolling back
// does not stick while the field holds focus, because iOS re-applies it to keep
// the field visible — which is what every earlier attempt here ran into.
//
// `preventScroll` also suppresses the browser's own scroll-into-view. That costs
// nothing here: the composer is a row of a box sized to the visible area, so it
// is never behind the keyboard to begin with.
document.addEventListener('touchstart', (event) => {
  const target = event.target;
  if (target?.matches?.('textarea, input') && document.activeElement !== target) {
    target.focus({ preventScroll: true });
  }
}, { capture: true, passive: true });

// Backgrounding the app with the keyboard open leaves the layout wrong on return:
// iOS snapshots the focused state and keeps the page scrolled to hold the field in
// view. Dropping focus is what releases that, and only then does resetting the
// scroll stick.
function blurField() {
  const active = document.activeElement;
  if (active?.matches?.('textarea, input')) active.blur();
}

function restore() {
  blurField();

  if (scrollY !== 0 || scrollX !== 0) scrollTo(0, 0);
  const root = document.scrollingElement || document.documentElement;
  if (root.scrollTop !== 0) root.scrollTop = 0;

  fitApp();

  // Coming back to the page is the moment its content is most likely to be
  // stale, and a restored page runs no script on the way in: the timer is
  // wherever it was left, up to three seconds away. Ask now instead of waiting
  // for it.
  poll();
}

addEventListener('pageshow', restore);
document.addEventListener('visibilitychange', () => {
  if (document.visibilityState === 'hidden') blurField();
  else restore();
});

// One line when idle, grown to its content while focused.
//
// An unfocused composer is dead space in a conversation, so it collapses back to a
// single row and gives the height to the transcript. Clearing the inline height
// returns it to the one-row default from the markup, rather than guessing a pixel
// value here.
const input = composer.querySelector('textarea');

// One row, measured once: what the field collapses back to when it is not being
// typed in.
const restHeight = (() => {
  input.style.height = 'auto';
  const height = input.scrollHeight;
  input.style.height = height + 'px';
  return height;
})();
function fitInput() {

  // `auto` first so the field can shrink as well as grow, then the final height,
  // both in one synchronous block: the browser paints once, at the end. Reading
  // and restoring in between is what made this flicker.
  //
  // Always an explicit height, never cleared. The field's natural height differs
  // from its measured one-row height by a fraction of a pixel, so letting it fall
  // back on blur resizes the dock — which reserves space for itself, so the whole
  // conversation shifts by a pixel every time focus comes and goes.
  const current = input.style.height;

  let wanted;
  if (document.activeElement === input) {
    input.style.height = 'auto';
    wanted = Math.min(input.scrollHeight, visible() / 3) + 'px';
  } else {
    wanted = restHeight + 'px';
  }

  // Nothing to do, and nothing to scroll. Most keystrokes land here: a line only
  // wraps occasionally, and re-scrolling on every character is what made typing
  // shove the conversation up and down.
  if (wanted === current) {
    input.style.height = current;
    return;
  }

  const wasDown = atBottom();
  input.style.height = wanted;
  if (wasDown) toBottom();
}
input.addEventListener('input', fitInput);

// Cmd+Enter sends, as in every other composer.
//
// `requestSubmit` rather than `submit`: it raises the submit event, which is what
// posts in the background and keeps the text until the message lands. `submit`
// would bypass all of that and navigate.
input.addEventListener('keydown', (event) => {
  if (event.key !== 'Enter' || !(event.metaKey || event.ctrlKey)) return;

  event.preventDefault();
  composer.requestSubmit();
});

// Which configurations the next message runs under.
//
// Kept here rather than on the server: nothing is applied until a message is
// sent, so this is a choice in progress, not state the conversation has.
const configModal = document.getElementById('config-modal');
const configGroups = document.getElementById('config-groups');
let chosenConfigs = new Set();
let configsLoaded = false;

document.getElementById('open-config').addEventListener('click', () => {
  nav.open = false;
  configModal.showModal();
  loadConfigs();
});

async function loadConfigs() {
  if (configsLoaded) return;

  try {
    const r = await fetch('/configs');
    if (!r.ok) throw new Error(r.status);

    const entries = await r.json();
    configsLoaded = true;
    configGroups.textContent = '';

    if (entries.length === 0) {
      const empty = document.createElement('p');
      empty.className = 'config-note';
      empty.textContent = 'No configurations found on the load paths.';
      configGroups.append(empty);
      return;
    }

    // Grouped by namespace, relying on the host's sort by segment: entries in one
    // namespace share a prefix, so they arrive together.
    let group = null;
    let namespace = null;

    for (const entry of entries) {
      if (group === null || entry.namespace !== namespace) {
        namespace = entry.namespace;
        group = document.createElement('fieldset');
        const legend = document.createElement('legend');
        legend.textContent = namespace || 'General';
        group.append(legend);
        configGroups.append(group);
      }

      const label = document.createElement('label');
      label.className = 'config-option';

      const box = document.createElement('input');
      box.type = 'checkbox';
      box.value = entry.segment;
      box.checked = chosenConfigs.has(entry.segment);

      const name = document.createElement('span');
      name.textContent = entry.name;

      label.append(box, name);
      group.append(label);
    }
  } catch (e) {
    configGroups.textContent = '';
    const failed = document.createElement('p');
    failed.className = 'composer-error';
    failed.textContent = 'Could not read the available configurations.';
    configGroups.append(failed);
  }
}

// Cancel leaves the previous choice alone; apply replaces it with what is ticked.
configModal.addEventListener('close', () => {
  if (configModal.returnValue !== 'apply') return;

  chosenConfigs = new Set(
    Array.from(configGroups.querySelectorAll('input:checked')).map(box => box.value),
  );

  document.getElementById('open-config').classList.toggle('active', chosenConfigs.size > 0);
});

// A larger field for a longer reply.
//
// The same value, not a second draft: the small field is the one that gets sent,
// so this copies in on open and back out on close.
const expanded = document.getElementById('expanded');
const expandModal = document.getElementById('expand-modal');

document.getElementById('expand').addEventListener('click', () => {
  expanded.value = input.value;
  expandModal.showModal();
  expanded.focus();
  expanded.setSelectionRange(expanded.value.length, expanded.value.length);
});

expandModal.addEventListener('close', () => {
  input.value = expanded.value;
  fitInput();
  saveDraft();
});

// Quote what is selected.
//
// Back to markdown rather than plain text: the transcript is rendered markdown, so
// a quote of it should read as what was written, not as its rendering flattened.
// A subset — the block and inline elements the renderer emits — and anything else
// falls through to its text.
function toMarkdown(node) {
  if (node.nodeType === Node.TEXT_NODE) return node.textContent;
  if (node.nodeType !== Node.ELEMENT_NODE) return '';

  const inner = () => Array.from(node.childNodes).map(toMarkdown).join('');

  switch (node.tagName) {
    case 'BR': return '\n';
    case 'P': return inner() + '\n\n';
    case 'PRE': return '```\n' + node.textContent.replace(/\n$/, '') + '\n```\n\n';
    case 'CODE': return node.closest('pre') ? node.textContent : '`' + inner() + '`';
    case 'STRONG': case 'B': return '**' + inner() + '**';
    case 'EM': case 'I': return '*' + inner() + '*';
    case 'DEL': return '~~' + inner() + '~~';
    case 'A': return '[' + inner() + '](' + (node.getAttribute('href') ?? '') + ')';
    case 'LI': return '- ' + inner().trim() + '\n';
    case 'UL': case 'OL': return inner() + '\n';
    case 'BLOCKQUOTE':
      return inner().trim().split('\n').map(line => '> ' + line).join('\n') + '\n\n';
    case 'H1': case 'H2': case 'H3': case 'H4': case 'H5': case 'H6':
      return '#'.repeat(Number(node.tagName[1])) + ' ' + inner() + '\n\n';
    case 'HR': return '---\n\n';
    default: return inner();
  }
}

document.getElementById('quote').addEventListener('click', () => {
  const selection = getSelection();
  if (!selection || selection.isCollapsed) return;

  // The selection as its own tree, so partial elements come back whole rather
  // than as the text between two points.
  const fragment = selection.getRangeAt(0).cloneContents();
  const markdown = Array.from(fragment.childNodes).map(toMarkdown).join('').trim();
  if (!markdown) return;

  const quoted = markdown.split('\n').map(line => ('> ' + line).trimEnd()).join('\n');

  // Appended, so quoting twice builds up rather than replacing.
  input.value = input.value ? input.value.replace(/\s*$/, '\n\n') + quoted + '\n\n' : quoted + '\n\n';
  fitInput();
  input.focus();
  input.setSelectionRange(input.value.length, input.value.length);
  saveDraft();
});

// Renaming, in place.
//
// The heading and the field swap rather than the heading becoming editable: a
// form gets Enter-to-submit and a real input for free, and the title is short
// enough that losing the heading's styling for a moment costs nothing.
const heading = document.getElementById('title');
const renameForm = document.getElementById('rename-form');
const titleField = document.getElementById('title-field');
const renameButton = document.getElementById('rename');

function showRename(editing) {
  heading.hidden = editing;
  renameButton.hidden = editing;
  renameForm.hidden = !editing;

  if (editing) {
    titleField.value = heading.textContent.trim();
    titleField.focus();
    titleField.select();
  }
}

renameButton.addEventListener('click', () => showRename(true));
document.getElementById('rename-cancel').addEventListener('click', () => showRename(false));

// Escape backs out; Enter commits. Both are handled here rather than left to the
// form, because a form inside a header is not reliably submitted by Enter and
// Escape would otherwise reach the dialog machinery instead.
titleField.addEventListener('keydown', (event) => {
  if (event.key === 'Escape') {
    event.preventDefault();
    event.stopPropagation();
    showRename(false);
    return;
  }

  if (event.key === 'Enter') {
    event.preventDefault();
    renameForm.requestSubmit();
  }
});

renameForm.addEventListener('submit', async (event) => {
  event.preventDefault();

  const title = titleField.value.trim();

  try {
    const r = await fetch(renameForm.action, {
      method: 'POST',
      headers: {
        'content-type': 'application/x-www-form-urlencoded',
        accept: 'application/json',
      },
      body: new URLSearchParams({ title }),
    });
    if (!r.ok) throw new Error(r.status);

    // Applied here rather than reloading, so the transcript and the scroll stay
    // where they are.
    heading.textContent = title || 'Untitled';
    document.title = (title || 'Untitled') + ' - JP';
    showRename(false);
  } catch (e) {
    titleField.setCustomValidity('Could not rename this conversation.');
    titleField.reportValidity();
    titleField.setCustomValidity('');
  }
});

// Stopping posts in the background, like everything else here.
//
// The control is a real form so it works without script, but letting it navigate
// means a full reload — and the page that comes back still shows the turn as
// running, because the lock is held until it has finished unwinding. That reads
// as the button having done nothing.
//
// Delegated, so it covers both the form the server renders and the one the poller
// builds. A handler on the form itself may already have asked for confirmation
// and been declined; that shows up as the event being cancelled, and is left
// alone.
status.addEventListener('submit', async (event) => {
  const form = event.target.closest('.composer-stop');
  if (!form || event.defaultPrevented) return;

  event.preventDefault();

  try {
    await fetch(form.action, {
      method: 'POST',
      headers: { accept: 'application/json' },
    });
  } catch (e) {
    // The poll reports what actually happened either way.
  }

  poll();
});

// The header is a way back to the top, matching the platform gesture this page
// cannot receive: with the document pinned to the window, there is no window
// scroll position for iOS to reset when the status bar is tapped.
//
// Ignores clicks on the links inside it, which have somewhere else to go.
document.querySelector('.page-header').addEventListener('click', (event) => {
  // The links and the rename controls inside it have their own jobs.
  if (event.target.closest('a, button, form')) return;

  transcript.scrollTop = 0;
});

// Older events, fetched as the reader scrolls back to them.
//
// The page holds a window rather than the whole conversation: a long one is
// thousands of nodes, and painting them all is what made scrolling crawl.
//
// Prepending moves everything down by the height of what was added, so the scroll
// position is corrected by that much — otherwise the reader is thrown backwards
// by exactly the amount they just gained.
async function loadOlder(all) {
  if (loadingOlder || !older()) return false;
  loadingOlder = true;

  try {
    const r = await fetch(
      url + '?before=' + box.dataset.first + (all ? '&all=1' : ''),
    );
    if (!r.ok) return false;

    const d = await r.json();
    if (d.html === undefined) return false;

    const before = transcript.scrollHeight;
    box.insertAdjacentHTML('afterbegin', d.html);
    box.dataset.first = d.from;
    transcript.scrollTop += transcript.scrollHeight - before;

    return true;
  } catch (e) {
    return false;
  } finally {
    loadingOlder = false;
  }
}

// Everything older, in one request rather than a window at a time: a conversation
// of several thousand events is dozens of round trips that way, and the reader is
// left watching the scrollbar twitch.
const loadAllOlder = () => loadOlder(true);

// Fetched well before the reader arrives, so scrolling back at a normal pace
// never meets the top of what is loaded. A window is large enough that this
// rarely fires twice in a row.
transcript.addEventListener('scroll', () => {
  if (transcript.scrollTop < 3000) loadOlder(false);
}, { passive: true });

// Touch platforms take the whole conversation up front.
//
// Windowing exists because painting a long transcript is slow on a desktop
// browser; on touch it never was, and there the fetching is the only thing the
// reader would notice. So they get what they had: everything, once, and no pauses
// while scrolling back.
if (!matchMedia('(hover: hover) and (pointer: fine)').matches) {
  addEventListener('load', () => loadAllOlder());
}

// Jumps between turns.
//
// A turn starts at its separator, so those are the anchors. `prev` and `next` are
// relative to what is at the top of the view rather than to a remembered position,
// which keeps the buttons honest after scrolling by hand.
const nav = document.getElementById('nav');

function turnStarts() {
  return Array.from(transcript.querySelectorAll('.turn-separator'));
}

function jump(where) {
  if (where === 'top') {
    // The whole conversation, then the top of it. Anything less would land at the
    // top of the window rather than the top of the conversation, which is not what
    // the button says.
    loadAllOlder().then(() => { transcript.scrollTop = 0; });
    return;
  }

  if (where === 'bottom') {
    toBottom();
    return;
  }

  const starts = turnStarts();
  if (starts.length === 0) return;

  // Offsets within the scroller, which is what `scrollTop` is measured against.
  const tops = starts.map(el => el.offsetTop - transcript.offsetTop);

  // A few pixels of slack, so a jump that lands a hair past a separator does not
  // count as already being below it.
  const here = transcript.scrollTop + 2;

  const target = where === 'next'
    ? tops.find(top => top > here)
    : tops.filter(top => top < here - 4).pop();

  if (target !== undefined) transcript.scrollTop = target;
}

nav.addEventListener('click', (event) => {
  const button = event.target.closest('[data-nav]');
  if (!button) return;

  jump(button.dataset.nav);
});

// Deliberately no close-on-outside-click: the menu is for jumping around a
// conversation, and every jump is a click on the thing being navigated. Closing
// on those would mean reopening it between each one.

// The transcript is the scroller, not the window.
const atBottom = () =>
  transcript.scrollTop + transcript.clientHeight >= transcript.scrollHeight - 80;
// Scroll to the end.
//
// An element is scrolled into view rather than `scrollTop` set to `scrollHeight`,
// because that height is a lie while messages further up are still skipped: it is
// built from their estimates, so setting it lands short and the view stops an
// event or two above the newest.
const toBottom = () => {
  // The anchor, not the last child: the last child is the status row, which is
  // `display: none` whenever there is nothing to say, and scrolling a box-less
  // element into view does nothing at all.
  //
  // The anchor always has a box, and is not a message — so it is never one of the
  // elements whose height is being estimated. Aiming at it is the one way to reach
  // the true end while the extent is still a guess.
  end.scrollIntoView({ block: 'end' });
};

// Stay at the end until it stops moving.
//
// One scroll is not enough after the transcript is replaced. Every message is
// recreated, so every one of them is unseen again and reports the placeholder
// height instead of its own; the extent collapses, the scroll lands on that false
// end, and then the messages near the viewport are laid out for real and the true
// end moves away below. From the reader's seat the view drifts upward, which is
// the opposite of what was asked for.
//
// So: scroll, look at whether the extent changed, and go again until it holds
// still. It converges quickly, because each pass realises the messages it just
// scrolled past.
//
// Bounded in time rather than in passes, because a slow frame should not end the
// chase early, and an extent that never settles must not spin forever.
let settling = 0;

function stayAtBottom() {
  const until = performance.now() + 600;

  // A pass already running just gets more time, rather than a second pass
  // racing it.
  if (settling > 0) {
    settling = until;
    return;
  }

  settling = until;
  let previous = -1;

  const step = () => {
    toBottom();

    const height = transcript.scrollHeight;
    const held = height === previous;
    previous = height;

    if (!held && performance.now() < settling) {
      requestAnimationFrame(step);
      return;
    }

    settling = 0;
  };

  requestAnimationFrame(step);
}

// Registered here rather than at the declaration: `fitDock` reads the scroll
// helpers above, which are not initialised until this point.
if (window.ResizeObserver) new ResizeObserver(fitDock).observe(dock);

fitInput();
fitDock();
toBottom();

// Reveal once the conversation is in place.
//
// A long transcript paints top-down over seconds, so without this the reader
// watches it stream past from the beginning and then jump to the end. The overlay
// covers that, and comes off after a frame in which the scroll has been applied.
// Settled, not applied: messages out of view are skipped until scrolled near and
// report an estimated height until then, so scrolling to the end lands short, the
// messages there are laid out for real, and the end moves. Repeating until the
// height stops changing converges on the actual bottom, and the veil covers it.
function reveal() {
  // The same chase the poller uses after a swap: on first paint no message has
  // been measured either, so the end moves for the same reason.
  stayAtBottom();

  // Uncovered once that has had its window, so the settling happens behind the
  // veil rather than in front of the reader.
  setTimeout(() => document.documentElement.classList.add('ready'), 650);
}

if (document.readyState === 'complete') reveal();
else addEventListener('load', reveal);

// A cap, so a page that never fires `load` — a stalled image, a slow font — is
// still usable. Better to reveal a conversation mid-scroll than to hold a blank
// screen over a working page.
setTimeout(() => document.documentElement.classList.add('ready'), 3000);

// Both edges, and before the first viewport event: the keyboard starts moving on
// focus and on blur, and iOS may report nothing until it has finished. Without the
// blur half, the column stays at its keyboard-open height after the keyboard has
// gone.
input.addEventListener('focus', () => { fitInput(); trackKeyboard(); });
input.addEventListener('blur', () => { fitInput(); trackKeyboard(); });

// Pre-emptively, before iOS snapshots the page with the field still focused.
addEventListener('pagehide', blurField);

// Draft sync.
//
// The same file `jp query` uses, so a message can be started in a terminal and
// finished here, or the reverse, and a reload never loses what was typed.
//
// Writes are conditional on the revision last read: if the terminal changed the
// draft in the meantime the host refuses, hands back what is on disk, and this
// says so rather than overwriting it. Losing typing is the thing being avoided,
// so a refusal is the correct outcome, not a failure.
let revision = null;
let saving = false;

// The newest content waiting for an in-flight save to finish. A boolean would
// lose it: the retry would re-read the field, which is wrong for a save that was
// asked to store something specific.
let queued = null;

// Bounds the automatic re-save below, so a draft that keeps being cleared under
// us cannot turn into a request loop.
let retried = false;

async function loadDraft() {
  try {
    const r = await fetch(draftUrl);
    if (!r.ok) return;
    const d = await r.json();
    revision = d.revision ?? null;

    // Never clobber something already being typed — a slow read must not win
    // against the person at the keyboard.
    if (d.content && !input.value) {
      input.value = d.content;
      fitInput();
    }
  } catch (e) {
    // No draft is a normal state; a failed read is not worth a message.
  }
}

// `content` defaults to what is in the field. Passing it explicitly is how the
// submit path clears the stored draft without touching the field — emptying the
// textarea during submit makes the form post an empty message, because the
// browser serialises it after the handler runs.
async function saveDraft(content) {
  const text = content ?? input.value;

  // Nothing here and nothing recorded means nothing to say. Writing anyway would
  // assert there is no draft, against one another device just wrote, and come back
  // as a conflict about text this one never had.
  if (!text && revision === null) return;

  if (saving) { queued = text; return; }
  saving = true;

  try {
    const r = await fetch(draftUrl, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      // Survives the navigation a submit triggers.
      keepalive: true,
      body: JSON.stringify({ content: text, revision }),
    });
    if (!r.ok) return;
    const d = await r.json();
    revision = d.revision ?? null;

    // A draft that has gone *empty* underneath us is not somebody else's edit:
    // the host clears it when it turns a message into a request, which leaves
    // this page holding a revision for a file that no longer exists. Adopt the
    // new revision and put the text back, rather than reporting a conflict that
    // has no other party.
    if (d.conflict && !d.content) {
      draftNote.hidden = true;

      if (input.value && !retried) {
        retried = true;
        queued = input.value;
      }
    } else if (d.conflict) {
      draftNote.textContent =
        'This draft was changed elsewhere. Yours is kept here; the other version '
        + 'is on disk.';
      draftNote.hidden = false;
    } else {
      draftNote.hidden = true;
      retried = false;
    }
  } catch (e) {
    // Offline or mid-restart. The next keystroke tries again.
  } finally {
    saving = false;
    if (queued !== null) {
      const next = queued;
      queued = null;
      saveDraft(next);
    }
  }
}

let saveTimer = null;
input.addEventListener('input', () => {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => saveDraft(), 600);
});

// Leaving the field, or the page, is the last chance to keep what is there —
// unless it has just been sent, in which case saving would resurrect it.
input.addEventListener('blur', () => { if (!submitted) saveDraft(); });
addEventListener('pagehide', () => { if (!submitted) saveDraft(); });

// Send without navigating.
//
// A form post would reload the page: the transcript is rebuilt, the scroll jumps,
// the draft is re-read, and the composer loses focus and its height — all to show
// a message the poller was about to bring in anyway. Posting in the background
// leaves the page exactly as it was.
//
// The form still works without JavaScript; the handler is what suppresses the
// navigation, and the endpoint answers both shapes.
composer.addEventListener('submit', async (event) => {
  event.preventDefault();

  // In cancel mode the button pulls the message back rather than sending one.
  //
  // No confirmation: this page started the turn moments ago, which is what `own`
  // means. Stopping a turn someone else started asks first, from the indicator's
  // stop button.
  if (cancelling) {
    try {
      await fetch(location.pathname.replace(/\/$/, '') + '/interrupt', {
        method: 'POST',
        headers: { accept: 'application/json' },
      });
    } catch (e) {
      // The poll reports what actually happened either way.
    }
    poll();
    return;
  }

  const content = input.value.trim();
  if (!content) return;

  submitted = true;
  clearTimeout(saveTimer);
  lockComposer(true);

  // Left in the field on purpose, and cleared only once the message is in the
  // transcript. A turn can be refused after the request has been accepted — the
  // conversation may be locked by another process — and clearing on send would
  // destroy the message on the way to finding that out.
  const landedAbove = Number(box.dataset.count);

  try {
    const response = await fetch(composer.action, {
      method: 'POST',
      headers: {
        'content-type': 'application/x-www-form-urlencoded',
        accept: 'application/json',
      },
      body: (() => {
        const params = new URLSearchParams({ content, client: clientId });
        // One entry per choice: the same shape `--cfg` takes, repeated.
        for (const segment of chosenConfigs) params.append('cfg', segment);
        return params;
      })(),
    });

    const body = await response.json();

    // Refused, not failed: the text stays where it is and the reason is shown.
    if (!response.ok) {
      draftNote.textContent = body.error ?? 'The message was not sent.';
      draftNote.hidden = false;
      lockComposer(false);
      submitted = false;
      return;
    }

    // Rendered by the server from the message just sent, so it is the same markup
    // the transcript will carry and the swap moves nothing.
    draftNote.hidden = true;
    showPending(body.pending);

    // The chase, not one scroll: the ghost is a message like any other, and is no
    // more measured than the rest.
    stayAtBottom();
  } catch (e) {
    lockComposer(false);
    submitted = false;
    return;
  }

  // Free for the next message, including one meant to interrupt this turn.
  submitted = false;
  clearWhenLanded = landedAbove;

  poll();
});

loadDraft();

// The server renders this, so it matches the real request exactly rather than
// approximating it in the DOM.
function showPending(html) {
  if (pending.dataset.html === (html ?? '')) return;
  pending.dataset.html = html ?? '';
  pending.innerHTML = html ?? '';
}

// Which disclosure blocks are open, so a swap doesn't collapse what is being
// read. The transcript only ever grows, so position is a stable key: blocks
// appended by the swap start closed, and everything before them keeps its state.
function openBlocks() {
  return Array.from(box.querySelectorAll('details')).map(d => d.open);
}

function restoreBlocks(open) {
  box.querySelectorAll('details').forEach((d, i) => {
    if (open[i]) d.open = true;
  });
}

function showStatus(running, error, stopMode) {
  const stoppable = stopMode === 'own' || stopMode === 'shared';
  if (error) {
    status.dataset.running = 'false';
    status.textContent = '';
    const p = document.createElement('p');
    p.className = 'composer-error';
    p.textContent = error;
    status.append(p);
    return;
  }

  const state = String(running) + ':' + String(stopMode);
  if (state === status.dataset.running) return;
  status.dataset.running = state;
  status.textContent = '';
  if (running) {
    const s = document.createElement('span');
    s.className = 'composer-working';
    s.role = 'status';
    s.ariaLabel = 'Working';
    s.append(...[0, 1, 2].map(() => document.createElement('i')));
    status.append(s);

    // Offered only for a turn this server is running: an interrupt reaches its
    // own host, and a turn started elsewhere is another process's to stop.
    if (stopMode === 'unreachable') {
      const why = document.createElement('span');
      why.className = 'composer-hint';
      why.textContent =
        'Another process is running this turn; it can only be stopped there.';
      status.append(why);
    }

    if (stoppable) {
      const stop = document.createElement('form');
      stop.className = 'composer-stop';
      stop.method = 'post';
      stop.action = base + '/interrupt';

      // Somebody else's work: stoppable, since this server is running it, but
      // not without asking. Their window has no say in it and no warning that
      // it happened.
      if (stopMode === 'shared') {
        stop.addEventListener('submit', (event) => {
          const ok = confirm(
            'This turn was started in another window. Stop it anyway?',
          );
          if (!ok) event.preventDefault();
        });
      }

      const button = document.createElement('button');
      button.type = 'submit';
      button.title = 'Stop';
      button.setAttribute('aria-label', 'Stop');
      // The same barred circle the server renders, so the swap is invisible.
      // Single-quoted attributes: this whole script is a Rust string, and a double
      // quote would end it.
      button.innerHTML =
        `<svg viewBox='0 0 24 24' width='18' height='18' fill='none'`
        + ` stroke='currentColor' stroke-width='2' stroke-linecap='round'`
        + ` aria-hidden='true'><circle cx='12' cy='12' r='9'></circle>`
        + `<path d='M8 12h8'></path></svg>`;

      stop.append(button);
      status.append(stop);
    }
  }

  // The indicator sits at the end of the conversation, so appearing or going
  // away changes its height.
  if (atBottom()) toBottom();
}

// Polls overlap: the timer's and the one fired right after a submit. A slow
// earlier response arriving after a newer one would put the older state back,
// blanking a working indicator that had just appeared. Only the newest applies.
let pollSeq = 0;

async function poll() {
  const seq = ++pollSeq;

  try {
    // The count we already have, so the answer can leave the transcript out when
    // it has not changed. Rendering it is the whole conversation's markdown, and
    // most polls change nothing.
    const r = await fetch(
      url + '?count=' + box.dataset.count + '&client=' + encodeURIComponent(clientId),
    );
    if (!r.ok || seq !== pollSeq) return;
    const d = await r.json();
    if (seq !== pollSeq) return;

    // A different server means this page's markup and styles are stale. Data
    // recovers by itself; the page cannot.
    if (boot === null) {
      boot = d.boot;
    } else if (d.boot !== boot) {
      reload.hidden = false;
    }

    // Taken before anything is inserted. Asking afterwards always says no: the
    // content has grown by then, so the scroll position is no longer near the
    // end even though it was a moment ago.
    const wasDown = atBottom();

    // Present only when the server had something the page does not.
    //
    // `from` says whether it continues the transcript or replaces it. Continuing
    // is the normal case, and it leaves every message already on the page alone:
    // their open blocks stay open, their measured heights stay measured, and the
    // scroll position means the same thing before and after.
    if (d.html !== undefined) {
      box.dataset.count = d.count;

      const first = Number(box.dataset.first);

      if (d.from < first) {
        // Older than anything held, which means the transcript was rewritten
        // under us — compacted, or edited on disk. Nothing can be carried across,
        // so the open blocks are restored by position.
        const open = openBlocks();
        box.innerHTML = d.html;
        box.dataset.first = d.from;
        restoreBlocks(open);
      } else {
        // Everything from `from` onward is replaced, not appended.
        //
        // Usually that is nothing but new events on the end. It is more when the
        // tail is still moving: a tool call shows its request first and its result
        // later, and the entry that has to change is one the page already holds.
        // With calls running in parallel that reaches back to the earliest one
        // still waiting, so settled calls after it are rewritten too.
        //
        // Open blocks are captured across the whole transcript, not just the part
        // being kept: the replaced events come back in the same order, so their
        // positions still line up — and a disclosure the reader opened inside a
        // finished tool call must not snap shut once a second because an earlier
        // call is still running.
        const keep = d.from - first;
        const open = openBlocks();

        while (box.children.length > keep) box.lastElementChild.remove();
        box.insertAdjacentHTML('beforeend', d.html);
        restoreBlocks(open);
      }
    }

    // The message reached the transcript, so the field can let go of it.
    if (clearWhenLanded !== null && d.count > clearWhenLanded) {
      clearWhenLanded = null;
      input.value = '';
      fitInput();
      saveDraft('');
    }

    // A refused turn leaves the text where it is, to be sent again or edited.
    if (d.error) clearWhenLanded = null;

    showPending(d.pending);

    // The ghost and the indicator mean different things and must not both be up:
    // the ghost says the request has not been taken yet, the dots say a reply is
    // being written. Together they read as an answer to a message that has not
    // arrived.
    // `stop` is a mode, not a flag: `own`, `shared`, `none` or `unreachable`.
    // `showStatus` wants the mode, because it renders differently for each; the
    // rest here only needs to know whether stopping is possible at all.
    const stoppable = d.stop === 'own' || d.stop === 'shared';

    const awaitingSend = Boolean(d.pending);
    showStatus(!awaitingSend && d.running, d.error, d.stop);

    // While the ghost is up, Send offers to pull the message back instead — the
    // window in which a typo is still worth catching. Once it lands, the button
    // returns to Send and the indicator takes over the stopping.
    setCancelMode(awaitingSend && stoppable);

    // The field is held while the message is in flight, but the button is not: it
    // is the way out of that state.
    input.readOnly = clearWhenLanded !== null;
    send.disabled = cancelling ? false : d.running && !stoppable;

    // Anything newly arrived needs the chase, appended or not: a message that has
    // not been measured reports a placeholder height, and for a tall one that is a
    // large undershoot — scroll to that end and the real height then pushes the
    // end below the fold. A short one overshoots instead, which is why single-line
    // tool calls always looked fine.
    //
    // Everything else has not moved the extent, so one scroll is enough.
    if (wasDown) {
      if (d.html !== undefined) stayAtBottom();
      else toBottom();
    }
  } catch (e) {
    // A failed poll is not worth reporting: the next one is a second away.
  }
}

// Attentive while a turn is live, lazy when idle.
status.dataset.running = String(!!status.querySelector('.composer-working'))
  + ':' + String(!!status.querySelector('.composer-stop'));
// The flag is `running:stoppable`, so match the prefix rather than the whole.
(function tick() {
  const live = () => status.dataset.running.startsWith('true');
  poll().finally(() => setTimeout(tick, live() ? 1000 : 3000));
})();
addEventListener('focus', poll);
";
