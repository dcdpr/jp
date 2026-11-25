//! Writing comments, replies, and pass messages: the actions a selection in the
//! file is offered, and the composer.
//!
//! The actions show beside the selection, where [`crate::popover`] places them.
//!
//! The composer is a sheet over the bottom of a narrow screen, above the
//! on-screen keyboard.
//! On a wide screen, what is about a passage of the file (a new comment on a
//! range, an edit of one, a reply to one) is written in the file's text, right
//! under the passage, in the element the page seats there ([`Compose::host`]);
//! anything else is written under the button that opened the composer: the top
//! bar's, which sends a pass, or the comments', which adds a comment.
//! [`crate::layout::site`] says which, and [`at`] draws the composer there.
//!
//! The active composer draft is persisted in the tab's `sessionStorage` as the
//! user types so that an iOS tab reload restores it and submits against the
//! captured file, version, and byte range.
//! Because `sessionStorage` is isolated per tab, saving or discarding in one
//! tab does not affect drafts in other tabs.
//!
//! [`layout::site`]: crate::layout::site

use std::time::Duration;

use leptos_dom::helpers::{event_target_value, request_animation_frame};
use reactive_graph::{
    computed::Memo, effect::Effect, owner::StoredValue, prelude::*, signal::RwSignal, spawn_local,
};
use serde_json::Value;
use tachys::{
    html::{
        element::{Div, Section, button, div, h2, header, label, p, pre, section, span, textarea},
        event as ev,
    },
    prelude::*,
    reactive_graph::node_ref::NodeRef,
};
use wasm_bindgen::JsCast;
use web_sys::{HtmlElement, KeyboardEvent};

pub(crate) use crate::highlight::PLAIN;
use crate::{
    capture,
    card::{DIFF, line_view},
    client,
    composer::{self, Draft, Kind, Target},
    dom,
    layout::Site,
    popover::{self, Anchor, Point, Size},
    selection::Refusal,
    text,
    widget::{self, Tone},
};

/// How long a selection has to stay as it is before it is read, so dragging its
/// handles doesn't read it on every step.
pub(crate) const SETTLE: Duration = Duration::from_millis(150);

/// What the reader last pointed at in the file pane, which they are offered
/// something to do with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Pending {
    /// A selection a comment can be anchored to.
    Captured(Target),

    /// A selection no comment can be anchored to, and why.
    Refused(Refusal),

    /// A click on the passage the approval with this number approves.
    Approved(u32),
}

/// What the reader marks in the file, apart from commenting on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Mark {
    /// Approve the range.
    Approve(Target),

    /// Say the file was read through the block the range ends in.
    ReadTo(Target),

    /// Take back the approval with this number.
    Unapprove(u32),
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

    /// The selection the reader is offered to comment on.
    pub pending: RwSignal<Option<Pending>>,

    /// What the actions for the pending selection are placed beside.
    pub anchor: RwSignal<Option<Anchor>>,

    /// The range the composer writes a new comment on, which the file marks in
    /// place of the browser's selection of it.
    ///
    /// Typing in the composer leaves it as it is.
    pub marked: Memo<Option<Marked>>,

    /// Why the last save failed.
    pub problem: RwSignal<Option<String>>,

    pub saving: RwSignal<bool>,

    /// Changed to ask the page to poll at once.
    pub refresh: RwSignal<u32>,

    /// When the last press on the selection's actions began, in milliseconds
    /// since the epoch.
    pub pressed: StoredValue<Option<f64>>,

    /// How many times the composer was opened; a change tells the page to hand
    /// the composer the keyboard.
    pub opened: RwSignal<u32>,

    /// The element that sits in the file's text, under a passage: the page
    /// moves it there, and draws the composer in it while it writes about that
    /// passage.
    pub host: NodeRef<Div>,

    /// The composer's form, while it is open.
    pub form: NodeRef<Section>,
}

/// The classes of what holds the actions offered for a selection, or the note
/// in their place.
/// As wide as what it holds, wherever in the file's column it is placed.
const OFFER: &str = "absolute z-30 w-max max-w-[calc(100vw-1rem)]";

/// The classes of the actions offered for a selection that can take a comment.
const ACTIONS: &str =
    "flex gap-0.5 rounded-lg border border-line bg-canvas p-0.5 shadow-md pointer-coarse:gap-1";

/// The classes of the note saying why a selection can't take a comment.
const REFUSAL: &str =
    "w-72 rounded-lg border border-line bg-canvas p-3 text-sm text-muted shadow-md";

