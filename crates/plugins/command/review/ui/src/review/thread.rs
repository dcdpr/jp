//! What sits in the file's text, right under a passage of it: the thread of the
//! comment picked, and the composer while it writes about that passage.
//!
//! [`layout::inline`] decides what sits there, [`host`] draws it in one element
//! outside the file's text, and [`seat`] moves that element under the passage.
//! The element is moved rather than drawn in place, since the file's text is
//! HTML the server rendered, which each block holds as it came.

use leptos_dom::helpers::request_animation_frame;
use reactive_graph::{computed::Memo, effect::Effect, prelude::*};
use tachys::{html::element::div, prelude::*};

use super::{Page, seat_under, write};
use crate::{
    actions,
    card::thread,
    compose::{self, PLAIN},
    dom,
    layout::{self, Inline, Site},
    seat::Order,
};

/// What sits in the file's text, on a screen wide enough for the text to stay
/// readable around it.
///
/// A narrow screen shows a thread with the other comments, and keeps the
/// composer in a sheet above the on-screen keyboard.
pub(super) fn inline(page: Page) -> Memo<Option<Inline>> {
    let kind = compose::kind(page.compose());

    Memo::new(move |_| {
        if !dom::wide() || page.view.with(|view| view.changes.is_some()) {
            return None;
        }
        let file = page.shown.get()?;
        let shown = page
            .file
            .with(|file| Some(file.as_ref()?.as_ref().ok()?.version))?;
        let picked = page.picked.get();

        kind.with(|kind| {
            page.review.with(|review| {
                let review = review.as_ref()?;
                layout::inline(kind.as_ref(), picked, &review.comments, &file, [
                    shown,
                    review.version,
                ])
            })
        })
    })
}

/// The element that sits in the file's text, holding what `inline` says: the
/// thread, and the composer under it or in its place, while `site` says the
/// composer is drawn in the file.
pub(super) fn host(
    page: Page,
    inline: Memo<Option<Inline>>,
    site: Memo<Option<Site>>,
) -> impl RenderHtml {
    let compose = page.compose();
    // The comment whose thread shows, which changes less often than the
    // comment itself does.
    let shown = Memo::new(move |_| match inline.get() {
        Some(Inline::Thread { number, .. }) => Some(number),
        Some(Inline::Composer(_)) | None => None,
    });

    div().node_ref(compose.host).class(PLAIN).child((
        move || shown.get().map(|number| card(page, number)),
        compose::at(compose, site, Site::File),
    ))
}

/// The thread of the comment numbered `number`, as the review holds it now.
fn card(page: Page, number: u32) -> impl RenderHtml {
    move || {
        let view = page.view.get();
        let comment = page.review.with(|review| {
            review
                .as_ref()?
                .comments
                .iter()
                .find(|comment| actions::number(comment) == Some(number))
                .cloned()
        })?;

        Some(thread(
            page.nav,
            comment,
            &view,
            write::actor(page),
            move || {
                page.picked.set(None);
            },
        ))
    }
}

/// Keep the element [`host`] draws under the passage `inline` names, and out of
/// the file's text while it names none; and hand the composer the keyboard each
/// time it is opened.
pub(super) fn seat(page: Page, inline: Memo<Option<Inline>>) {
    let compose = page.compose();

    Effect::new(move |before: Option<u32>| {
        let opened = compose.opened.get();
        let focus = before.is_some_and(|before| before != opened);
        let bytes = inline.get().map(Inline::bytes);
        let (pane, stage) = (page.pane.get(), page.stage.get());
        // Either changing draws the file's text again, without the element.
        page.file.track();
        page.view.track();

        // Once the browser has drawn the file and the composer, which the
        // effect may run before.
        drop(request_animation_frame(move || {
            let (Some(host), Some(stage)) = (compose.host.get_untracked(), stage) else {
                return;
            };
            // Right under the text it is about, over the line that says how
            // far the file was read when that sits there too.
            let under = bytes.map(|bytes| (bytes, Order::First));
            seat_under(page, &host, pane, &stage, under);
            if focus {
                compose.focus();
            }
        }));

        opened
    });
}
