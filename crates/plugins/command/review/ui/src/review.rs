//! `/reviews/{id}`: one review, a file of it with the comments on it, and what
//! changed in that file from one version to another.
//!
//! The page polls the review ([`crate::poll`]), placing its comments in the
//! version shown, and pairs that answer with the same version's rendered file.
//! When a pass this page watched ends, the page switches to what that pass
//! changed, unless the reviewer is commenting on something.
//! `write` holds what the page asks the server to change.

mod write;

use jp_review_api::{
    Change, Detail, DetailReply, DiffView, FileDiff, FileReply, Pass, Running, Version,
};
use leptos_dom::helpers::{event_target_value, request_animation_frame};
use reactive_graph::{
    computed::Memo,
    effect::Effect,
    owner::{StoredValue, on_cleanup},
    prelude::*,
    signal::RwSignal,
    spawn_local,
};
use tachys::{
    html::{
        element::{Div, div, h1, header, main, option, p, pre, section, select, span},
        event as ev,
    },
    prelude::*,
    reactive_graph::node_ref::NodeRef,
};

use crate::{
    blocks::{self, Block},
    card::{card, line_view},
    client,
    compose::{self, Compose},
    dom::{Nav, keep_polling, link},
    highlight,
    layout::{self, CHANGED, PENDING},
    mapping, poll,
    route::{Changes, ReviewView, Route, Span},
    text,
};

/// What every poll says of a review, beside the review itself.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Status {
    running: Option<Running>,
    working_tree_changed: bool,
}

/// What the review page holds, every part of it a reactive handle.
#[derive(Clone, Copy)]
struct Page {
    nav: Nav,
    view: Memo<ReviewView>,

    /// The review, its comments placed in the version it names.
    review: RwSignal<Option<Detail>>,
    status: RwSignal<Option<Status>>,

    /// Why the last poll failed, until one doesn't.
    error: RwSignal<Option<String>>,

    /// The file shown: the one the location names, or the review's first.
    shown: Memo<Option<String>>,

    /// The file shown, rendered, as the version shown holds it.
    file: RwSignal<Option<Result<FileReply, String>>>,

    /// What changed in the file shown, in the changes view.
    diff: RwSignal<Option<Result<DiffView, String>>>,

    /// How many changed lines of the file render to nothing.
    hidden: RwSignal<usize>,

    /// What is being written: the pending selection and the composer; read
    /// through [`Page::compose`].
    compose: StoredValue<Compose>,

    /// Why the last change asked of the server failed, until dismissed.
    notice: RwSignal<Option<String>>,
}

impl Page {
    /// What is being written on the page.
    fn compose(self) -> Compose {
        self.compose.get_value()
    }
}

/// The page of the review `id`, which notes the workspace each answer names in
/// `seen`.
pub(crate) fn page(nav: Nav, id: String, seen: RwSignal<Option<String>>) -> impl RenderHtml {
    let view = Memo::new(move |_| match nav.route.get() {
        Route::Review(view) => view,
        Route::List | Route::Unknown(_) => ReviewView::default(),
    });
    let review = RwSignal::new(None::<Detail>);
    let shown = Memo::new(move |_| {
        view.with(|view| view.file.clone())
            .or_else(|| review.with(|review| review.as_ref()?.head.files.first().cloned()))
    });
    let page = Page {
        nav,
        view,
        review,
        status: RwSignal::new(None),
        error: RwSignal::new(None),
        shown,
        file: RwSignal::new(None),
        diff: RwSignal::new(None),
        hidden: RwSignal::new(0),
        compose: StoredValue::new(Compose::new(&id)),
        notice: RwSignal::new(None),
    };

    follow(page, id.clone(), seen);
    fetch_diff(page, id.clone());
    fetch_file(page, id);
    let pane = NodeRef::<Div>::new();
    paint(page, pane);
    write::follow_selection(page, pane);
    write::drop_selection(page);

    main().class("jp-page jp-review").child((
        top(page),
        move || {
            page.error
                .get()
                .map(|error| p().class("jp-error").child(error))
        },
        move || write::notice(page),
        move || body(page, pane),
        compose::bar(page.compose()),
        compose::sheet(page.compose()),
    ))
}