/// The classes of the composer in a sheet over the bottom of the screen, above
/// the on-screen keyboard.
const SHEET: &str = "fixed inset-x-0 bottom-[var(--jp-keyboard,0px)] z-40 grid \
                     max-h-[calc(var(--jp-viewport,100vh)*0.85)] gap-2 overflow-y-auto \
                     rounded-t-xl border border-line bg-canvas p-3 \
                     pb-[calc(0.75rem+env(safe-area-inset-bottom))] \
                     shadow-[0_-8px_24px_rgb(0_0_0/0.18)]";

/// The classes of the composer under the top bar's button: as wide as the
/// comments' column, its right edge under the button's, and no taller than the
/// window under the bar.
/// The element holding the button and the composer is what it is placed in.
const UNDER_BUTTON: &str = "absolute top-full right-0 z-40 mt-2 grid max-h-[calc(100dvh-4.5rem)] \
                            w-88 max-w-[calc(100vw-1.5rem)] gap-2 overflow-y-auto rounded-xl \
                            border border-line bg-canvas p-3 shadow-lg";

/// The classes of the composer at the top of the comments, no taller than the
/// comments' column under its heading.
const IN_COMMENTS: &str = "mb-2 grid max-h-[calc(100dvh-7rem)] gap-2 overflow-y-auto rounded-lg \
                           border border-line bg-surface p-3 shadow-sm";

/// The classes of a box in the file's text, as wide as the text around it,
/// which keeps its own type whatever block it sits in.
pub(crate) const INLINE: &str = "my-3 cursor-auto rounded-lg border border-line bg-surface p-3 \
                                 font-sans text-sm leading-normal font-normal tracking-normal \
                                 whitespace-normal text-ink shadow-sm";

impl Compose {
    /// Initialize composer state for review `id`, restoring any unsaved draft
    /// from `sessionStorage` and syncing subsequent edits back to storage.
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
            anchor: RwSignal::new(None),
            marked: Memo::new(move |_| marked(draft)),
            problem: RwSignal::new(None),
            saving: RwSignal::new(false),
            refresh: RwSignal::new(0),
            pressed: StoredValue::new(None),
            opened: RwSignal::new(0),
            host: NodeRef::new(),
            form: NodeRef::new(),
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
        // The file marks the range from here on, and the browser's selection
        // of it would only be drawn over that.
        if matches!(draft.kind, Kind::Comment {
            on: Target::Range { .. }
        }) {
            capture::clear();
        }

        self.pending.set(None);
        self.problem.set(None);
        self.draft.set(Some(draft));
        self.opened.update(|count| *count = count.wrapping_add(1));
    }

    /// Put the keyboard in the composer's first field.
    pub(crate) fn focus(self) {
        let field = self
            .form
            .get_untracked()
            .and_then(|form| form.query_selector("textarea").ok().flatten())
            .and_then(|field| field.dyn_into::<HtmlElement>().ok());
        if let Some(field) = field {
            // A field that can't take the keyboard is still there to click.
            drop(field.focus());
        }
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

    /// Return `true` if the owning page has been unmounted and its reactive
    /// state disposed.
    fn gone(self) -> bool {
        self.draft.try_with_untracked(|_| ()).is_none()
    }
}

/// The range `draft` writes a new comment on.
fn marked(draft: RwSignal<Option<Draft>>) -> Option<Marked> {
    draft.with(|draft| match &draft.as_ref()?.kind {
        Kind::Comment {
            on:
                Target::Range {
                    path,
                    version,
                    bytes,
                    ..
                },
        } => Some((path.clone(), *version, *bytes)),
        Kind::Comment { .. } | Kind::Edit { .. } | Kind::Reply { .. } | Kind::Pass => None,
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
        if compose.gone() {
            return;
        }
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

        compose.close();
        compose.refreshed();
    });
}

