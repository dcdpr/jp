//! Writing comments, replies, and pass messages: the bar a selection in the
//! file shows, and the composer sheet.
//!
//! The composer's draft is kept in local storage as it is written, so a tab iOS
//! reloads opens with it, and saves against the file, version, and bytes it was
//! written for.

use std::time::Duration;

use leptos_dom::helpers::event_target_value;
use reactive_graph::{
    computed::Memo, effect::Effect, owner::StoredValue, prelude::*, signal::RwSignal, spawn_local,
};
use serde_json::Value;
use tachys::{
    html::{
        element::{button, div, h2, header, label, p, pre, section, span, textarea},
        event as ev,
    },
    prelude::*,
};

use crate::{
    capture,
    card::line_view,
    client,
    composer::{self, Draft, Kind, Target},
    dom,
    selection::Refusal,
    text,
};

/// How long a selection has to stay as it is before it is read, so dragging its
/// handles doesn't read it on every step.
pub(crate) const SETTLE: Duration = Duration::from_millis(150);

/// What the last selection in the file pane was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Pending {
    /// A range a comment can be anchored to.
    Captured(Target),

    /// A selection no comment can be anchored to, and why.
    Refused(Refusal),
}

/// A range of a file being commented on: its path, the version it is in, and
/// its source bytes.
pub(crate) type Marked = (String, u32, [usize; 2]);

/// The state of writing in a review page, every part of it a reactive handle.
#[derive(Clone, Copy)]
pub(crate) struct Compose {
    /// The review: `R-1h4k2m9`.
    pub id: StoredValue<String>,

    /// What the composer holds; `None` while it is closed.
    pub draft: RwSignal<Option<Draft>>,

    /// The selection the comment bar offers to comment on.
    pub pending: RwSignal<Option<Pending>>,

    /// The range being commented on, to mark in the file: the composer's, or
    /// the pending selection's.
    ///
    /// Typing in the composer leaves it as it is.
    pub marked: Memo<Option<Marked>>,

    /// Why the last save failed.
    pub problem: RwSignal<Option<String>>,

    pub saving: RwSignal<bool>,

    /// Changed to ask the page to poll at once.
    pub refresh: RwSignal<u32>,

    /// When the last press on the comment bar began, in milliseconds since the
    /// epoch.
    pub pressed: StoredValue<Option<f64>>,
}

impl Compose {
    /// The writing state of the review `id`, with the draft local storage kept
    /// for it, which it keeps there from then on.
    pub(crate) fn new(id: &str) -> Self {
        let key = composer::key(id);
        let draft = RwSignal::new(dom::stored(&key).as_deref().and_then(composer::decode));
        Effect::new(move |_| {
            draft.with(|draft| dom::store(&key, draft.as_ref().map(composer::encode).as_deref()));
        });
        let pending = RwSignal::new(None);

        Self {
            id: StoredValue::new(id.to_owned()),
            draft,
            pending,
            marked: Memo::new(move |_| marked(draft, pending)),
            problem: RwSignal::new(None),
            saving: RwSignal::new(false),
            refresh: RwSignal::new(0),
            pressed: StoredValue::new(None),
        }
    }

    /// Whether the composer is open, tracked.
    pub(crate) fn composing(self) -> bool {
        self.draft.with(Option::is_some)
    }

    /// Open the composer on `draft`, asking first when it holds something the
    /// reader wrote.
    pub(crate) fn open(self, draft: Draft) {
        let written = self
            .draft
            .with_untracked(|draft| draft.as_ref().is_some_and(written));
        if written && !dom::confirm("Discard what you are writing?") {
            return;
        }

        self.pending.set(None);
        self.problem.set(None);
        self.draft.set(Some(draft));
    }

    /// Close the composer, dropping its draft.
    pub(crate) fn close(self) {
        self.problem.set(None);
        self.draft.set(None);
    }

    /// Whether something is being commented on: a selection is pending, or the
    /// composer is open.
    /// While it is, the file pane doesn't move to another version on its own.
    pub(crate) fn holding(self) -> bool {
        self.draft.with_untracked(Option::is_some)
            || self
                .pending
                .with_untracked(|pending| matches!(pending, Some(Pending::Captured(_))))
    }

    /// Ask the page to poll at once.
    pub(crate) fn refreshed(self) {
        self.refresh.update(|count| *count = count.wrapping_add(1));
    }
}