/// Poll the review for as long as the page shows it, asking again at once when
/// the version shown changes.
fn follow(page: Page, id: String, seen: RwSignal<Option<String>>) {
    // The version and revision of the review last read, which a poll for the
    // same version sends to be told only of what changed.
    let cursor = RwSignal::new(None::<(Option<u32>, u64)>);
    let refresh = page.compose().refresh;
    Effect::new(move |before: Option<Option<u32>>| {
        let version = page.view.with(|view| view.version);
        if before.is_some_and(|before| before != version) {
            refresh.update(|count| *count = count.wrapping_add(1));
        }
        version
    });

    keep_polling(
        move || {
            page.status.with_untracked(|status| {
                status
                    .as_ref()
                    .is_some_and(|status| status.running.is_some())
            })
        },
        refresh,
        move || {
            let id = id.clone();
            async move { read(page, cursor, seen, &id).await }
        },
    );
}

/// Ask for the review as the version shown holds it, and take in the answer.
async fn read(
    page: Page,
    cursor: RwSignal<Option<(Option<u32>, u64)>>,
    seen: RwSignal<Option<String>>,
    id: &str,
) {
    let asked = page.view.with_untracked(|view| view.version);
    let since = cursor
        .get_untracked()
        .filter(|(version, _)| *version == asked)
        .map(|(_, revision)| revision);
    let answer = client::get::<DetailReply>(&client::detail_url(id, since, asked)).await;
    // Another version was asked for meanwhile, whose comments sit elsewhere.
    if page.view.with_untracked(|view| view.version) != asked {
        return;
    }
    let reply = match answer {
        Ok(reply) => reply,
        Err(error) => return page.error.set(Some(error)),
    };

    seen.set(Some(reply.workspace.clone()));
    let before = page.status.with_untracked(|status| {
        status
            .as_ref()
            .and_then(|status| status.running.as_ref())
            .map(|running| running.number)
    });
    let now = reply.running.as_ref().map(|running| running.number);
    if let Some(review) = reply.review.filter(|review| !held_back(page, review)) {
        cursor.set(Some((asked, reply.revision)));
        page.review.set(Some(review));
    }
    page.status.set(Some(Status {
        running: reply.running,
        working_tree_changed: reply.working_tree_changed,
    }));
    page.error.set(None);

    // Only a pass that recorded the files has changes to show, and the page
    // stays where it is while the reviewer comments on something.
    if let Some(pass) = poll::ended(before, now).filter(|pass| recorded(page, *pass))
        && !page.compose().holding()
    {
        let view = page.view.get_untracked();
        page.nav.go(Route::Review(ReviewView {
            changes: Some(Changes {
                span: Span::Pass(pass),
                unified: false,
            }),
            ..view
        }));
    }
}

/// Whether `review`, the latest, is kept off the page for now: the page follows
/// the latest version, `review` is a newer one than the one shown, and
/// something in the shown one is being commented on.
///
/// A version the reader picked is never held back.
/// The poll cursor stays where it was, so the next poll after the comment is
/// saved or dropped brings the newer version.
fn held_back(page: Page, review: &Detail) -> bool {
    if page.view.with_untracked(|view| view.version.is_some()) {
        return false;
    }
    let newer = page.review.with_untracked(|shown| {
        shown
            .as_ref()
            .is_some_and(|shown| shown.version != review.version)
    });

    newer && page.compose().holding()
}

/// Whether pass `number` ended with the files recorded.
fn recorded(page: Page, number: u32) -> bool {
    page.review.with_untracked(|review| {
        review.as_ref().is_some_and(|review| {
            review
                .passes
                .iter()
                .find(|pass| pass.number == number)
                .and_then(|pass| pass.ended.as_ref())
                .is_some_and(|ended| ended.version.is_some())
        })
    })
}

/// Fetch what changed in the file shown whenever the changes view asks for
/// another span or file, or the review gains a version.
fn fetch_diff(page: Page, id: String) {
    let key = Memo::new(move |_| {
        let changes = page.view.with(|view| view.changes)?;
        let file = page.shown.get()?;
        let latest = page
            .review
            .with(|review| Some(review.as_ref()?.versions.last()?.number));

        Some((changes.span, file, latest))
    });

    Effect::new(move |_| {
        page.diff.set(None);
        let Some((span, file, latest)) = key.get() else {
            return;
        };
        let id = id.clone();
        spawn_local(async move {
            let answer = client::get::<DiffView>(&client::diff_url(&id, span, &file)).await;
            if key.get_untracked() == Some((span, file, latest)) {
                page.diff.set(Some(answer));
            }
        });
    });
}