/// What a selection in the file is offered, beside it: commenting on it,
/// suggesting a change to it, approving it, or saying the file was read to it;
/// or why none of that can be done.
/// A click on an approved passage is offered taking the approval back.
/// Nothing is offered while the composer is open.
///
/// `mark` does what isn't a comment.
/// Deselecting, or pressing Escape, takes the offer away.
pub(crate) fn offer<M>(compose: Compose, mark: M) -> impl RenderHtml
where
    M: Fn(Mark) + Copy + Send + Sync + 'static,
{
    let bar = NodeRef::<Div>::new();
    // The top left corner of the actions, measured from that of the element
    // they are placed in; `None` until they are drawn and measured.
    let spot = RwSignal::new(None::<Point>);
    place(compose, bar, spot);

    move || {
        if compose.composing() {
            return None;
        }
        let content = match compose.pending.get()? {
            Pending::Captured(target) => actions(compose, target, mark).into_any(),
            Pending::Refused(refusal) => p()
                .class(REFUSAL)
                .attr("role", "status")
                .child(refusal.reason())
                .into_any(),
            Pending::Approved(number) => approved(number, mark).into_any(),
        };

        Some(
            div()
                .node_ref(bar)
                .class(OFFER)
                .attr("style", move || match spot.get() {
                    Some(spot) => format!("left:{:.0}px;top:{:.0}px", spot.x, spot.y),
                    None => "left:0;top:0;visibility:hidden".to_owned(),
                })
                .child(content),
        )
    }
}

/// Put the actions drawn in `bar` beside what [`Compose::anchor`] names, each
/// time either changes, writing where to `spot`.
///
/// The actions are measured as the browser drew them, so they are placed right
/// whatever its fonts make of their labels.
fn place(compose: Compose, bar: NodeRef<Div>, spot: RwSignal<Option<Point>>) {
    Effect::new(move |_| {
        spot.set(None);
        let (Some(element), Some(anchor)) = (bar.get(), compose.anchor.get()) else {
            return;
        };

        // Once the browser has laid the actions out, which the effect may run
        // before.
        drop(request_animation_frame(move || {
            // Actions that left the page meanwhile are placed in nothing.
            let Some(within) = element.offset_parent() else {
                return;
            };
            let drawn = element.get_bounding_client_rect();
            let within = within.get_bounding_client_rect();
            let size = Size {
                width: drawn.width(),
                height: drawn.height(),
            };
            let corner = popover::place(anchor, size, dom::visible());

            spot.set(Some(Point {
                x: corner.x - within.left(),
                y: corner.y - within.top(),
            }));
        }));
    });
}

/// What a click on the passage approval `number` approves is offered: that it
/// is approved, and taking that back, which `mark` does.
fn approved<M>(number: u32, mark: M) -> impl RenderHtml
where
    M: Fn(Mark) + Copy + Send + Sync + 'static,
{
    div()
        .class(ACTIONS)
        .attr("role", "toolbar")
        .attr("aria-label", "The approved passage")
        .child((
            span()
                .class("self-center px-2 text-sm font-medium text-added")
                .child("\u{2713} Approved"),
            widget::small_action(Tone::Quiet, "Remove", move || {
                mark(Mark::Unapprove(number));
            }),
        ))
}

/// What to do with a selection that covers `target`: comment on it, suggest a
/// change to it, or mark it, which `mark` does.
fn actions<M>(compose: Compose, target: Target, mark: M) -> impl RenderHtml
where
    M: Fn(Mark) + Copy + Send + Sync + 'static,
{
    let (suggested, passage, read_to) = (target.clone(), target.clone(), target.clone());

    div()
        .class(ACTIONS)
        .attr("role", "toolbar")
        .attr("aria-label", "The selection")
        // Pressing a button would otherwise end the selection on a desktop
        // browser before the button saw the click.
        .on(ev::mousedown, |event| event.prevent_default())
        // A tap ends it on a touch screen regardless; the time of the press
        // tells the selection that ends next to keep its range.
        .on(ev::pointerdown, move |_| {
            compose.pressed.set_value(Some(dom::now()));
        })
        .child((
            widget::small_action(Tone::Primary, "Comment", move || {
                compose.open(Draft::comment(target.clone()));
            }),
            widget::small_action(Tone::Quiet, "Suggest", move || {
                compose.open(Draft::suggestion(suggested.clone()));
            }),
            widget::small_action(Tone::Quiet, "Approve", move || {
                mark(Mark::Approve(passage.clone()));
            }),
            widget::small_action(Tone::Quiet, "Read to here", move || {
                mark(Mark::ReadTo(read_to.clone()));
            }),
        ))
}

/// What the composer writes, which changes only when it opens on something else
/// or closes.
pub(crate) fn kind(compose: Compose) -> Memo<Option<Kind>> {
    Memo::new(move |_| {
        compose
            .draft
            .with(|draft| draft.as_ref().map(|draft| draft.kind.clone()))
    })
}

/// The composer, while it is open and `site` says it is drawn `here`.
///
/// The page calls this once for each place the composer can be drawn in, and
/// `site` names one of them at a time.
pub(crate) fn at(compose: Compose, site: Memo<Option<Site>>, here: Site) -> impl RenderHtml {
    let kind = kind(compose);

    move || {
        kind.get()
            .filter(|_| site.get() == Some(here))
            .map(|kind| form(compose, &kind, here))
    }
}

