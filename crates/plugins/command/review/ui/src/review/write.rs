//! What the review page asks the server to change, and what it reads of the
//! reader's hand in the file: the selection a comment is offered for, the
//! passage or the link a click falls on, and the card a click picks.

use std::{cell::Cell, rc::Rc, time::Duration};

use jp_review_api::{AnchorRequest, Comment, Draft as Drafted, Recorded, State, StopRequest};
use leptos_dom::helpers::{TimeoutHandle, set_timeout, window_event_listener_untyped};
use reactive_graph::{
    effect::Effect, owner::on_cleanup, prelude::*, signal::RwSignal, spawn_local,
};
use serde_json::Value;
use tachys::{
    html::{
        element::{button, div, span},
        event as ev,
    },
    prelude::*,
};
use wasm_bindgen::JsCast;
use web_sys::{Element, Event, KeyboardEvent, MouseEvent, Node, PointerEvent};

use super::{Page, Reveal};
use crate::{
    actions::{self, Action},
    capture,
    client::{self, Call},
    compose::{Mark, PLAIN, Pending, SETTLE},
    composer::{Draft, Target},
    dom::{self, document_listener},
    layout, link,
    popover::{Anchor, Point, Rect},
    route::{ReviewView, Route},
    seat, selection, text,
    widget::{self, Tone},
};

/// The selector of a link in the file's text.
const LINK: &str = "a[href]";

/// How long after a mouse lets go of a selection the selection is read: once
/// the browser has settled what the drag selected.
const RELEASED: Duration = Duration::from_millis(10);

/// For how long after a mouse let go a selection counts as the one the mouse
/// made, in milliseconds; one read later was made some other way.
const RELEASE_WINDOW: f64 = 1000.0;

/// Half the height of a line of the file's text, in CSS pixels: how far over
/// and under a click the line clicked on reaches.
const HALF_LINE: f64 = 14.0;

/// Whether the review takes changes.
pub(super) fn writable(page: Page) -> bool {
    page.review.with(|review| {
        review
            .as_ref()
            .is_some_and(|review| review.head.status == State::Open)
    })
}

/// What a card does with its comment, while the review takes changes.
pub(super) fn actor(
    page: Page,
) -> Option<impl Fn(Action, &Comment) + Clone + Send + Sync + 'static> {
    writable(page).then_some(move |action: Action, comment: &Comment| act(page, action, comment))
}

/// Do `action` with `comment`: open the composer on it, or ask the server.
fn act(page: Page, action: Action, comment: &Comment) {
    let Some(number) = actions::number(comment) else {
        return;
    };
    let id = page.compose().id.get_value();

    match action {
        // Picked too, so the comment's thread shows once the composer closes.
        Action::Edit => {
            if let Some(draft) = Draft::edit(comment) {
                page.picked.set(Some(number));
                page.compose().open(draft);
            }
        }
        Action::Reply | Action::EditReply => {
            if let Some(draft) = Draft::reply(comment) {
                page.picked.set(Some(number));
                page.compose().open(draft);
            }
        }
        Action::Withdraw | Action::WithdrawReply => {
            let question = if action == Action::Withdraw {
                format!("Withdraw draft #{number}?")
            } else {
                format!("Withdraw your draft reply to #{number}?")
            };
            if dom::confirm(&question) {
                // The draft the question named, which the server refuses to
                // withdraw once the comment holds the other.
                let draft = if action == Action::Withdraw {
                    Drafted::Comment
                } else {
                    Drafted::Reply
                };
                write(page, Call::delete(client::withdraw_url(&id, number, draft)));
            }
        }
        Action::Resolve => write(page, Call::post_empty(client::resolve_url(&id, number))),
        Action::Reopen => write(
            page,
            Call::post_empty(client::reopen_comment_url(&id, number)),
        ),
    }
}

