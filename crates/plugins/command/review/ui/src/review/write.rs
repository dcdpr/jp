//! What the review page asks the server to change: the reviewer's actions on
//! comments, the header's comment and pass controls, recording the working
//! tree, and the selection the comment bar offers.

use std::{cell::Cell, rc::Rc};

use jp_review_api::{Comment, Recorded, State, StopRequest};
use leptos_dom::helpers::{TimeoutHandle, set_timeout};
use reactive_graph::{effect::Effect, prelude::*, spawn_local};
use serde_json::Value;
use tachys::{
    html::{
        element::{Div, button, div, p},
        event as ev,
    },
    prelude::*,
    reactive_graph::node_ref::NodeRef,
};

use super::Page;
use crate::{
    actions::{self, Action},
    capture,
    client::{self, Call},
    compose::{Pending, SETTLE},
    composer::{Draft, Target},
    dom::{self, document_listener},
    route::{ReviewView, Route},
    selection,
};

/// Whether the review takes changes.
fn writable(page: Page) -> bool {
    page.review.with(|review| {
        review
            .as_ref()
            .is_some_and(|review| review.head.status == State::Open)
    })
}

/// A button labelled `label` that runs `then`.
fn control(
    label: String,
    class: &'static str,
    then: impl Fn() + Send + Sync + 'static,
) -> impl RenderHtml {
    button()
        .attr("type", "button")
        .class(class)
        .on(ev::click, move |_| then())
        .child(label)
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
        Action::Edit => {
            if let Some(draft) = Draft::edit(comment) {
                page.compose().open(draft);
            }
        }
        Action::Reply | Action::EditReply => {
            if let Some(draft) = Draft::reply(comment) {
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
                write(page, Call::delete(client::comment_url(&id, number)));
            }
        }
        Action::Resolve => write(page, Call::post_empty(client::resolve_url(&id, number))),
    }
}

/// Make `call`, polling at once when the server took it, and saying why when it
/// didn't.
fn write(page: Page, call: Call) {
    spawn_local(async move {
        match client::send::<Value>(call).await {
            Ok(_) => {
                page.notice.set(None);
                page.compose().refreshed();
            }
            Err(error) => page.notice.set(Some(error)),
        }
    });
}

/// The header's controls: a comment on the review, and sending the pending
/// comments to the assistant, or stopping the pass this server runs.
pub(super) fn controls(page: Page) -> impl RenderHtml {
    move || {
        if !writable(page) {
            return None;
        }
        let running = page
            .status
            .with(|status| status.as_ref().and_then(|status| status.running.clone()));
        let pending = page.review.with(|review| {
            review
                .as_ref()
                .map_or(0, |review| actions::pending(&review.comments))
        });

        let pass = match running {
            None => control(
                format!("Send ({pending})"),
                "jp-button jp-button-primary",
                move || {
                    page.compose().open(Draft::pass());
                },
            )
            .into_any(),
            Some(running) if running.stoppable => {
                control("Stop".to_owned(), "jp-button", move || {
                    stop(page, running.number);
                })
                .into_any()
            }
            Some(_) => ().into_any(),
        };

        Some(div().class("jp-controls").child((
            control("Comment".to_owned(), "jp-button", move || {
                page.compose().open(Draft::comment(Target::Review));
            }),
            pass,
        )))
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

/// The file tab bar's button for a comment on the whole file shown, in the file
/// view of a review that takes changes.
pub(super) fn file_comment(page: Page) -> Option<impl RenderHtml> {
    if !writable(page) || page.view.with(|view| view.changes.is_some()) {
        return None;
    }
    let path = page.shown.get()?;
    let version = page.review.with(|review| Some(review.as_ref()?.version))?;

    Some(control(
        "Comment on file".to_owned(),
        "jp-tab jp-tab-action",
        move || {
            page.compose().open(Draft::comment(Target::File {
                path: path.clone(),
                version,
            }));
        },
    ))
}

/// The working-tree banner's button, which records the files on disk as a new
/// version and shows it.
pub(super) fn record_button(page: Page) -> Option<impl RenderHtml> {
    writable(page).then(|| {
        control("Record a new version".to_owned(), "jp-button", move || {
            record(page);
        })
    })
}

fn record(page: Page) {
    let call = Call::post_empty(client::versions_url(&page.compose().id.get_value()));
    spawn_local(async move {
        match client::send::<Recorded>(call).await {
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

/// Why the last change asked of the server failed, until dismissed.
pub(super) fn notice(page: Page) -> Option<impl RenderHtml> {
    let notice = page.notice.get()?;

    Some(
        div()
            .class("jp-banner jp-banner-warn")
            .attr("role", "alert")
            .child((
                p().child(notice),
                control("Dismiss".to_owned(), "jp-button", move || {
                    page.notice.set(None);
                }),
            )),
    )
}

/// Read the selection in the file pane once it settles, for the comment bar.
pub(super) fn follow_selection(page: Page, pane: NodeRef<Div>) {
    let waiting = Rc::new(Cell::new(None::<TimeoutHandle>));
    document_listener("selectionchange", move |_| {
        if let Some(handle) = waiting.take() {
            handle.clear();
        }
        waiting.set(set_timeout(move || take_selection(page, pane), SETTLE).ok());
    });
}

/// Take the selection in the file pane as what the comment bar offers, in the
/// file view while the composer is closed.
fn take_selection(page: Page, pane: NodeRef<Div>) {
    // The page may be gone by the time the selection settles.
    let Some(compose) = page.compose.try_get_value() else {
        return;
    };
    if compose.draft.with_untracked(Option::is_some)
        || page.view.with_untracked(|view| view.changes.is_some())
    {
        return;
    }
    let Some(element) = pane.get_untracked() else {
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
        // pressed the comment bar.
        if !selection::kept(compose.pressed.get_value(), dom::now()) {
            compose.pending.set(None);
        }
        return;
    };

    compose.pending.set(Some(match taken {
        Ok(target) => Pending::Captured(target),
        Err(refusal) => Pending::Refused(refusal),
    }));
}

/// Drop the pending selection whenever the file pane renders again, which takes
/// the browser's selection with it.
pub(super) fn drop_selection(page: Page) {
    Effect::new(move |_| {
        page.file.track();
        page.review.track();
        page.compose().pending.set(None);
    });
}
