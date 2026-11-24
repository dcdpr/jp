//! What the page shows of how far the reviewer got, beside the highlights over
//! the passages they approved: the line across the file's text that says how
//! far it was read, and the approvals of passages that changed since.

use jp_review_api::Approval;
use leptos_dom::helpers::request_animation_frame;
use reactive_graph::{computed::Memo, effect::Effect, prelude::*};
use tachys::{
    html::element::{Div, div, li, p, section, span, ul},
    prelude::*,
    reactive_graph::node_ref::NodeRef,
};

use super::{Page, seat_under, write};
use crate::{
    compose::{Mark, PLAIN},
    layout,
    seat::Order,
    text,
    widget::{self, Tone},
};

/// The classes of the line across the file's text, which keeps its own type
/// whatever block it sits in.
const LINE: &str = "my-4 flex cursor-auto items-center gap-2 font-sans text-xs leading-normal \
                    font-medium tracking-normal whitespace-normal text-accent";

/// The classes of either half of the line, beside what it says.
const STROKE: &str = "h-px flex-1 bg-accent/40";

/// The bytes of the last line read of the file shown; `None` when the reviewer
/// didn't say how far they read it, and while the page shows what changed in
/// the file, or another version than the one the review's marks are placed in.
fn read_to(page: Page) -> Option<[usize; 2]> {
    if page.view.with(|view| view.changes.is_some()) {
        return None;
    }
    let file = page.shown.get()?;
    let shown = page
        .file
        .with(|file| Some(file.as_ref()?.as_ref().ok()?.version))?;

    page.review.with(|review| {
        let review = review.as_ref()?;
        if review.version != shown {
            return None;
        }

        layout::read_line(&review.read_until, &file)
    })
}

/// The line that says how far the file shown was read, in an element kept under
/// the last block read.
pub(super) fn read_line(page: Page) -> impl RenderHtml {
    let host = NodeRef::<Div>::new();
    let read = Memo::new(move |_| read_to(page).is_some());
    seat(page, host);

    div().node_ref(host).class(PLAIN).child(move || {
        read.get().then(|| {
            let remove = write::writable(page).then(|| {
                widget::small_action(Tone::Quiet, "Remove", move || write::forget_read(page))
            });

            div()
                .class(LINE)
                .attr("role", "separator")
                .attr("aria-label", "Read until here")
                .child((
                    span().class(STROKE),
                    span().child("Read until here"),
                    remove,
                    span().class(STROKE),
                ))
        })
    })
}

/// Keep the element `host` holds under the last block read of the file shown,
/// and out of the file's text while there is none.
fn seat(page: Page, host: NodeRef<Div>) {
    let bytes = Memo::new(move |_| read_to(page));

    Effect::new(move |_| {
        let bytes = bytes.get();
        let (pane, stage) = (page.pane.get(), page.stage.get());
        // Either changing draws the file's text again, without the element.
        page.file.track();
        page.view.track();

        // Once the browser has drawn the file, which the effect may run
        // before.
        drop(request_animation_frame(move || {
            if let (Some(host), Some(stage)) = (host.get_untracked(), stage) {
                // After a comment on the block, which was read too.
                let under = bytes.map(|bytes| (bytes, Order::Last));
                seat_under(page, &host, pane, &stage, under);
            }
        }));
    });
}

/// The approvals of passages of the file shown that changed since they were
/// made, each with the start of what it approved and taking it back; nothing
/// when there are none.
pub(super) fn changed(page: Page) -> Option<impl RenderHtml> {
    let file = page.shown.get()?;
    let changed: Vec<Approval> = page.review.with(|review| {
        let review = review.as_ref()?;
        let changed = layout::changed_since(&review.approvals, &file, review.version);

        Some(changed.into_iter().cloned().collect())
    })?;
    if changed.is_empty() {
        return None;
    }
    let writable = write::writable(page);
    let heading = text::changed_since(changed.len());

    let entries = changed
        .into_iter()
        .map(|approval| {
            let number = approval.number;
            let dismiss = writable.then(|| {
                widget::small_action(Tone::Plain, "Dismiss", move || {
                    write::mark(page, Mark::Unapprove(number));
                })
            });

            li().class("flex items-start justify-between gap-2").child((
                span()
                    .class("line-clamp-2 text-xs text-muted")
                    .child(approval.excerpt.unwrap_or_default()),
                dismiss,
            ))
        })
        .collect::<Vec<_>>();

    Some(
        section()
            .class("rounded-lg border border-l-4 border-line border-l-draft p-3 text-sm")
            .child((
                p().class("font-semibold").child(heading),
                ul().class("mt-1.5 flex flex-col gap-1.5").child(entries),
            )),
    )
}