/// Make `call`, polling at once when the server took it, and saying why when it
/// didn't.
fn write(page: Page, call: Call) {
    spawn_local(async move {
        let answer = client::send::<Value>(call).await;
        if page.gone() {
            return;
        }
        match answer {
            Ok(_) => {
                page.notice.set(None);
                page.compose().refreshed();
            }
            Err(error) => page.notice.set(Some(error)),
        }
    });
}

/// Whether the review is closed, and takes no changes until it is opened again.
pub(super) fn closed(page: Page) -> bool {
    page.review.with(|review| {
        review
            .as_ref()
            .is_some_and(|review| review.head.status == State::Closed)
    })
}

/// Mark what `mark` says in the file shown, and take the offer of it away.
pub(super) fn mark(page: Page, mark: Mark) {
    let compose = page.compose();
    let id = compose.id.get_value();
    let call = match mark {
        Mark::Approve(target) => {
            place(&target, None).map(|place| Call::post(client::approvals_url(&id), &place))
        }
        Mark::ReadTo(target) => place(&target, block_end(page, &target))
            .map(|place| Call::put(client::read_url(&id), &place)),
        Mark::Unapprove(number) => Some(Call::delete(client::approval_url(&id, number))),
    };

    compose.pending.set(None);
    capture::clear();
    if let Some(call) = call {
        write(page, call);
    }
}

/// The range `target` covers, as a request names it, or the place `end` in its
/// file when that is given; `None` for a target that is no range.
fn place(target: &Target, end: Option<usize>) -> Option<AnchorRequest> {
    let Target::Range {
        path,
        version,
        bytes,
        ..
    } = target
    else {
        return None;
    };

    Some(AnchorRequest {
        version: Some(*version),
        path: Some(path.clone()),
        range: None,
        bytes: Some(end.map_or(*bytes, |end| [end, end])),
    })
}

/// The byte of the file at which the block ends that the range `target` covers
/// ends in, as the file is drawn; `None` while it isn't.
fn block_end(page: Page, target: &Target) -> Option<usize> {
    let Target::Range { bytes, .. } = target else {
        return None;
    };
    let pane = page.pane.get_untracked()?;

    page.file.with_untracked(|file| {
        let source = &file.as_ref()?.as_ref().ok()?.source;
        seat::end(&seat::under(&pane, source, *bytes)?)
    })
}

/// Forget how far the file shown was read.
pub(super) fn forget_read(page: Page) {
    let Some(path) = page.shown.get_untracked() else {
        return;
    };
    let id = page.compose().id.get_value();

    write(page, Call::delete(client::unread_url(&id, &path)));
}

/// The top bar's control: sending the comments that wait to the assistant, or
/// stopping the pass this server runs; and opening a closed review again.
pub(super) fn controls(page: Page) -> impl RenderHtml {
    move || {
        if !writable(page) {
            return closed(page).then(|| {
                widget::action(Tone::Plain, "Reopen", move || {
                    let id = page.compose().id.get_value();
                    write(page, Call::post_empty(client::reopen_url(&id)));
                })
                .into_any()
            });
        }
        let running = page
            .status
            .with(|status| status.as_ref().and_then(|status| status.running.clone()));
        let pending = page.review.with(|review| {
            review
                .as_ref()
                .map_or(0, |review| actions::pending(&review.comments))
        });

        Some(match running {
            None => {
                let (wide, narrow) = text::send(pending);
                widget::action(
                    Tone::Primary,
                    widget::wide_or_narrow(wide, narrow),
                    move || {
                        page.compose().open(Draft::pass());
                    },
                )
                .into_any()
            }
            Some(running) if running.stoppable => widget::action(Tone::Plain, "Stop", move || {
                stop(page, running.number);
            })
            .into_any(),
            Some(_) => ().into_any(),
        })
    }
}

/// Stop pass `number`; the server stops nothing when another pass runs.
fn stop(page: Page, number: u32) {
    let id = page.compose().id.get_value();
    write(
        page,
        Call::post(client::stop_url(&id), &StopRequest { pass: Some(number) }),
    );
}

