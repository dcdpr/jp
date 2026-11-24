//! `/`: every review, with its files, latest version, comments by status, and
//! the pass running on it, and a form to start a new one.

use jp_review_api::{Listing, NewReview, State, Summary};
use leptos_dom::helpers::event_target_value;
use reactive_graph::{prelude::*, signal::RwSignal, spawn_local};
use tachys::{
    html::{
        element::{
            button, div, form, h1, h2, header, input, label, li, main, p, section, span, textarea,
            ul,
        },
        event as ev,
    },
    prelude::*,
};
use web_sys::SubmitEvent;

use crate::{
    client,
    dom::{Nav, keep_polling, link},
    route::{ReviewView, Route},
    text,
};

/// The review list, which notes the workspace each answer names in `seen`.
pub(crate) fn page(nav: Nav, seen: RwSignal<Option<String>>) -> impl RenderHtml {
    let listing = RwSignal::new(None::<Result<Listing, String>>);
    follow(listing, seen);

    main().class("jp-page jp-list").child((
        header()
            .class("jp-header")
            .child(div().class("jp-bar").child((
                span().class("jp-home").child("jp"),
                h1().class("jp-title").child(move || {
                    seen.get()
                        .map_or_else(|| "Reviews".to_owned(), |name| format!("Reviews of {name}"))
                }),
            ))),
        start(nav),
        section()
            .class("jp-reviews")
            .child(move || listing.with(|listing| reviews(nav, listing.as_ref()))),
    ))
}

/// Poll the list into `listing` for as long as the page shows it, more often
/// while a pass runs on one of the reviews.
fn follow(listing: RwSignal<Option<Result<Listing, String>>>, seen: RwSignal<Option<String>>) {
    keep_polling(
        move || {
            listing.with_untracked(|listing| {
                matches!(listing, Some(Ok(listing))
                    if listing.reviews.iter().any(|review| review.running.is_some()))
            })
        },
        RwSignal::new(0),
        move || async move {
            let answer = client::get::<Listing>(&client::list_url()).await;
            if let Ok(listing) = &answer {
                seen.set(Some(listing.workspace.clone()));
            }
            listing.set(Some(answer));
        },
    );
}

/// The reviews, newest first, and those that can't be read.
fn reviews(nav: Nav, listing: Option<&Result<Listing, String>>) -> AnyView {
    let listing = match listing {
        None => return p().class("jp-note").child("Loading…").into_any(),
        Some(Err(error)) => return p().class("jp-error").child(error.clone()).into_any(),
        Some(Ok(listing)) => listing,
    };
    if listing.reviews.is_empty() && listing.unreadable.is_empty() {
        return p()
            .class("jp-note")
            .child("No reviews yet. Start one above, or with `jp review start`.")
            .into_any();
    }

    let unreadable = listing
        .unreadable
        .iter()
        .map(|unreadable| {
            p().class("jp-error").child(format!(
                "{} can't be read: {}",
                unreadable.id, unreadable.error
            ))
        })
        .collect::<Vec<_>>();

    (
        ul().class("jp-items").child(
            listing
                .reviews
                .iter()
                .rev()
                .map(|summary| item(nav, summary))
                .collect::<Vec<_>>(),
        ),
        unreadable,
    )
        .into_any()
}

/// A review in the list, newest first.
fn item(nav: Nav, summary: &Summary) -> impl RenderHtml {
    let head = &summary.head;

    li().class(format!("jp-item jp-item-{}", head.status))
        .child((
            link(
                nav,
                Route::Review(ReviewView::of(&head.id)),
                "jp-item-title",
                head.title.clone(),
            ),
            div().class("jp-item-meta").child((
                span().class("jp-item-id").child(head.id.clone()),
                span().child(format!("version {}", summary.latest)),
                span().child(text::counts(&summary.comments)),
                summary.running.map(|pass| {
                    span()
                        .class("jp-running")
                        .child(format!("Pass {pass} running"))
                }),
                (head.status == State::Closed).then(|| span().class("jp-status").child("closed")),
            )),
            div().class("jp-item-files").child(head.files.join(", ")),
        ))
}

/// The form that starts a review of workspace files, and opens it.
fn start(nav: Nav) -> impl RenderHtml {
    let files = RwSignal::new(String::new());
    let title = RwSignal::new(String::new());
    let refused = RwSignal::new(None::<String>);
    let sending = RwSignal::new(false);

    let submit = move |event: SubmitEvent| {
        event.prevent_default();
        let request = NewReview {
            files: paths(&files.get_untracked()),
            title: Some(title.get_untracked()).filter(|title| !title.trim().is_empty()),
        };
        if request.files.is_empty() {
            refused.set(Some("Name a file to review.".to_owned()));
            return;
        }

        send(nav, request, refused, sending);
    };

    section().class("jp-start").child(
        form().on(ev::submit, submit).child((
            h2().child("Start a review"),
            label().class("jp-field").child((
                span().child("Files, one per line, relative to the workspace root"),
                textarea()
                    .attr("rows", "2")
                    .attr("placeholder", "docs/notes.md")
                    .attr("autocapitalize", "off")
                    .attr("spellcheck", "false")
                    .prop("value", move || files.get())
                    .on(ev::input, move |event| {
                        files.set(event_target_value(&event));
                    }),
            )),
            label().class("jp-field").child((
                span().child("Title (its files when left empty)"),
                input()
                    .attr("type", "text")
                    .prop("value", move || title.get())
                    .on(ev::input, move |event| {
                        title.set(event_target_value(&event));
                    }),
            )),
            button()
                .attr("type", "submit")
                .class("jp-button")
                .prop("disabled", move || sending.get())
                .child("Start"),
            move || {
                refused
                    .get()
                    .map(|error| p().class("jp-error").child(error))
            },
        )),
    )
}

/// Start the review `request` asks for, and open it; why the server refused it
/// goes in `refused`, and `sending` holds while the request is out.
fn send(nav: Nav, request: NewReview, refused: RwSignal<Option<String>>, sending: RwSignal<bool>) {
    sending.set(true);
    spawn_local(async move {
        match client::post::<Summary>(&client::list_url(), &request).await {
            Ok(summary) => nav.go(Route::Review(ReviewView::of(&summary.head.id))),
            Err(error) => refused.set(Some(error)),
        }
        sending.set(false);
    });
}

/// The paths `text` names, one per line or separated by commas.
fn paths(text: &str) -> Vec<String> {
    text.split(['\n', ','])
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(test)]
#[path = "list_tests.rs"]
mod tests;
