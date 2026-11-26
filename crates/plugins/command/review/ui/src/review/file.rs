//! The review page's file column: the file as a version holds it, where text is
//! selected to comment on it, or what changed in the file between two versions.
//!
//! The file's text is the same elements whichever comments there are, so a
//! comment arriving leaves the reader's place and selection as they are.

use jp_review_api::{Change, FileReply, Version};
use reactive_graph::prelude::*;
use tachys::{
    html::{
        element::{div, p, pre, span},
        event as ev,
    },
    prelude::*,
};
use web_sys::MouseEvent;

use super::{Page, entry, write};
use crate::{
    blocks::{self, Block},
    card::{DIFF, line_view},
    dom::link,
    route::{Changes, ReviewView, Route, Span},
    text,
    widget::{self, Choice, Fit, Tone},
};

/// The classes of the element holding the file's text.
/// A tap on it picks the comment under the tap, which iOS would flash the whole
/// file for.
const PANE: &str = "jp-pane [-webkit-tap-highlight-color:transparent]";

/// The classes of a notice over the file.
const NOTICE: &str = "mb-4 flex flex-wrap items-center justify-between gap-x-4 gap-y-2 rounded-lg \
                      border border-line bg-surface px-3 py-2 text-sm";

/// The classes of a notice of something that went wrong.
const WARNING: &str = "mb-4 flex flex-wrap items-center justify-between gap-x-4 gap-y-2 \
                       rounded-lg bg-warn px-3 py-2 text-sm";

/// What the reader is told over the file: why the last poll failed, why the
/// last change didn't happen, and that the files on disk changed.
pub(super) fn notices(page: Page) -> impl RenderHtml {
    (
        move || {
            page.error
                .get()
                .map(|error| p().class(WARNING).attr("role", "alert").child(error))
        },
        move || {
            let notice = page.notice.get()?;

            Some(div().class(WARNING).attr("role", "alert").child((
                p().child(notice),
                widget::action(Tone::Plain, "Dismiss", move || page.notice.set(None)),
            )))
        },
        move || working_tree(page),
    )
}

/// That the files on disk changed since the latest version, while no pass runs,
/// whose end records them.
fn working_tree(page: Page) -> Option<impl RenderHtml> {
    let changed = page.status.with(|status| {
        status
            .as_ref()
            .is_some_and(|status| status.working_tree_changed && status.running.is_none())
    });
    let latest = page
        .review
        .with(|review| Some(review.as_ref()?.versions.last()?.number))?;

    changed.then(|| {
        div().class(NOTICE).child((
            p().child(format!(
                "The files on disk changed since version {latest}, the latest recorded."
            )),
            write::record_button(page),
        ))
    })
}

/// The file with its comments' passages, or what changed in it.
pub(super) fn body(page: Page) -> impl RenderHtml {
    let view = page.view.get();
    let Some(file) = page.shown.get() else {
        return unread(page).into_any();
    };
    if let Some(changes) = view.changes {
        return changed(page, &view, &file, changes).into_any();
    }

    match page.file.get() {
        None => loading().into_any(),
        Some(Err(error)) => p().class(widget::ERROR).child(error).into_any(),
        Some(Ok(reply)) => commented(page, &reply).into_any(),
    }
}

fn loading() -> impl RenderHtml {
    p().class(widget::NOTE).child("Loading\u{2026}")
}

/// What shows in place of the file while the review hasn't been read: that it
/// is loading, or the way back to the reviews when the server has none at this
/// address, which the notice over this says.
fn unread(page: Page) -> AnyView {
    if page.error.with(Option::is_none) {
        return loading().into_any();
    }

    link(page.nav, Route::List, widget::LINK, "All reviews").into_any()
}

/// The file, for its comments' passages to be highlighted over, and its text to
/// be selected and tapped.
fn commented(page: Page, reply: &FileReply) -> impl RenderHtml + use<> {
    let blocks = blocks::blocks(reply.html.as_deref().unwrap_or_default())
        .into_iter()
        .map(text_of)
        .collect::<Vec<_>>();

    div()
        .class(PANE)
        .node_ref(page.pane)
        .on(ev::click, move |event: MouseEvent| write::tap(page, &event))
        .child(blocks)
}

/// What changed in the file: the later version rendered with its changed lines
/// highlighted, or the unified diff.
fn changed(page: Page, view: &ReviewView, file: &str, changes: Changes) -> AnyView {
    let strip = strip(page, view, changes);
    let diff = match page.diff.get() {
        None => return (strip, loading()).into_any(),
        Some(Err(error)) => return (strip, p().class(widget::ERROR).child(error)).into_any(),
        Some(Ok(diff)) => diff,
    };
    let entry = entry(&diff, file).cloned();
    let unified_view = ReviewView {
        changes: Some(Changes {
            unified: true,
            ..changes
        }),
        ..view.clone()
    };

    let deleted = entry
        .as_ref()
        .is_some_and(|entry| entry.change == Change::Deleted);
    if let Some(entry) = entry.as_ref().filter(|_| changes.unified || deleted) {
        return (
            strip,
            deleted.then(|| {
                p().class(widget::NOTE)
                    .child(format!("{file} is deleted in version {}.", diff.to))
            }),
            unified(&entry.diff),
        )
            .into_any();
    }

    let unchanged = entry.is_none().then(|| {
        p().class(widget::NOTE).child(format!(
            "{file} didn't change from version {} to version {}.",
            diff.from, diff.to
        ))
    });

    (strip, unchanged, hidden(page, unified_view), rendered(page)).into_any()
}