/// The button that starts a comment on no passage of the file, in a review that
/// takes changes.
///
/// A review of one file has one thing such a comment can be on, the review.
/// A review of several asks whether the file shown or the review is meant, by
/// setting `scoping`.
pub(super) fn add_comment(page: Page, scoping: RwSignal<bool>) -> Option<impl RenderHtml> {
    if !writable(page) {
        return None;
    }
    let several = page.review.with(|review| {
        review
            .as_ref()
            .is_some_and(|review| review.head.files.len() > 1)
    });

    Some(
        button()
            .attr("type", "button")
            .class(widget::small_button_class(Tone::Plain))
            .on(ev::click, move |_| {
                if several {
                    scoping.update(|asking| *asking = !*asking);
                } else {
                    page.compose().open(Draft::comment(Target::Review));
                }
            })
            .child("Add comment"),
    )
}

/// The question of what a new comment is on, the file shown or the review,
/// while `scoping` says [`add_comment`] asks it.
pub(super) fn scope(page: Page, scoping: RwSignal<bool>) -> Option<impl RenderHtml> {
    if !scoping.get() || !writable(page) {
        return None;
    }
    let path = page.shown.get()?;
    let version = page.review.with(|review| Some(review.as_ref()?.version))?;
    let name = path.rsplit('/').next().unwrap_or(&path).to_owned();

    // A file's name can be wider than the comments' column.
    let file = span().class("max-w-40 truncate").child(name);

    Some(
        div()
            .class("flex flex-wrap items-center gap-2 pb-2 text-sm text-muted")
            .child((
                "Comment on",
                widget::action(Tone::Plain, file, move || {
                    scoping.set(false);
                    page.compose().open(Draft::comment(Target::File {
                        path: path.clone(),
                        version,
                    }));
                }),
                widget::action(Tone::Plain, "the whole review", move || {
                    scoping.set(false);
                    page.compose().open(Draft::comment(Target::Review));
                }),
            )),
    )
}

/// The button of the notice that the files on disk changed, which records them
/// as a new version and shows it.
pub(super) fn record_button(page: Page) -> Option<impl RenderHtml> {
    writable(page).then(|| {
        widget::action(Tone::Plain, "Record a new version", move || {
            record(page);
        })
    })
}

fn record(page: Page) {
    let call = Call::post_empty(client::versions_url(&page.compose().id.get_value()));
    spawn_local(async move {
        let answer = client::send::<Recorded>(call).await;
        if page.gone() {
            return;
        }
        match answer {
            Ok(_) => {
                let view = page.view.get_untracked();
                page.nav.go(Route::Review(ReviewView {
                    version: None,
                    changes: None,
                    ..view
                }));
                page.notice.set(None);
                page.compose().refreshed();
            }
            Err(error) => page.notice.set(Some(error)),
        }
    });
}

/// Pick the comment numbered `number`, whose card was clicked: bring it out,
/// and scroll the file to its passage, leaving a changes view for the file
/// first.
pub(super) fn pick(page: Page, number: u32) {
    page.reveal.set_value(Some(Reveal::Passage(number)));
    page.picked.set(Some(number));

    let view = page.view.get_untracked();
    if view.changes.is_some() {
        page.nav.go(Route::Review(ReviewView {
            changes: None,
            ..view
        }));
    }
}

