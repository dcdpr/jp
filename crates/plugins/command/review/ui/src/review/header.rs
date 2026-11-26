//! The review page's top bar: the review's title, the file and the version or
//! changes shown, the pass running, and sending to the assistant.
//!
//! Everything here is one row, on a phone too, where the title gives way and
//! the labels shorten.

use jp_review_api::Detail;
use reactive_graph::{computed::Memo, prelude::*};
use tachys::{
    html::{
        element::{button, div, h1, header, span},
        event as ev,
    },
    prelude::*,
};

use super::{Page, write};
use crate::{
    compose,
    dom::link,
    layout::{self, Site},
    route::{Route, Showing},
    text,
    widget::{self, Choice, Fit},
};

/// A speech bubble, for the button that shows the comments on a narrow screen.
const BUBBLE: &str = r#"<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round" aria-hidden="true"><path d="M4 5h16v11H10l-5 4v-4H4z"/></svg>"#;

/// The top bar, with the composer under its last button while `site` says a
/// pass is written there.
pub(super) fn top(page: Page, site: Memo<Option<Site>>) -> impl RenderHtml {
    header().class(widget::BAR).child((
        link(page.nav, Route::List, widget::HOME, "jp"),
        h1().class(widget::TITLE).child(move || {
            page.review
                .with(|review| review.as_ref().map(|review| review.head.title.clone()))
                .unwrap_or_default()
        }),
        move || closed(page),
        move || files(page),
        move || showing(page),
        move || running(page),
        move || comments(page),
        // What the composer under the button is placed in.
        div().class("relative shrink-0").child((
            write::controls(page),
            compose::at(page.compose(), site, Site::Send),
        )),
    ))
}

/// That the review is closed, and takes no changes.
fn closed(page: Page) -> Option<impl RenderHtml> {
    write::closed(page).then(|| {
        span()
            .class("shrink-0 rounded bg-raised px-1.5 py-px text-xs font-medium text-muted")
            .child("Closed")
    })
}

/// The picker of the file shown, each file with how many of its comments are
/// not resolved; nothing for a review of one file.
fn files(page: Page) -> Option<impl RenderHtml> {
    let shown = page.shown.get()?;

    page.review.with(|review| {
        let review = review.as_ref()?;
        if review.head.files.len() < 2 {
            return None;
        }
        let choices = review
            .head
            .files
            .iter()
            .map(|path| Choice {
                value: path.clone(),
                label: text::file(path, layout::unresolved(&review.comments, path)),
                group: "",
            })
            .collect();
        let name = shown.rsplit('/').next().unwrap_or(&shown).to_owned();

        // The one thing in the bar that gives way on a narrow screen, where
        // the bar is otherwise wider than the screen.
        Some(widget::picker(
            "File",
            Fit::Shrinks,
            span()
                .class("max-w-28 min-w-0 truncate sm:max-w-72")
                .child(widget::wide_or_narrow(shown.clone(), name)),
            shown,
            choices,
            move |path| write::show_file(page, path, None),
        ))
    })
}

/// The picker of what the page shows of the file: it as a version holds it, or
/// what a pass or two versions changed in it.
fn showing(page: Page) -> Option<impl RenderHtml> {
    let view = page.view.get();

    page.review.with(|review| {
        let review = review.as_ref()?;
        let latest = review.versions.last()?.number;
        // The latest version is offered once, as the one the page moves on
        // from when a newer one is recorded.
        let now = match view.showing() {
            Showing::Version(version) if version == latest => Showing::Latest,
            other => other,
        };
        let (wide, narrow) = text::showing(now, latest);
        let nav = page.nav;

        Some(widget::picker(
            "What the page shows",
            Fit::Rigid,
            widget::wide_or_narrow(wide, narrow),
            now.value(),
            offered(review, latest),
            move |value| {
                if let Some(showing) = Showing::parse(&value) {
                    nav.go(Route::Review(view.show(showing)));
                }
            },
        ))
    })
}

/// What can be shown of a file of `review`, whose latest version is numbered
/// `latest`: each version, newest first, and then what each pass and any two
/// versions changed.
fn offered(review: &Detail, latest: u32) -> Vec<Choice> {
    let versions = review.versions.iter().rev().map(|version| {
        let showing = if version.number == latest {
            Showing::Latest
        } else {
            Showing::Version(version.number)
        };
        Choice {
            value: showing.value(),
            label: text::version(version, version.number == latest),
            group: "The file",
        }
    });
    // Only a pass that recorded the files has changes to show.
    let passes = review
        .passes
        .iter()
        .rev()
        .filter(|pass| {
            pass.ended
                .as_ref()
                .is_some_and(|ended| ended.version.is_some())
        })
        .map(|pass| Choice {
            value: Showing::Pass(pass.number).value(),
            label: text::pass(pass),
            group: "What changed",
        });
    let compare = (review.versions.len() > 1).then(|| Choice {
        value: Showing::Compare.value(),
        label: "Between two versions".to_owned(),
        group: "What changed",
    });

    versions.chain(passes).chain(compare).collect()
}

/// That a pass runs, and where.
fn running(page: Page) -> Option<impl RenderHtml> {
    let running = page
        .status
        .with(|status| status.as_ref()?.running.clone())?;

    Some(
        span()
            .class("flex shrink-0 items-center gap-1.5 text-sm font-medium text-accent")
            .attr("role", "status")
            .child((
                span()
                    .class("size-2 animate-pulse rounded-full bg-accent")
                    .attr("aria-hidden", "true"),
                span()
                    .class("max-sm:sr-only")
                    .child(text::running(&running)),
            )),
    )
}

/// The button that shows and hides the comments on a narrow screen, with how
/// many the file shown has; a wide screen shows them beside the file.
fn comments(page: Page) -> Option<impl RenderHtml> {
    let resolved = page.view.with(|view| view.resolved);
    let file = page.shown.get()?;
    let count = page.review.with(|review| {
        review.as_ref().map_or(0, |review| {
            layout::order(&review.comments, &file, resolved).len()
        })
    });
    let open = page.panel.get();

    Some(
        button()
            .attr("type", "button")
            .class(format!(
                "{} lg:hidden",
                widget::button_class(if open {
                    widget::Tone::Primary
                } else {
                    widget::Tone::Plain
                })
            ))
            .attr("aria-label", text::comments(count))
            .attr("aria-expanded", if open { "true" } else { "false" })
            .on(ev::click, move |_| page.panel.update(|open| *open = !*open))
            .child((span().inner_html(BUBBLE), span().child(count.to_string()))),
    )
}