/// What the changes view compares, how it shows it, and the way back to the
/// file, over the changes.
fn strip(page: Page, view: &ReviewView, changes: Changes) -> impl RenderHtml + use<> {
    let nav = page.nav;
    let unified = changes.unified;

    div()
        .class("mb-4 flex flex-wrap items-center gap-2 text-sm text-muted")
        .child((
            link(
                nav,
                Route::Review(ReviewView {
                    changes: None,
                    ..view.clone()
                }),
                widget::button_class(Tone::Plain),
                "\u{2190} The file",
            ),
            compared(page, view, changes.span),
            span().class("flex-1"),
            link(
                nav,
                Route::Review(ReviewView {
                    changes: Some(Changes {
                        unified: !unified,
                        ..changes
                    }),
                    ..view.clone()
                }),
                widget::button_class(Tone::Plain),
                if unified {
                    "Show rendered"
                } else {
                    "Show the diff"
                },
            ),
        ))
}

/// The two versions the changes view compares: those a pass was sent against
/// and ended with, or the two picked here.
fn compared(page: Page, view: &ReviewView, between: Span) -> AnyView {
    let compared = page.diff.with(|diff| {
        let diff = diff.as_ref()?.as_ref().ok()?;
        Some((diff.from, diff.to))
    });
    let versions: Vec<Version> = page.review.with(|review| {
        review
            .as_ref()
            .map_or_default(|review| review.versions.clone())
    });

    match (between, compared) {
        (Span::Pass(pass), Some((from, to))) => span()
            .child(format!("Pass {pass} changed version {from} into {to}"))
            .into_any(),
        (Span::Pass(pass), None) => span().child(format!("Pass {pass}")).into_any(),
        (Span::Versions { .. }, Some((from, to))) => (
            version_picker(page, &versions, from, "From version", {
                let view = view.clone();
                move |from| compare(&view, from, to)
            }),
            span().child("to"),
            version_picker(page, &versions, to, "To version", {
                let view = view.clone();
                move |to| compare(&view, from, to)
            }),
        )
            .into_any(),
        (Span::Versions { .. }, None) => ().into_any(),
    }
}

/// `view` comparing versions `from` and `to`.
fn compare(view: &ReviewView, from: u32, to: u32) -> ReviewView {
    ReviewView {
        changes: Some(Changes {
            span: Span::Versions {
                from: Some(from),
                to: Some(to),
            },
            unified: view.changes.is_some_and(|changes| changes.unified),
        }),
        ..view.clone()
    }
}

/// A picker of one of `versions`, `chosen` chosen, that shows the page `to`
/// names for the version picked.
fn version_picker<F>(
    page: Page,
    versions: &[Version],
    chosen: u32,
    name: &'static str,
    to: F,
) -> impl RenderHtml + use<F>
where
    F: Fn(u32) -> ReviewView + 'static,
{
    let latest = versions.last().map(|version| version.number);
    let choices = versions
        .iter()
        .rev()
        .map(|version| Choice {
            value: version.number.to_string(),
            label: text::version(version, Some(version.number) == latest),
            group: "",
        })
        .collect();
    let nav = page.nav;

    widget::picker(
        name,
        Fit::Rigid,
        span().class("text-ink").child(format!("Version {chosen}")),
        chosen.to_string(),
        choices,
        move |value| {
            if let Ok(version) = value.parse() {
                nav.go(Route::Review(to(version)));
            }
        },
    )
}

/// How many changed lines render to nothing, with a link to `unified`, the
/// unified diff that shows them; nothing while none do.
fn hidden(page: Page, unified: ReviewView) -> impl RenderHtml {
    move || {
        let count = page.hidden.get();
        (count > 0).then(|| {
            p().class(format!("{} mb-4", widget::NOTE)).child((
                text::hidden(count),
                "; ",
                link(
                    page.nav,
                    Route::Review(unified.clone()),
                    widget::LINK,
                    "see the diff",
                ),
                ".",
            ))
        })
    }
}

/// The file as the later version compared holds it, for the changed lines to be
/// highlighted over.
fn rendered(page: Page) -> AnyView {
    match page.file.get() {
        None => loading().into_any(),
        Some(Err(error)) => p().class(widget::ERROR).child(error).into_any(),
        Some(Ok(reply)) => {
            let blocks = blocks::blocks(reply.html.as_deref().unwrap_or_default())
                .into_iter()
                .map(text_of)
                .collect::<Vec<_>>();

            div()
                .class(PANE)
                .node_ref(page.pane)
                .child(blocks)
                .into_any()
        }
    }
}

/// A block of a rendered file; a line of a plain-text file keeps its spacing.
fn text_of(block: Block) -> impl RenderHtml {
    let class = if block.line {
        "jp-text jp-line"
    } else {
        "jp-text"
    };

    div().class(class).inner_html(block.html)
}

/// The unified diff hunks `diff`, each line styled as what it is, wrapping long
/// lines.
fn unified(diff: &str) -> impl RenderHtml + use<> {
    pre().class(DIFF).child(
        text::diff_lines(diff)
            .into_iter()
            .map(|(kind, line)| line_view(kind, line.to_owned()))
            .collect::<Vec<_>>(),
    )
}
