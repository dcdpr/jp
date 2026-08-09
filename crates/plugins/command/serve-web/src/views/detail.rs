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
pub(crate) fn render(
    id: &str,
    title: &str,
    events: &[RenderedEvent],
    running: bool,
    stoppable: bool,
) -> Markup {
    layout::page(title, html! {
        header class="page-header" {
            a href="/conversations" class="back" { "← Conversations" }
            h1 { (title) }
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

            main id="transcript" class="conversation-detail" {
                // Replaced wholesale by the poller when the count changes.
                div id="messages" data-count=(events.len()) {
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
const transcript = document.getElementById('transcript');
const box = document.getElementById('messages');
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
const dock = document.querySelector('.composer-dock');
function fitDock() {
  const wasDown = atBottom();
  document.documentElement.style.setProperty('--dock', dock.offsetHeight + 'px');
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
function fitInput() {
  const wasDown = atBottom();

  if (document.activeElement === input) {
    // `auto` first so the field can shrink as well as grow, then the final height,
    // both in one synchronous block: the browser paints once, at the end. Reading
    // and restoring in between is what made this flicker.
    input.style.height = 'auto';
    input.style.height = Math.min(input.scrollHeight, visible() / 3) + 'px';
  } else {
    input.style.height = '';
  }

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
    transcript.scrollTop = 0;
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

// Anywhere else closes it, as a menu should.
document.addEventListener('click', (event) => {
  if (!nav.contains(event.target)) nav.open = false;
});

// The transcript is the scroller, not the window.
const atBottom = () =>
  transcript.scrollTop + transcript.clientHeight >= transcript.scrollHeight - 80;
const toBottom = () => { transcript.scrollTop = transcript.scrollHeight; };

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
function reveal() {
  toBottom();
  requestAnimationFrame(() => {
    document.documentElement.classList.add('ready');
  });
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
      body: new URLSearchParams({ content }),
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
    toBottom();
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

function showStatus(running, error, stoppable) {
  if (error) {
    status.dataset.running = 'false';
    status.textContent = '';
    const p = document.createElement('p');
    p.className = 'composer-error';
    p.textContent = error;
    status.append(p);
    return;
  }

  const state = String(running) + ':' + String(stoppable);
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
    if (!stoppable) {
      const why = document.createElement('span');
      why.className = 'composer-hint';
      why.textContent = 'Another process is running this turn.';
      status.append(why);
    }

    if (stoppable) {
      const stop = document.createElement('form');
      stop.className = 'composer-stop';
      stop.method = 'post';
      stop.action = location.pathname.replace(/\/$/, '') + '/interrupt';

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
    const r = await fetch(url + '?count=' + box.dataset.count);
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

    // Present only when the server had something newer, which is also the only
    // time reading should be interrupted by a swap.
    if (d.html !== undefined) {
      const open = openBlocks();
      box.dataset.count = d.count;
      box.innerHTML = d.html;
      restoreBlocks(open);
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

    lockComposer(clearWhenLanded !== null);

    showPending(d.pending);
    showStatus(d.running, d.error, d.stoppable);

    // Blocked for the length of a turn another process is running, where the lock
    // would refuse it anyway. A turn this server owns leaves it enabled, because
    // sending is how you interrupt one.
    if (clearWhenLanded === null) send.disabled = d.running && !d.stoppable;

    if (wasDown) toBottom();
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