/// The composer, writing a draft of `kind`, drawn at `site`.
///
/// It says what it writes on, except in the file's text, where the text right
/// over it says so.
fn form(compose: Compose, kind: &Kind, site: Site) -> impl RenderHtml + use<> {
    let in_file = site == Site::File;
    let title = kind.title();
    let suggest = kind
        .excerpt()
        .map(|excerpt| suggestion(compose, excerpt.to_owned(), kind.can_unsuggest()));
    let discard = move || widget::action(Tone::Quiet, "Discard", move || compose.close());
    let head = (!in_file).then(|| {
        header()
            .class("flex items-start justify-between gap-2")
            .child((
                h2().class("min-w-0 pt-1.5 text-sm font-semibold wrap-anywhere")
                    .child(title.clone()),
                discard(),
            ))
    });

    let class = match site {
        Site::File => format!("{INLINE} grid gap-2"),
        Site::Send => UNDER_BUTTON.to_owned(),
        Site::Comments => IN_COMMENTS.to_owned(),
        Site::Sheet => SHEET.to_owned(),
    };

    section()
        .node_ref(compose.form)
        .class(class)
        .attr("role", if in_file { "group" } else { "dialog" })
        .attr("aria-label", title)
        .child((
            head,
            suggest,
            body(compose, kind),
            move || {
                compose
                    .problem
                    .get()
                    .map(|problem| p().class(widget::ERROR).child(problem))
            },
            div().class("flex items-center justify-end gap-2").child((
                in_file.then(discard),
                button()
                    .attr("type", "button")
                    .class(widget::button_class(Tone::Primary))
                    .prop("disabled", move || compose.saving.get())
                    .on(ev::click, move |_| save(compose))
                    .child(kind.save()),
            )),
        ))
}

/// Save the composer's draft when `event` is Enter with Command or Control
/// held, as a field of the composer takes it.
fn save_on_enter(compose: Compose, event: &KeyboardEvent) {
    if event.key() == "Enter" && (event.meta_key() || event.ctrl_key()) {
        event.prevent_default();
        save(compose);
    }
}

/// The field the draft's body is written in.
fn body(compose: Compose, kind: &Kind) -> impl RenderHtml + use<> {
    let hint = match kind {
        Kind::Comment { .. } | Kind::Edit { .. } => "Comment, in Markdown",
        Kind::Reply { .. } => "Reply, in Markdown",
        Kind::Pass => "A message to the assistant beside the comments (optional)",
    };

    label().class(widget::FIELD).child((
        span().child(hint),
        textarea()
            .class(widget::INPUT)
            .attr("rows", "4")
            .prop("value", move || {
                compose
                    .draft
                    .with(|draft| draft.as_ref().map(|draft| draft.body.clone()))
                    .unwrap_or_default()
            })
            .prop("disabled", move || compose.saving.get())
            .on(ev::keydown, move |event: KeyboardEvent| {
                save_on_enter(compose, &event);
            })
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
            return div()
                .child(widget::action(Tone::Plain, "Suggest a change", move || {
                    compose.draft.update(|draft| {
                        if let Some(draft) = draft {
                            draft.replacement = Some(excerpt.get_value());
                        }
                    });
                }))
                .into_any();
        }

        div()
            .class("grid gap-2")
            .child((
                replacement(compose),
                preview(compose, excerpt),
                removable.then(|| {
                    div().child(widget::action(
                        Tone::Quiet,
                        "Don't suggest a change",
                        move || {
                            compose.draft.update(|draft| {
                                if let Some(draft) = draft {
                                    draft.replacement = None;
                                }
                            });
                        },
                    ))
                }),
            ))
            .into_any()
    }
}

/// The field the replacement is written in, Markdown syntax included.
fn replacement(compose: Compose) -> impl RenderHtml {
    label().class(widget::FIELD).child((
        span().child("Replace the selection with"),
        textarea()
            .class(format!("{} font-mono", widget::INPUT))
            .attr("rows", "3")
            .attr("spellcheck", "true")
            .prop("value", move || {
                compose
                    .draft
                    .with(|draft| draft.as_ref().and_then(|draft| draft.replacement.clone()))
                    .unwrap_or_default()
            })
            .prop("disabled", move || compose.saving.get())
            .on(ev::keydown, move |event: KeyboardEvent| {
                save_on_enter(compose, &event);
            })
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
    pre().class(DIFF).child(move || {
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
