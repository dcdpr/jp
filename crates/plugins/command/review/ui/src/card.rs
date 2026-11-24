//! A comment's card: who wrote it, where it stands, what it is on, what it
//! says, the replies to it, and what the reviewer can do with it.
//!
//! A comment shows in two places: listed beside the file ([`card`]), and, once
//! picked, as its thread in the file's text, under its passage ([`thread`]).
//!
//! Bodies are the HTML the server rendered with `md-html`, which escapes raw
//! HTML in them.

use jp_review_api::{Comment, Reply, Role, Status, Verdict};
use reactive_graph::{prelude::*, signal::RwSignal};
use tachys::{
    html::{
        element::{article, blockquote, button, div, header, p, pre, span},
        event as ev,
    },
    prelude::*,
};
use web_sys::MouseEvent;

use crate::{
    actions::{self, Action},
    compose::INLINE,
    dom::{Nav, link},
    route::{ReviewView, Route},
    text::{self, Line},
    widget::{self, Tone},
};

/// The classes of a diff: a suggestion's, or a file's.
pub(crate) const DIFF: &str = "my-2 overflow-x-auto rounded-md border border-line bg-canvas \
                               py-1.5 font-mono text-xs/5 wrap-anywhere whitespace-pre-wrap";

/// The classes of every listed card.
/// Scrolling a card into view keeps it clear of the heading that stays at the
/// top of the comments.
const CARD: &str =
    "scroll-mt-14 cursor-pointer rounded-lg border border-l-4 border-line p-3 text-sm";

/// The id of the card of the comment numbered `number`.
///
/// The `:` keeps it apart from the ids of a file's headings, which the page
/// also holds: a heading's anchor has none.
pub(crate) fn id(number: u32) -> String {
    format!("comment:{number}")
}

/// The classes of the edge that says where a comment stands.
fn edge(status: Status) -> &'static str {
    match status {
        Status::Draft => "border-l-draft",
        Status::Open => "border-l-accent",
        Status::Resolved => "border-l-line opacity-75",
    }
}

/// The classes of the listed card of a comment that stands at `status`, brought
/// out when `picked`.
fn frame(status: Status, picked: bool) -> String {
    let edge = edge(status);
    if picked {
        return format!("{CARD} {edge} bg-accent/10 ring-2 ring-accent");
    }

    format!("{CARD} {edge} bg-surface")
}

/// The classes of the mark saying where a comment stands.
fn status_mark(status: Status) -> &'static str {
    match status {
        Status::Draft => "rounded bg-draft/15 px-1.5 py-px text-xs font-medium text-draft",
        Status::Open => "rounded bg-accent/10 px-1.5 py-px text-xs font-medium text-accent",
        Status::Resolved => "rounded bg-raised px-1.5 py-px text-xs font-medium text-muted",
    }
}

/// The classes of the mark saying what the assistant made of a comment.
fn verdict_mark(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Accept => "rounded bg-added/15 px-1.5 py-px text-xs font-medium text-added",
        Verdict::Decline => "rounded bg-removed/15 px-1.5 py-px text-xs font-medium text-removed",
        Verdict::Amend | Verdict::Defer | Verdict::Ask => {
            "rounded bg-raised px-1.5 py-px text-xs font-medium text-muted"
        }
    }
}

/// The card of `comment` among the comments listed beside the file, shown on
/// the page `here`.
///
/// The card is brought out while `picked` holds its comment's number, and a
/// click on it tells `pick` that number.
/// It offers what can be done with the comment to `act`, when the review takes
/// changes.
pub(crate) fn card<A, P>(
    nav: Nav,
    comment: Comment,
    here: &ReviewView,
    act: Option<A>,
    picked: RwSignal<Option<u32>>,
    pick: P,
) -> impl RenderHtml + use<A, P>
where
    A: Fn(Action, &Comment) + Clone + Send + Sync + 'static,
    P: Fn(u32) + Send + Sync + 'static,
{
    let number = actions::number(&comment);
    let status = comment.status;

    article()
        .class(move || frame(status, number.is_some() && picked.get() == number))
        .attr("id", number.map(id))
        .on(ev::click, move |_| {
            if let Some(number) = number {
                pick(number);
            }
        })
        .child(contents(nav, comment, here, act, None))
}

/// The thread of `comment` in the file's text, under its passage, shown on the
/// page `here`.
///
/// It offers what can be done with the comment to `act`, when the review takes
/// changes, and `close` puts it away.
pub(crate) fn thread<A, C>(
    nav: Nav,
    comment: Comment,
    here: &ReviewView,
    act: Option<A>,
    close: C,
) -> impl RenderHtml + use<A, C>
where
    A: Fn(Action, &Comment) + Clone + Send + Sync + 'static,
    C: Fn() + Send + Sync + 'static,
{
    let class = format!("{INLINE} border-l-4 {}", edge(comment.status));
    let close = button()
        .attr("type", "button")
        .class(widget::small_button_class(Tone::Quiet))
        .on(ev::click, move |_| close())
        .child("Close")
        .into_any();

    article()
        .class(class)
        .attr("aria-label", "Comment")
        .child(contents(nav, comment, here, act, Some(close)))
}