/// The range `draft` comments on, or else the one `pending` offers.
fn marked(draft: RwSignal<Option<Draft>>, pending: RwSignal<Option<Pending>>) -> Option<Marked> {
    let range = |target: &Target| match target {
        Target::Range {
            path,
            version,
            bytes,
            ..
        } => Some((path.clone(), *version, *bytes)),
        Target::Review | Target::File { .. } => None,
    };

    let drafted = draft.with(|draft| match &draft.as_ref()?.kind {
        Kind::Comment { on } => range(on),
        Kind::Edit { .. } | Kind::Reply { .. } | Kind::Pass => None,
    });
    drafted.or_else(|| {
        pending.with(|pending| match pending {
            Some(Pending::Captured(target)) => range(target),
            Some(Pending::Refused(_)) | None => None,
        })
    })
}

/// Whether `draft` holds something the reader wrote: a body, or a suggestion
/// that isn't the text it started from.
fn written(draft: &Draft) -> bool {
    let suggested = draft
        .replacement
        .as_deref()
        .is_some_and(|replacement| Some(replacement) != draft.kind.excerpt());

    !draft.body.trim().is_empty() || suggested
}

/// Save the composer's draft, closing it once the server took it.
///
/// The draft stays when the server refuses it, as it does when what it was
/// written for was resolved or withdrawn meanwhile, for its text to be copied.
fn save(compose: Compose) {
    let Some(draft) = compose.draft.get_untracked() else {
        return;
    };
    if let Some(problem) = composer::problem(&draft) {
        compose.problem.set(Some(problem.to_owned()));
        return;
    }

    let call = composer::call(&compose.id.get_value(), &draft);
    compose.saving.set(true);
    spawn_local(async move {
        let answer = client::send::<Value>(call).await;
        compose.saving.set(false);
        // Another composer was opened meanwhile, which this save leaves alone.
        if compose
            .draft
            .with_untracked(|open| open.as_ref() != Some(&draft))
        {
            compose.refreshed();
            return;
        }
        if let Err(error) = answer {
            compose.problem.set(Some(error));
            return;
        }

        if matches!(draft.kind, Kind::Comment {
            on: Target::Range { .. }
        }) {
            capture::clear();
        }
        compose.close();
        compose.refreshed();
    });
}

/// The bar a selection in the file shows: what to do with it, or why nothing
/// can be; nothing while the composer is open.
pub(crate) fn bar(compose: Compose) -> impl RenderHtml {
    move || {
        if compose.composing() {
            return None;
        }
        let choices = match compose.pending.get()? {
            Pending::Captured(target) => {
                let suggested = target.clone();
                (
                    choice("Comment", move || {
                        compose.open(Draft::comment(target.clone()));
                    }),
                    choice("Suggest a change", move || {
                        compose.open(Draft::suggestion(suggested.clone()));
                    }),
                )
                    .into_any()
            }
            Pending::Refused(refusal) => {
                p().class("jp-bar-note").child(refusal.reason()).into_any()
            }
        };

        Some(
            div()
                .class("jp-comment-bar")
                .attr("role", "toolbar")
                // Pressing a button would otherwise end the selection on a
                // desktop browser before the button saw the click.
                .on(ev::mousedown, |event| event.prevent_default())
                // A tap ends it on a touch screen regardless; the time of the
                // press tells the selection that ends next to keep its range.
                .on(ev::pointerdown, move |_| {
                    compose.pressed.set_value(Some(dom::now()));
                })
                .child((
                    choices,
                    choice("Dismiss", move || compose.pending.set(None)),
                )),
        )
    }
}

/// A button that runs `then`.
fn choice(name: &'static str, then: impl Fn() + Send + Sync + 'static) -> impl RenderHtml {
    button()
        .attr("type", "button")
        .class("jp-button")
        .on(ev::click, move |_| then())
        .child(name)
}

/// The composer sheet, while the composer is open.
pub(crate) fn sheet(compose: Compose) -> impl RenderHtml {
    let kind = Memo::new(move |_| {
        compose
            .draft
            .with(|draft| draft.as_ref().map(|draft| draft.kind.clone()))
    });

    move || kind.get().map(|kind| form(compose, &kind))
}