/// Fetch the file shown, rendered, as the version shown holds it: the review's
/// in the file view, and the later version compared in the changes view.
fn fetch_file(page: Page, id: String) {
    let key = Memo::new(move |_| {
        let file = page.shown.get()?;
        if page.view.with(|view| view.changes.is_none()) {
            let version = page.review.with(|review| Some(review.as_ref()?.version))?;
            return Some((version, file));
        }

        page.diff.with(|diff| {
            let diff = diff.as_ref()?.as_ref().ok()?;
            let deleted = entry(diff, &file).is_some_and(|entry| entry.change == Change::Deleted);
            (!deleted).then_some((diff.to, file))
        })
    });

    Effect::new(move |_| {
        page.file.set(None);
        let Some((version, file)) = key.get() else {
            return;
        };
        let id = id.clone();
        spawn_local(async move {
            let answer = client::get::<FileReply>(&client::file_url(&id, version, &file)).await;
            if key.get_untracked() == Some((version, file)) {
                page.file.set(Some(answer));
            }
        });
    });
}

/// The entry of `file` in `diff`; `None` when the file didn't change.
fn entry<'a>(diff: &'a DiffView, file: &str) -> Option<&'a FileDiff> {
    diff.files.iter().find(|entry| entry.path == file)
}

/// Highlight, over the file in `pane`, the comments' ranges in the file view,
/// and the changed lines in the changes view, each time either changes.
fn paint(page: Page, pane: NodeRef<Div>) {
    Effect::new(move |_| {
        let element = pane.get();
        let shown = page.file.with(|file| {
            file.as_ref()
                .and_then(|file| file.as_ref().ok())
                .map(|file| (file.source.clone(), file.version))
        });
        let (Some(element), Some((source, version)), Some(file)) =
            (element, shown, page.shown.get())
        else {
            highlight::clear();
            return;
        };

        let (wanted, removed, added) = match page.view.with(|view| view.changes) {
            Some(changes) if changes.unified => {
                highlight::clear();
                return;
            }
            Some(_) => page.diff.with(|diff| {
                let entry = diff
                    .as_ref()
                    .and_then(|diff| diff.as_ref().ok())
                    .and_then(|diff| entry(diff, &file));
                let added = entry.map_or_default(|entry| entry.added.clone());
                let wanted = added.iter().map(|bytes| (CHANGED, *bytes)).collect();

                (
                    wanted,
                    entry.map_or_default(|entry| entry.removed.clone()),
                    added,
                )
            }),
            None => (marks(page, &file, version), vec![], vec![]),
        };

        // Once the browser has laid the file out, which the effect may run
        // before.
        drop(request_animation_frame(move || {
            highlight::paint(&element, &source, &wanted);
            let spans = highlight::spans(&element);
            highlight::mark_removed(&spans, &removed);
            let plain: Vec<mapping::Span> = spans.iter().map(|(_, span)| *span).collect();
            page.hidden.set(mapping::hidden(&plain, &source, &added));
        }));
    });
    on_cleanup(highlight::clear);
}

/// What the file view highlights in version `version` of `file`: the comments'
/// ranges, and the range being commented on.
fn marks(page: Page, file: &str, version: u32) -> Vec<(&'static str, [usize; 2])> {
    let resolved = page.view.with(|view| view.resolved);
    let mut wanted = page.review.with(|review| {
        review
            .as_ref()
            .map_or_default(|review| layout::highlights(&review.comments, file, resolved))
    });
    if let Some((path, marked, bytes)) = page.compose().marked.get()
        && path == file
        && marked == version
    {
        wanted.push((PENDING, bytes));
    }

    wanted
}

/// The header: the review's title, the version shown, the pass running or the
/// last one's closing message, the passes' changes, and the file tabs.
fn top(page: Page) -> impl RenderHtml {
    header().class("jp-header").child((
        div().class("jp-bar").child((
            link(page.nav, Route::List, "jp-home", "jp"),
            h1().class("jp-title").child(move || {
                page.review
                    .with(|review| review.as_ref().map(|review| review.head.title.clone()))
                    .unwrap_or_default()
            }),
            move || {
                page.status.with(|status| {
                    status
                        .as_ref()?
                        .running
                        .as_ref()
                        .map(|running| span().class("jp-running").child(text::running(running)))
                })
            },
            write::controls(page),
        )),
        move || closing(page),
        move || passes(page),
        move || tabs(page),
        move || banner(page),
        move || toolbar(page),
    ))
}

