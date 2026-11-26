//! What the page shows of how far the reviewer got, beside the highlights over
//! the passages they approved: the line across the file's text that says how
//! far it was read, and the approvals of passages that changed since.

use jp_review_api::Approval;
use leptos_dom::helpers::request_animation_frame;
use reactive_graph::{computed::Memo, effect::Effect, prelude::*, signal::RwSignal};
use tachys::{
    html::element::{Div, div, li, p, section, span, ul},
    prelude::*,
    reactive_graph::node_ref::NodeRef,
};

use super::{Page, seat_under, write};
use crate::{
    compose::{Mark, PLAIN},
    layout::{self, ReadLine},
    seat::{self, Order},
    text,
    widget::{self, Tone},
};

/// The classes of the line across the file's text, which keeps its own type
/// whatever block it sits in.
const LINE: &str = "my-4 flex cursor-auto items-center gap-2 font-sans text-xs leading-normal \
                    font-medium tracking-normal whitespace-normal text-accent";

/// The classes of either half of the line, beside what it says.
const STROKE: &str = "h-px flex-1 bg-accent/40";

/// Return the projected read-until line for the displayed file, or `None` if no
/// marker is set, if the changes view is active, or if the displayed version
/// differs from the version the review's projections target.
fn read_to(page: Page) -> Option<ReadLine> {
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
///
/// The line shows only while the element sits in the file's text: a file whose
/// text before the line read renders as nothing has no block to put it under,
/// and the end of the file would say the whole of it was read.
/// It names the line read, and says so when the block it sits under runs past
/// that line, as a code block or a long paragraph can.
pub(super) fn read_line(page: Page) -> impl RenderHtml {
    let host = NodeRef::<Div>::new();
    let line = Memo::new(move |_| read_to(page).map(|read| read.line));
    // `Some(true)` when seated under a block that extends past the read-until
    // line, `Some(false)` when seated at the end of the block, or `None` when
    // not seated in the document.
    let seated = RwSignal::new(None::<bool>);
    seat(page, host, seated);

    div().node_ref(host).class(PLAIN).child(move || {
        let (line, within) = (line.get()?, seated.get()?);
        let label = text::read_until(&line, within);
        let remove = write::writable(page)
            .then(|| widget::small_action(Tone::Quiet, "Remove", move || write::forget_read(page)));

        Some(
            div()
                .class(LINE)
                .attr("role", "separator")
                .attr("aria-label", label.clone())
                .child((
                    span().class(STROKE),
                    span().child(label),
                    remove,
                    span().class(STROKE),
                )),
        )
    })
}

/// Position `host` below the last rendered block up to the read-until line, or
/// park it outside the document when no marker applies, updating `seated` with
/// whether the block extends past the read-until line.
///
/// Because the read-until line itself may produce no rendered DOM nodes (for
/// example, a blank line), the divider is placed after the last rendered span
/// whose byte range ends at or before the end of that line.
fn seat(page: Page, host: NodeRef<Div>, seated: RwSignal<Option<bool>>) {
    let bytes = Memo::new(move |_| read_to(page).map(|read| read.bytes));

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
                let under = bytes.map(|[_, end]| ([0, end], Order::Last));
                let taken = seat_under(page, &host, pane, &stage, under);
                let within = bytes
                    .zip(taken)
                    .map(|([_, end], taken)| seat::end(&taken) > Some(end));
                seated.set(within);
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