/// What a comment's card holds.
///
/// `close` ends the heading of a thread in the file's text.
/// A thread sits right under the text its comment is on, so only a listed card
/// quotes that text.
fn contents<A>(
    nav: Nav,
    comment: Comment,
    here: &ReviewView,
    act: Option<A>,
    close: Option<AnyView>,
) -> impl RenderHtml + use<A>
where
    A: Fn(Action, &Comment) + Clone + Send + Sync + 'static,
{
    let number = actions::number(&comment);
    let status = comment.status;
    let outdated = outdated(nav, &comment, here);
    let on = on(&comment, close.is_none());
    let body = (!comment.body.is_empty())
        .then(|| div().class("jp-body").inner_html(comment.body_html.clone()));
    let resolved = comment.resolved_at.map(|at| {
        p().class("mt-2 text-xs text-muted")
            .child(format!("Resolved {}.", text::time(at)))
    });
    let buttons = act.map(|act| buttons(&comment, act));

    (
        header()
            .class("mb-1.5 flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted")
            .child((
                span()
                    .class("text-sm font-semibold text-ink")
                    .child(comment.author),
                span().class(status_mark(status)).child(status.to_string()),
                number.map(|number| span().class("ml-auto").child(format!("#{number}"))),
                close,
            )),
        outdated,
        on,
        body,
        comment
            .replies
            .into_iter()
            .map(|sent| reply(sent, false))
            .collect::<Vec<_>>(),
        comment.draft_reply.map(|draft| reply(draft, true)),
        resolved,
        buttons,
    )
}

/// A button for each thing that can be done with `comment`, which `act` does.
fn buttons<A>(comment: &Comment, act: A) -> impl RenderHtml + use<A>
where
    A: Fn(Action, &Comment) + Clone + Send + Sync + 'static,
{
    let buttons = actions::actions(comment)
        .into_iter()
        .map(|action| {
            let (act, comment) = (act.clone(), comment.clone());
            button()
                .attr("type", "button")
                .class(widget::small_button_class(Tone::Plain))
                .on(ev::click, move |event: MouseEvent| {
                    // The card's own click picks the comment, which a press
                    // on one of its buttons doesn't ask for.
                    event.stop_propagation();
                    act(action, &comment);
                })
                .child(action.label())
        })
        .collect::<Vec<_>>();

    div().class("mt-2.5 flex flex-wrap gap-1.5").child(buttons)
}

/// That the text `comment` is on changed since it was written, with a link to
/// the latest version still holding it; `None` while the text stands.
fn outdated(nav: Nav, comment: &Comment, here: &ReviewView) -> Option<AnyView> {
    if !comment.anchor.outdated {
        return None;
    }

    let Some(last) = comment.anchor.resolved.as_ref() else {
        return Some(
            p().class("mb-1.5 text-xs text-muted")
                .child(format!(
                    "Written on version {}, and not in this one.",
                    comment.anchor.version
                ))
                .into_any(),
        );
    };
    let to = Route::Review(ReviewView {
        version: Some(last.version),
        changes: None,
        ..here.clone()
    });

    Some(
        p().class("mb-1.5 text-xs text-muted")
            .child((
                "The text changed since; ",
                link(
                    nav,
                    to,
                    widget::LINK,
                    format!("last seen in version {}", last.version),
                ),
                ".",
            ))
            .into_any(),
    )
}

/// What `comment` is on: the diff a suggestion makes, or, when `quoted`, the
/// start of the text its range covers, which says which passage of the file a
/// listed card belongs to.
fn on(comment: &Comment, quoted: bool) -> Option<AnyView> {
    match (&comment.suggestion, &comment.excerpt) {
        (Some(replacement), excerpt) => {
            Some(suggestion(excerpt.as_deref().unwrap_or_default(), replacement).into_any())
        }
        (None, Some(excerpt)) if quoted && !excerpt.trim().is_empty() => Some(
            blockquote()
                .class("mb-1.5 line-clamp-2 border-l-2 border-line pl-2 text-xs text-muted")
                .child(excerpt.clone())
                .into_any(),
        ),
        (None, _) => None,
    }
}

/// A reply under a comment; `draft` for the reviewer's reply the next pass
/// sends.
fn reply(reply: Reply, draft: bool) -> impl RenderHtml {
    let role = match reply.role {
        Role::Reviewer => "reviewer",
        Role::Assistant => "assistant",
    };

    div()
        .class("mt-2.5 border-t border-line pt-2.5")
        .attr("data-role", role)
        .child((
            div()
                .class("mb-1 flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted")
                .child((
                    span()
                        .class("text-sm font-semibold text-ink")
                        .child(reply.author),
                    reply.verdict.map(|verdict| {
                        span()
                            .class(verdict_mark(verdict))
                            .child(verdict.to_string())
                    }),
                    draft.then(|| span().class(status_mark(Status::Draft)).child("draft")),
                )),
            div().class("jp-body").inner_html(reply.body_html),
        ))
}

/// A suggestion as the diff it makes to the text its range covers.
fn suggestion(excerpt: &str, replacement: &str) -> impl RenderHtml {
    pre().class(DIFF).child(
        text::suggestion(excerpt, replacement)
            .into_iter()
            .map(|(kind, line)| line_view(kind, line))
            .collect::<Vec<_>>(),
    )
}

/// One line of a diff, styled as what it is.
pub(crate) fn line_view(kind: Line, line: String) -> impl RenderHtml {
    div().class(kind.class()).child(line)
}