/// The composer, writing a draft of `kind`.
fn form(compose: Compose, kind: &Kind) -> impl RenderHtml + use<> {
    let title = kind.title();
    let suggest = kind
        .excerpt()
        .map(|excerpt| suggestion(compose, excerpt.to_owned(), kind.can_unsuggest()));

    section()
        .class("jp-sheet")
        .attr("role", "dialog")
        .attr("aria-label", title.clone())
        .child((
            header().class("jp-sheet-head").child((
                h2().child(title),
                choice("Discard", move || compose.close()),
            )),
            suggest,
            body(compose, kind),
            move || {
                compose
                    .problem
                    .get()
                    .map(|problem| p().class("jp-error").child(problem))
            },
            div().class("jp-sheet-actions").child(
                button()
                    .attr("type", "button")
                    .class("jp-button jp-button-primary")
                    .prop("disabled", move || compose.saving.get())
                    .on(ev::click, move |_| save(compose))
                    .child(kind.save()),
            ),
        ))
}

/// The field the draft's body is written in.
fn body(compose: Compose, kind: &Kind) -> impl RenderHtml + use<> {
    let hint = match kind {
        Kind::Comment { .. } | Kind::Edit { .. } => "Comment, in Markdown",
        Kind::Reply { .. } => "Reply, in Markdown",
        Kind::Pass => "A message to the assistant beside the comments (optional)",
    };

    label().class("jp-field").child((
        span().child(hint),
        textarea()
            .attr("rows", "4")
            .prop("value", move || {
                compose
                    .draft
                    .with(|draft| draft.as_ref().map(|draft| draft.body.clone()))
                    .unwrap_or_default()
            })
            .prop("disabled", move || compose.saving.get())
            .on(ev::input, move |event| {
                let value = event_target_value(&event);
                compose.draft.update(|draft| {
                    if let Some(draft) = draft {
                        draft.body = value;
                    }
                });
            }),
    ))
}

/// The suggestion a comment on the range holding `excerpt` makes: a button to
/// make one, or the replacement and the change it makes; `removable` when it
/// can be taken back.
fn suggestion(compose: Compose, excerpt: String, removable: bool) -> impl RenderHtml {
    let suggesting = Memo::new(move |_| {
        compose.draft.with(|draft| {
            draft
                .as_ref()
                .is_some_and(|draft| draft.replacement.is_some())
        })
    });
    let excerpt = StoredValue::new(excerpt);

    move || {
        if !suggesting.get() {
            return choice("Suggest a change", move || {
                compose.draft.update(|draft| {
                    if let Some(draft) = draft {
                        draft.replacement = Some(excerpt.get_value());
                    }
                });
            })
            .into_any();
        }

        div()
            .class("jp-suggest")
            .child((
                replacement(compose),
                preview(compose, excerpt),
                removable.then(|| {
                    choice("Don't suggest a change", move || {
                        compose.draft.update(|draft| {
                            if let Some(draft) = draft {
                                draft.replacement = None;
                            }
                        });
                    })
                }),
            ))
            .into_any()
    }
}

/// The field the replacement is written in, Markdown syntax included.
fn replacement(compose: Compose) -> impl RenderHtml {
    label().class("jp-field").child((
        span().child("Replace the selection with"),
        textarea()
            .attr("rows", "3")
            .attr("spellcheck", "true")
            .prop("value", move || {
                compose
                    .draft
                    .with(|draft| draft.as_ref().and_then(|draft| draft.replacement.clone()))
                    .unwrap_or_default()
            })
            .prop("disabled", move || compose.saving.get())
            .on(ev::input, move |event| {
                let value = event_target_value(&event);
                compose.draft.update(|draft| {
                    if let Some(draft) = draft {
                        draft.replacement = Some(value);
                    }
                });
            }),
    ))
}

/// The change the suggestion makes to `excerpt`, as a diff.
fn preview(compose: Compose, excerpt: StoredValue<String>) -> impl RenderHtml {
    pre().class("jp-diff jp-suggestion").child(move || {
        let replacement = compose
            .draft
            .with(|draft| draft.as_ref().and_then(|draft| draft.replacement.clone()))
            .unwrap_or_default();

        excerpt.with_value(|excerpt| {
            text::suggestion(excerpt, &replacement)
                .into_iter()
                .map(|(kind, line)| line_view(kind, line))
                .collect::<Vec<_>>()
        })
    })
}