/// Take the click `event` in the file's text: follow the link it fell on, or
/// pick the comment whose passage it fell on and show its card; a click on
/// neither picks no comment.
///
/// A click on the passage of the comment picked picks the next comment on that
/// text, when there is one ([`layout::next`]).
pub(super) fn tap(page: Page, event: &MouseEvent) {
    let target = event
        .target()
        .and_then(|target| target.dyn_into::<Element>().ok());
    let within = |selector: &str| {
        target
            .as_ref()
            .and_then(|target| target.closest(selector).ok().flatten())
    };
    // The composer sits in the file's text, and its clicks are its own.
    if within(&format!(".{PLAIN}")).is_some() {
        return;
    }
    if let Some(link) = within(LINK) {
        return follow(page, event, &link);
    }
    // The click that ends a drag selected text, and picked nothing.
    if capture::selecting() {
        return;
    }
    let (Some(pane), Some(file)) = (page.pane.get_untracked(), page.shown.get_untracked()) else {
        return;
    };
    let resolved = page.view.with_untracked(|view| view.resolved);
    let passages = page.review.with_untracked(|review| {
        review
            .as_ref()
            .map_or_default(|review| layout::passages(&review.comments, &file, resolved))
    });
    let at = Point {
        x: f64::from(event.client_x()),
        y: f64::from(event.client_y()),
    };
    let hits = page.file.with_untracked(|shown| {
        shown
            .as_ref()
            .and_then(|shown| shown.as_ref().ok())
            .map_or_default(|shown| capture::hit(&pane, &shown.source, &passages, at))
    });

    let Some(number) = layout::next(&hits, page.picked.get_untracked()) else {
        page.picked.set(None);
        return point_at_approval(page, &pane, &file, at);
    };
    if offers_approval(page) {
        page.compose().pending.set(None);
    }
    // On a narrow screen the comments open over the lower part of the file,
    // which the passage may be in.
    if !dom::wide() {
        page.reveal.set_value(Some(Reveal::Passage(number)));
    }
    page.panel.set(true);
    page.picked.set(Some(number));
}

/// Offer to take back the approval of the passage of `file` drawn under `at` in
/// `pane`, in a review that takes changes; a click on no such passage takes
/// that offer away.
fn point_at_approval(page: Page, pane: &Element, file: &str, at: Point) {
    let compose = page.compose();
    let approved = page.review.with_untracked(|review| {
        review
            .as_ref()
            .map_or_default(|review| layout::approved(&review.approvals, file))
    });
    let hits = page.file.with_untracked(|shown| {
        shown
            .as_ref()
            .and_then(|shown| shown.as_ref().ok())
            .map_or_default(|shown| capture::hit(pane, &shown.source, &approved, at))
    });

    match layout::next(&hits, None).filter(|_| writable(page)) {
        Some(number) => {
            compose.anchor.set(Some(Anchor::Pointer {
                x: at.x,
                backward: false,
                selection: Rect {
                    left: at.x,
                    top: at.y - HALF_LINE,
                    right: at.x,
                    bottom: at.y + HALF_LINE,
                },
            }));
            compose.pending.set(Some(Pending::Approved(number)));
        }
        None if offers_approval(page) => compose.pending.set(None),
        None => {}
    }
}

/// Whether the reader is offered taking an approval back.
fn offers_approval(page: Page) -> bool {
    page.compose()
        .pending
        .with_untracked(|pending| matches!(pending, Some(Pending::Approved(_))))
}

/// Follow `link`, a link in the file's text that the click `event` fell on, to
/// where the file means it to lead ([`link::resolve`]).
///
/// A heading of the file shown is scrolled to, another file of the review is
/// shown in its place, and an address of its own opens in a tab of its own,
/// which keeps the review open.
/// A link to anything else is not followed, and the reader is told why: the
/// page can show the review's files and nothing more of the workspace.
fn follow(page: Page, event: &MouseEvent, link: &Element) {
    let Some(shown) = page.shown.get_untracked() else {
        return;
    };
    let href = link.get_attribute("href").unwrap_or_default();

    let (path, heading) = match link::resolve(&href, &shown) {
        // The browser scrolls to the heading the fragment names.
        link::Target::Heading(_) => return,
        link::Target::Elsewhere => {
            // A click asking for a new tab or window gets one from the browser.
            if dom::plain(event) {
                event.prevent_default();
                dom::open(&href);
            }
            return;
        }
        link::Target::Nowhere => {
            event.prevent_default();
            return page.notice.set(Some(format!(
                "The link to {href} leads out of this review's files."
            )));
        }
        link::Target::File { path, heading } => (path, heading),
    };

    event.prevent_default();
    let tracked = page.review.with_untracked(|review| {
        review
            .as_ref()
            .is_some_and(|review| review.head.files.contains(&path))
    });
    if !tracked {
        return page.notice.set(Some(format!(
            "{path} is not one of this review's files, which are all this page can show."
        )));
    }
    if path == shown {
        return dom::go_to_heading(heading.as_deref().unwrap_or_default());
    }

    show_file(page, path, heading);
}

