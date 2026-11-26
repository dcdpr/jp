//! The review page's comments: a card for each comment on the file shown, the
//! passes sent so far, and where a comment on no passage is started and
//! written.
//!
//! On a wide screen they are a column beside the file, which stays in view
//! while the file scrolls.
//! On a narrow one they are a sheet over the bottom of the screen, which the
//! top bar's button and a tap on a comment's passage show.

use jp_review_api::{Comment, Pass};
use reactive_graph::{computed::Memo, prelude::*, signal::RwSignal};
use tachys::{
    html::{
        element::{aside, button, details, div, h2, li, p, section, span, summary, ul},
        event as ev,
    },
    prelude::*,
};

use super::{Page, progress, write};
use crate::{
    card::card,
    compose,
    dom::link,
    layout::{self, Site},
    route::{ReviewView, Route, Showing},
    text,
    widget::{self, Tone},
};

/// The classes of the comments: a sheet on a narrow screen, and a column of the
/// page's grid on a wide one, as tall as the window under the top bar.
const PANEL: &str = "overflow-y-auto border-line max-lg:fixed max-lg:inset-x-0 max-lg:bottom-0 \
                     max-lg:z-30 max-lg:max-h-[55dvh] max-lg:rounded-t-xl max-lg:border-t \
                     max-lg:bg-canvas max-lg:pb-[env(safe-area-inset-bottom)] \
                     max-lg:shadow-[0_-8px_24px_rgb(0_0_0/0.18)] lg:sticky lg:top-12 \
                     lg:col-start-2 lg:row-start-1 lg:h-[calc(100dvh-3rem)] lg:self-start \
                     lg:border-l lg:pl-4 toc:col-start-3";

/// The classes of what the buttons of the comments' heading open, right under
/// the heading, where it stays in view as the heading does.
/// It takes no room while it holds nothing.
const OPENED: &str = "sticky top-12 z-10 bg-canvas px-3 lg:px-0";

/// The comments on the file shown, with the composer under their heading while
/// `site` says it is drawn there.
pub(super) fn panel(page: Page, site: Memo<Option<Site>>) -> impl RenderHtml {
    // Whether the reader is asked what a new comment is on.
    let scoping = RwSignal::new(false);

    aside()
        .class(move || {
            // A review that hasn't been read has no comments to show.
            if page.review.with(Option::is_none) {
                "hidden".to_owned()
            } else if page.panel.get() {
                PANEL.to_owned()
            } else {
                format!("{PANEL} max-lg:hidden")
            }
        })
        .attr("aria-label", "Comments")
        .child((
            head(page, scoping),
            div().class(OPENED).child((
                move || write::scope(page, scoping),
                compose::at(page.compose(), site, Site::Comments),
            )),
            div().class("flex flex-col gap-2 px-3 pb-4 lg:px-0").child((
                move || passes(page),
                move || progress::changed(page),
                move || cards(page),
            )),
        ))
}

/// The heading of the comments, which stays at their top: whether resolved ones
/// are listed, starting a comment, and hiding the sheet.
fn head(page: Page, scoping: RwSignal<bool>) -> impl RenderHtml {
    div()
        .class("sticky top-0 z-10 flex h-12 items-center gap-1 bg-canvas px-3 lg:px-0")
        .child((
            h2().class("mr-1 text-sm font-semibold").child("Comments"),
            move || resolved(page),
            span().class("flex-1"),
            move || write::add_comment(page, scoping),
            button()
                .attr("type", "button")
                .class(format!(
                    "{} lg:hidden",
                    widget::small_button_class(Tone::Quiet)
                ))
                .on(ev::click, move |_| page.panel.set(false))
                .child("Close"),
        ))
}

/// The switch between listing resolved comments and not; nothing while the page
/// has none.
fn resolved(page: Page) -> Option<impl RenderHtml> {
    let view = page.view.get();
    let file = page.shown.get()?;
    let count = page.review.with(|review| {
        review
            .as_ref()
            .map_or(0, |review| layout::resolved(&review.comments, &file))
    });
    let label = text::resolved(count, view.resolved);
    let to = Route::Review(ReviewView {
        resolved: !view.resolved,
        ..view
    });

    (count > 0).then(|| link(page.nav, to, widget::small_button_class(Tone::Quiet), label))
}

/// The passes sent so far: how the last one ended, with the assistant's closing
/// message, and the ones before it folded away.
fn passes(page: Page) -> Option<impl RenderHtml> {
    let view = page.view.get();
    let passes: Vec<Pass> = page.review.with(|review| {
        review
            .as_ref()
            .map_or_default(|review| review.passes.clone())
    });
    let (last, earlier) = passes.split_last()?;
    let ended = last.ended.as_ref();

    let earlier = (!earlier.is_empty()).then(|| {
        details().class("text-sm").child((
            summary()
                .class("cursor-pointer py-1 text-muted select-none")
                .child(format!("Earlier passes ({})", earlier.len())),
            ul().class("flex flex-col gap-1 py-1").child(
                earlier
                    .iter()
                    .rev()
                    .map(|pass| {
                        li().class("flex items-center justify-between gap-2")
                            .child((span().child(text::pass(pass)), changes(page, &view, pass)))
                    })
                    .collect::<Vec<_>>(),
            ),
        ))
    });

    Some((
        section()
            .class("rounded-lg border border-line p-3 text-sm")
            .child((
                div()
                    .class("flex items-center justify-between gap-2")
                    .child((
                        span().class("font-semibold").child(text::pass(last)),
                        changes(page, &view, last),
                    )),
                ended
                    .and_then(|ended| ended.error.clone())
                    .map(|error| p().class("mt-1.5 text-danger").child(error)),
                ended.and_then(|ended| ended.answer.clone()).map(|answer| {
                    p().class("mt-1.5 max-h-48 overflow-y-auto whitespace-pre-wrap text-muted")
                        .child(answer)
                }),
            )),
        earlier,
    ))
}

/// A link to what `pass` changed in the file shown; nothing for a pass that
/// runs, or ended without the files recorded.
fn changes(page: Page, view: &ReviewView, pass: &Pass) -> Option<impl RenderHtml + use<>> {
    pass.ended.as_ref()?.version?;

    Some(link(
        page.nav,
        Route::Review(view.show(Showing::Pass(pass.number))),
        widget::LINK,
        "What changed",
    ))
}

/// A card for each comment on the file shown, in the order [`layout::order`]
/// lists them.
fn cards(page: Page) -> AnyView {
    let view = page.view.get();
    let Some(file) = page.shown.get() else {
        return ().into_any();
    };
    let Some(comments) = page
        .review
        .with(|review| review.as_ref().map(|review| review.comments.clone()))
    else {
        return ().into_any();
    };

    let order = layout::order(&comments, &file, view.resolved);
    if order.is_empty() {
        return p()
            .class("py-2 text-sm text-muted")
            .child(nothing(page, &comments, &file))
            .into_any();
    }

    let act = write::actor(page);
    order
        .into_iter()
        .map(|index| {
            card(
                page.nav,
                comments[index].clone(),
                &view,
                act.clone(),
                page.picked,
                move |number| write::pick(page, number),
            )
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// What the comments say while none of `comments` is listed on the page of
/// `file`.
fn nothing(page: Page, comments: &[Comment], file: &str) -> &'static str {
    if layout::resolved(comments, file) > 0 {
        return "Every comment here is resolved.";
    }
    if write::writable(page) {
        return "No comments yet. Select text in the file to comment on it.";
    }

    "No comments."
}
