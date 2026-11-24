//! A comment's card: who wrote it, where it stands, what it says, the replies
//! to it, and what the reviewer can do with it.
//!
//! Bodies are the HTML the server rendered with `md-html`, which escapes raw
//! HTML in them.

use jp_review_api::{Comment, Reply, Role};
use tachys::{
    html::{
        element::{article, blockquote, button, div, header, p, pre, span},
        event as ev,
    },
    prelude::*,
};

use crate::{
    actions::{self, Action},
    dom::{Nav, link},
    route::{ReviewView, Route},
    text::{self, Line},
};

/// The card of `comment`, shown on the page `here`, offering what can be done
/// with it to `act` when the review takes changes.
pub(crate) fn card<A>(
    nav: Nav,
    comment: Comment,
    here: &ReviewView,
    act: Option<A>,
) -> impl RenderHtml
where
    A: Fn(Action, &Comment) + Clone + Send + Sync + 'static,
{
    let number = comment
        .id
        .rsplit_once('#')
        .map_or_else(|| comment.id.clone(), |(_, number)| format!("#{number}"));
    let status = comment.status;
    let outdated = outdated(nav, &comment, here);
    let excerpt = excerpt(&comment);
    let body = (!comment.body.is_empty())
        .then(|| div().class("jp-body").inner_html(comment.body_html.clone()));
    let resolved = comment.resolved_at.map(|at| {
        p().class("jp-resolved-at")
            .child(format!("Resolved {}.", text::time(at)))
    });
    let buttons = act.map(|act| buttons(&comment, act));

    article()
        .class(format!("jp-card jp-card-{status}"))
        .attr("id", format!("comment-{}", number.trim_start_matches('#')))
        .child((
            header().class("jp-card-head").child((
                span().class("jp-card-number").child(number),
                span().class("jp-card-author").child(comment.author),
                span()
                    .class(format!("jp-status jp-status-{status}"))
                    .child(status.to_string()),
            )),
            outdated,
            excerpt,
            body,
            comment
                .replies
                .into_iter()
                .map(|sent| reply(sent, false))
                .collect::<Vec<_>>(),
            comment.draft_reply.map(|draft| reply(draft, true)),
            resolved,
            buttons,
        ))
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
                .class("jp-button jp-button-small")
                .on(ev::click, move |_| act(action, &comment))
                .child(action.label())
        })
        .collect::<Vec<_>>();

    div().class("jp-card-actions").child(buttons)
}

/// That the text `comment` is on changed since it was written, with a link to
/// the latest version still holding it; `None` while the text stands.
fn outdated(nav: Nav, comment: &Comment, here: &ReviewView) -> Option<AnyView> {
    if !comment.anchor.outdated {
        return None;
    }

    let Some(last) = comment.anchor.resolved.as_ref() else {
        return Some(
            p().class("jp-outdated")
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
        p().class("jp-outdated")
            .child((
                "The text changed since; ",
                link(
                    nav,
                    to,
                    "jp-link",
                    format!("last seen in version {}", last.version),
                ),
                ".",
            ))
            .into_any(),
    )
}

/// What `comment` is on: the diff a suggestion makes, or the text of an
/// outdated comment, which the file no longer highlights.
fn excerpt(comment: &Comment) -> Option<AnyView> {
    match (&comment.suggestion, &comment.excerpt) {
        (Some(replacement), excerpt) => {
            Some(suggestion(excerpt.as_deref().unwrap_or_default(), replacement).into_any())
        }
        (None, Some(excerpt)) if comment.anchor.outdated && !excerpt.is_empty() => Some(
            blockquote()
                .class("jp-excerpt")
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

    div().class(format!("jp-reply jp-reply-{role}")).child((
        div().class("jp-reply-head").child((
            span().class("jp-card-author").child(reply.author),
            reply.verdict.map(|verdict| {
                span()
                    .class(format!("jp-verdict jp-verdict-{verdict}"))
                    .child(verdict.to_string())
            }),
            draft.then(|| span().class("jp-status jp-status-draft").child("draft")),
        )),
        div().class("jp-body").inner_html(reply.body_html),
    ))
}

/// A suggestion as the diff it makes to the text its range covers.
fn suggestion(excerpt: &str, replacement: &str) -> impl RenderHtml {
    pre().class("jp-diff jp-suggestion").child(
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