/// Show the review's file `path` in place of the one shown, at the heading with
/// the anchor `heading`, or from its start.
pub(super) fn show_file(page: Page, path: String, heading: Option<String>) {
    let to = Route::Review(ReviewView {
        file: Some(path),
        ..page.view.get_untracked()
    });
    page.nav.go_to_heading(to, heading.as_deref());
    match heading {
        // Scrolled to once the file is drawn.
        Some(anchor) => page.reveal.set_value(Some(Reveal::Heading(anchor))),
        None => dom::show_top(),
    }
}

/// Scroll to the heading the address names when Back or Forward lands on an
/// entry with one, once the file it is in is drawn.
///
/// A file drawn already is not drawn again, so its heading is scrolled to at
/// once.
pub(super) fn follow_history(page: Page) {
    // Added after the listener of `Nav`, which has set the route by the time
    // this runs.
    let listening = window_event_listener_untyped("popstate", move |_| {
        let Some(anchor) = dom::fragment().filter(|_| !page.gone()) else {
            return;
        };
        let drawn = page.file.with_untracked(|file| {
            let path = file.as_ref()?.as_ref().ok()?.path.clone();
            Some(path)
        });
        let pane = page.pane.get_untracked();
        match pane.filter(|_| drawn.is_some() && drawn == page.shown.get_untracked()) {
            Some(pane) => dom::show_heading(&pane, &anchor),
            None => page.reveal.set_value(Some(Reveal::Heading(anchor))),
        }
    });
    on_cleanup(move || listening.remove());
}

/// Take back, on Escape, the nearest thing the reader was offered or shown: the
/// actions for a selection, then the comment picked, then the comments' sheet.
pub(super) fn follow_keys(page: Page) {
    let listening = window_event_listener_untyped("keydown", move |event| {
        let escape = event
            .dyn_ref::<KeyboardEvent>()
            .is_some_and(|event| event.key() == "Escape");
        // The page may be gone by the time a key is pressed.
        let Some(compose) = page.compose.try_get_value().filter(|_| escape) else {
            return;
        };

        if compose.pending.with_untracked(Option::is_some) {
            compose.pending.set(None);
            capture::clear();
        } else if page.picked.get_untracked().is_some() {
            page.picked.set(None);
        } else {
            page.panel.set(false);
        }
    });
    on_cleanup(move || listening.remove());
}

/// Read the selection in the file once it is made, to offer a comment on it.
///
/// A selection dragged with a mouse is read when the mouse lets go, and offered
/// where it did.
/// Any other, made with a touch screen's handles or the keyboard, is read once
/// it stays as it is for [`SETTLE`].
pub(super) fn follow_selection(page: Page) {
    let waiting = Rc::new(Cell::new(None::<TimeoutHandle>));
    let dragging = Rc::new(Cell::new(false));
    // Where along the line a mouse last let go of a selection, and when.
    let released = Rc::new(Cell::new(None::<(f64, f64)>));

    let read = {
        let released = Rc::clone(&released);
        Rc::new(move |after: Duration| {
            if let Some(handle) = waiting.take() {
                handle.clear();
            }
            let released = Rc::clone(&released);
            waiting.set(set_timeout(move || take_selection(page, released.get()), after).ok());
        })
    };

    document_listener("pointerdown", {
        let dragging = Rc::clone(&dragging);
        move |event| dragging.set(mouse(&event) && in_pane(page, &event))
    });
    document_listener("pointerup", {
        let (dragging, read) = (Rc::clone(&dragging), Rc::clone(&read));
        move |event| {
            if !dragging.replace(false) {
                return;
            }
            if let Some(event) = event.dyn_ref::<MouseEvent>() {
                released.set(Some((f64::from(event.client_x()), dom::now())));
            }
            read(RELEASED);
        }
    });
    document_listener("pointercancel", {
        let dragging = Rc::clone(&dragging);
        move |_| dragging.set(false)
    });
    document_listener("selectionchange", move |_| {
        // A selection still being dragged is read when the mouse lets go.
        if !dragging.get() {
            read(SETTLE);
        }
    });
}