/// The last pass's outcome and closing message, once it has ended.
fn closing(page: Page) -> Option<impl RenderHtml> {
    let pass = page
        .review
        .with(|review| review.as_ref()?.passes.last().cloned())?;
    let answer = pass.ended.as_ref()?.answer.clone();

    Some(div().class("jp-closing").child((
        div().class("jp-closing-head").child(text::pass(&pass)),
        answer.map(|answer| p().class("jp-closing-body").child(answer)),
    )))
}

/// A link to what each ended pass changed in the file shown.
fn passes(page: Page) -> Option<impl RenderHtml> {
    let passes: Vec<Pass> = page.review.with(|review| {
        review
            .as_ref()
            .map_or_default(|review| review.passes.clone())
    });
    let view = page.view.get();
    let links = passes
        .iter()
        .rev()
        .filter(|pass| {
            pass.ended
                .as_ref()
                .is_some_and(|ended| ended.version.is_some())
        })
        .map(|pass| {
            let span = Span::Pass(pass.number);
            let current = view.changes.is_some_and(|changes| changes.span == span);
            let to = Route::Review(ReviewView {
                changes: Some(Changes {
                    span,
                    unified: view.changes.is_some_and(|changes| changes.unified),
                }),
                ..view.clone()
            });
            let class = if current {
                "jp-chip jp-chip-active"
            } else {
                "jp-chip"
            };
            link(page.nav, to, class, text::pass(pass))
        })
        .collect::<Vec<_>>();
    if links.is_empty() {
        return None;
    }

    Some(
        div()
            .class("jp-passes")
            .child((span().class("jp-label").child("Changes by pass"), links)),
    )
}

/// A tab for each of the review's files, with how many of its comments are not
/// resolved, and a button for a comment on the file shown.
fn tabs(page: Page) -> Option<impl RenderHtml> {
    let view = page.view.get();
    let shown = page.shown.get();
    let comment = write::file_comment(page);
    page.review.with(|review| {
        let review = review.as_ref()?;
        let tabs = review
            .head
            .files
            .iter()
            .map(|path| {
                let count = layout::unresolved(&review.comments, path);
                let to = Route::Review(ReviewView {
                    file: Some(path.clone()),
                    ..view.clone()
                });
                let class = if shown.as_deref() == Some(path.as_str()) {
                    "jp-tab jp-tab-active"
                } else {
                    "jp-tab"
                };
                link(
                    page.nav,
                    to,
                    class,
                    (
                        path.clone(),
                        (count > 0).then(|| span().class("jp-count").child(count.to_string())),
                    ),
                )
            })
            .collect::<Vec<_>>();

        Some(
            div()
                .class("jp-tabs")
                .attr("role", "navigation")
                .child((tabs, comment)),
        )
    })
}

/// That the files on disk changed since the latest version, while no pass runs,
/// whose end records them.
fn banner(page: Page) -> Option<impl RenderHtml> {
    let changed = page.status.with(|status| {
        status
            .as_ref()
            .is_some_and(|status| status.working_tree_changed && status.running.is_none())
    });
    let latest = page
        .review
        .with(|review| Some(review.as_ref()?.versions.last()?.number))?;

    changed.then(|| {
        div().class("jp-banner").child((
            p().child(format!(
                "The files on disk changed since version {latest}, which this page shows the \
                 latest of."
            )),
            write::record_button(page),
        ))
    })
}

/// What the file pane shows, and how to show something else: the version or the
/// two compared, resolved comments, and rendered or unified changes.
fn toolbar(page: Page) -> Option<AnyView> {
    let view = page.view.get();
    let versions: Vec<Version> = page.review.with(|review| {
        review
            .as_ref()
            .map_or_default(|review| review.versions.clone())
    });
    let shown = page.review.with(|review| Some(review.as_ref()?.version))?;

    Some(match view.changes {
        None => file_tools(page.nav, &view, &versions, shown).into_any(),
        Some(changes) => change_tools(page, &view, &versions, changes).into_any(),
    })
}