/// Whether a mouse, and not a finger or a pen, caused the pointer event
/// `event`.
fn mouse(event: &Event) -> bool {
    event
        .dyn_ref::<PointerEvent>()
        .is_some_and(|event| event.pointer_type() == "mouse")
}

/// Whether `event` happened in the file's text.
fn in_pane(page: Page, event: &Event) -> bool {
    let target = event
        .target()
        .and_then(|target| target.dyn_into::<Node>().ok());

    page.pane
        .get_untracked()
        .is_some_and(|pane| pane.contains(target.as_ref()))
}

/// Take the selection in the file as what a comment is offered for, in the file
/// view while the composer is closed.
///
/// `released` is where along the line, and when, a mouse last let go of a
/// selection in the file.
fn take_selection(page: Page, released: Option<(f64, f64)>) {
    // The page may be gone by the time the selection settles.
    let Some(compose) = page.compose.try_get_value() else {
        return;
    };
    if compose.draft.with_untracked(Option::is_some)
        || page.view.with_untracked(|view| view.changes.is_some())
    {
        return;
    }
    let Some(element) = page.pane.get_untracked() else {
        return;
    };
    let Some(taken) = page.file.with_untracked(|file| {
        let file = file.as_ref()?.as_ref().ok()?;
        let captured = capture::read(&element, &file.source)?;

        Some(captured.map(|bytes| {
            Target::Range {
                path: file.path.clone(),
                version: file.version,
                bytes,
                excerpt: file
                    .source
                    .get(bytes[0]..bytes[1])
                    .unwrap_or_default()
                    .to_owned(),
            }
        }))
    }) else {
        // The reader deselected, unless the selection ended because they
        // pressed one of its actions. A click on an approved passage selects
        // nothing either, and what it offers stays.
        if !offers_approval(page) && !selection::kept(compose.pressed.get_value(), dom::now()) {
            compose.pending.set(None);
        }
        return;
    };
    let pending = match taken {
        Ok(target) => Pending::Captured(target),
        Err(refusal) => Pending::Refused(refusal),
    };

    // On a narrow screen the comments' sheet would lie over the actions, and
    // over the lower lines of what was selected.
    if !dom::wide() {
        page.panel.set(false);
    }
    compose.anchor.set(anchor(released));
    compose.pending.set(Some(pending));
}

/// What the actions for the selection just read are placed beside: the lines
/// selected, at the place along them where a mouse let go a moment ago, as
/// `released` says, or else the selection alone.
fn anchor(released: Option<(f64, f64)>) -> Option<Anchor> {
    let (selection, backward) = capture::shape()?;

    Some(
        match released.filter(|(_, when)| dom::now() - when < RELEASE_WINDOW) {
            Some((x, _)) => Anchor::Pointer {
                x,
                backward,
                selection,
            },
            None => Anchor::Selection(selection),
        },
    )
}

/// Drop the pending selection whenever the file's text is drawn again, which
/// takes the browser's selection with it.
pub(super) fn drop_selection(page: Page) {
    Effect::new(move |_| {
        page.file.track();
        page.compose().pending.set(None);
    });
}