/// The file view's tools: the version shown, what changed in it, and whether
/// resolved comments are highlighted.
fn file_tools(nav: Nav, view: &ReviewView, versions: &[Version], shown: u32) -> impl RenderHtml {
    let resolved = view.resolved;
    let changes = ReviewView {
        changes: Some(Changes {
            span: Span::Versions {
                from: None,
                to: Some(shown),
            },
            unified: false,
        }),
        ..view.clone()
    };

    div().class("jp-toolbar").child((
        picker(nav, versions, shown, "Version", {
            let view = view.clone();
            move |version| ReviewView {
                version: Some(version),
                ..view.clone()
            }
        }),
        link(nav, Route::Review(changes), "jp-button", "Changes"),
        link(
            nav,
            Route::Review(ReviewView {
                resolved: !resolved,
                ..view.clone()
            }),
            "jp-button",
            if resolved {
                "Hide resolved"
            } else {
                "Show resolved"
            },
        ),
    ))
}

/// The changes view's tools: the versions compared, rendered or unified, and
/// the way back to the file.
fn change_tools(
    page: Page,
    view: &ReviewView,
    versions: &[Version],
    changes: Changes,
) -> impl RenderHtml {
    let nav = page.nav;
    let compared = page.diff.with(|diff| {
        let diff = diff.as_ref()?.as_ref().ok()?;
        Some((diff.from, diff.to))
    });
    let span_view = match (changes.span, compared) {
        (Span::Pass(pass), Some((from, to))) => span()
            .class("jp-label")
            .child(format!("Pass {pass}: version {from} to {to}"))
            .into_any(),
        (Span::Pass(pass), None) => span()
            .class("jp-label")
            .child(format!("Pass {pass}"))
            .into_any(),
        (Span::Versions { .. }, Some((from, to))) => (
            picker(nav, versions, from, "From version", {
                let view = view.clone();
                move |from| compare(&view, Some(from), Some(to))
            }),
            span().class("jp-label").child("to"),
            picker(nav, versions, to, "To version", {
                let view = view.clone();
                move |to| compare(&view, Some(from), Some(to))
            }),
        )
            .into_any(),
        (Span::Versions { .. }, None) => ().into_any(),
    };
    let unified = changes.unified;

    div().class("jp-toolbar").child((
        span_view,
        link(
            nav,
            Route::Review(ReviewView {
                changes: Some(Changes {
                    unified: !unified,
                    ..changes
                }),
                ..view.clone()
            }),
            "jp-button",
            if unified { "Rendered" } else { "Unified diff" },
        ),
        link(
            nav,
            Route::Review(ReviewView {
                changes: None,
                version: None,
                ..view.clone()
            }),
            "jp-button",
            "Back to the file",
        ),
    ))
}

/// `view` comparing versions `from` and `to`.
fn compare(view: &ReviewView, from: Option<u32>, to: Option<u32>) -> ReviewView {
    ReviewView {
        changes: Some(Changes {
            span: Span::Versions { from, to },
            unified: view.changes.is_some_and(|changes| changes.unified),
        }),
        ..view.clone()
    }
}

/// A list of `versions`, `selected` chosen, that shows the page `to` names for
/// the version picked.
fn picker(
    nav: Nav,
    versions: &[Version],
    selected: u32,
    label: &'static str,
    to: impl Fn(u32) -> ReviewView + Send + 'static,
) -> impl RenderHtml {
    select()
        .class("jp-select")
        .attr("aria-label", label)
        .on(ev::change, move |event| {
            if let Ok(version) = event_target_value(&event).parse() {
                nav.go(Route::Review(to(version)));
            }
        })
        .child(
            versions
                .iter()
                .rev()
                .map(|version| {
                    option()
                        .attr("value", version.number.to_string())
                        .attr("selected", version.number == selected)
                        .child(text::version(version))
                })
                .collect::<Vec<_>>(),
        )
}

/// The file pane: the file with its comments, or what changed in it.
fn body(page: Page, pane: NodeRef<Div>) -> impl RenderHtml {
    let view = page.view.get();
    let Some(file) = page.shown.get() else {
        return p().class("jp-note").child("Loading…").into_any();
    };
    if let Some(changes) = view.changes {
        return changed(page, &file, changes, pane).into_any();
    }

    match page.file.get() {
        None => p().class("jp-note").child("Loading…").into_any(),
        Some(Err(error)) => p().class("jp-error").child(error).into_any(),
        Some(Ok(reply)) => commented(page, &view, &file, &reply, pane).into_any(),
    }
}

/// The file, each comment on a range of it under the block its range starts in,
/// and the comments on the review, the whole file, and changed text above it.
fn commented(
    page: Page,
    view: &ReviewView,
    file: &str,
    reply: &FileReply,
    pane: NodeRef<Div>,
) -> impl RenderHtml {
    let blocks = blocks::blocks(reply.html.as_deref().unwrap_or_default());
    let comments = page.review.with(|review| {
        review
            .as_ref()
            .map_or_default(|review| review.comments.clone())
    });
    let placement = layout::place(&comments, file, &blocks);
    let act = write::actor(page);
    let cards = |indices: &[usize]| {
        indices
            .iter()
            .map(|index| card(page.nav, comments[*index].clone(), view, act.clone()))
            .collect::<Vec<_>>()
    };
    let above = (!placement.above.is_empty())
        .then(|| section().class("jp-above").child(cards(&placement.above)));
    let rows = blocks
        .into_iter()
        .zip(&placement.under)
        .map(|(block, under)| {
            div()
                .class("jp-row")
                .child((text_of(block), div().class("jp-cards").child(cards(under))))
        })
        .collect::<Vec<_>>();

    div()
        .class("jp-file")
        .child((above, div().class("jp-pane").node_ref(pane).child(rows)))
}

/// What changed in the file: the later version rendered with its changed lines
/// highlighted, or the unified diff.
fn changed(page: Page, file: &str, changes: Changes, pane: NodeRef<Div>) -> impl RenderHtml {
    let diff = match page.diff.get() {
        None => return p().class("jp-note").child("Loading…").into_any(),
        Some(Err(error)) => return p().class("jp-error").child(error).into_any(),
        Some(Ok(diff)) => diff,
    };
    let view = page.view.get();
    let entry = entry(&diff, file).cloned();
    let unified_view = ReviewView {
        changes: Some(Changes {
            unified: true,
            ..changes
        }),
        ..view
    };

    let deleted = entry
        .as_ref()
        .is_some_and(|entry| entry.change == Change::Deleted);
    if let Some(entry) = entry.as_ref().filter(|_| changes.unified || deleted) {
        return div()
            .class("jp-file")
            .child((
                deleted.then(|| {
                    p().class("jp-note")
                        .child(format!("{file} is deleted in version {}.", diff.to))
                }),
                unified(&entry.diff),
            ))
            .into_any();
    }

    let unchanged = entry.is_none().then(|| {
        p().class("jp-note").child(format!(
            "{file} didn't change from version {} to version {}.",
            diff.from, diff.to
        ))
    });
    div()
        .class("jp-file jp-changes")
        .child((unchanged, hidden(page, unified_view), rendered(page, pane)))
        .into_any()
}

/// How many changed lines render to nothing, with a link to `unified`, the
/// unified diff that shows them; nothing while none do.
fn hidden(page: Page, unified: ReviewView) -> impl RenderHtml {
    move || {
        let count = page.hidden.get();
        (count > 0).then(|| {
            p().class("jp-note").child((
                text::hidden(count),
                "; ",
                link(
                    page.nav,
                    Route::Review(unified.clone()),
                    "jp-link",
                    "see the unified diff",
                ),
                ".",
            ))
        })
    }
}

/// The file as the later version compared holds it, without comments, for the
/// changed lines to be highlighted over.
fn rendered(page: Page, pane: NodeRef<Div>) -> AnyView {
    match page.file.get() {
        None => p().class("jp-note").child("Loading…").into_any(),
        Some(Err(error)) => p().class("jp-error").child(error).into_any(),
        Some(Ok(reply)) => {
            let rows = blocks::blocks(reply.html.as_deref().unwrap_or_default())
                .into_iter()
                .map(|block| div().class("jp-row").child(text_of(block)))
                .collect::<Vec<_>>();
            div().class("jp-pane").node_ref(pane).child(rows).into_any()
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
fn unified(diff: &str) -> impl RenderHtml {
    pre().class("jp-diff jp-unified").child(
        text::diff_lines(diff)
            .into_iter()
            .map(|(kind, line)| line_view(kind, line.to_owned()))
            .collect::<Vec<_>>(),
    )
}
