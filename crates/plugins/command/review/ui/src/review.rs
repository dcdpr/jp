//! `/reviews/{id}`: one review, a file of it beside the comments on it, and
//! what changed in that file from one version to another.
//!
//! The page polls the review ([`crate::poll`]), placing its comments in the
//! version shown, and pairs that answer with the same version's rendered file.
//! When a pass this page watched ends, the page switches to what that pass
//! changed, unless the reviewer is commenting on something.
//!
//! - `header` is the top bar: what the page shows, and sending to the
//!   assistant.
//! - `outline` is the file's sections, beside the file on a screen wide enough.
//! - `file` is the file, or what changed in it.
//! - `thread` is what sits in the file's text under a passage: the thread of
//!   the comment picked, and the composer while it writes about the passage.
//! - `progress` is the line across the file's text that says how far it was
//!   read, and the approvals of passages that changed since.
//! - `panel` is the comments on the file and the passes, beside the file on a
//!   wide screen and in a sheet over it on a narrow one.
//! - `write` is what the page asks the server to change, and what it reads of
//!   the reader's selections and clicks in the file.

mod file;
mod header;
mod outline;
mod panel;
mod progress;
mod thread;
mod write;

use jp_review_api::{Change, Detail, DetailReply, DiffView, FileDiff, FileReply, Running};
use leptos_dom::helpers::request_animation_frame;
use reactive_graph::{
    computed::Memo,
    effect::Effect,
    owner::{StoredValue, on_cleanup},
    prelude::*,
    signal::RwSignal,
    spawn_local,
};
use tachys::{
    html::element::{Div, div, main},
    prelude::*,
    reactive_graph::node_ref::NodeRef,
};
use web_sys::{Element, HtmlDivElement};

use crate::{
    actions, card, client,
    compose::{self, Compose},
    dom::{self, Nav, keep_polling},
    highlight,
    layout::{self, APPROVED, CHANGED, FOCUS, Inline, PENDING, Site},
    mapping, poll,
    route::{Changes, ReviewView, Route, Span},
    seat::{self, Order, Seat},
};

/// What the file is scrolled to once it is drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Reveal {
    /// The passage of the comment with this number.
    Passage(u32),

    /// The heading with this anchor.
    Heading(String),
}

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

    /// The number of the comment picked, whose card and passage are brought
    /// out.
    picked: RwSignal<Option<u32>>,

    /// What the file is scrolled to, once the file is drawn.
    reveal: StoredValue<Option<Reveal>>,

    /// Whether the comments show on a narrow screen, where they are a sheet
    /// over the file; a wide screen shows them either way.
    panel: RwSignal<bool>,

    /// The element holding the file's text.
    pane: NodeRef<Div>,

    /// The element the file's column is laid out in, which the actions for a
    /// selection are placed in.
    stage: NodeRef<Div>,
}

impl Page {
    /// What is being written on the page.
    fn compose(self) -> Compose {
        self.compose.get_value()
    }

    /// Whether the page was left, which disposes of its state.
    ///
    /// A task the page spawned checks this after each `await` before it touches
    /// the page: reading a disposed signal panics.
    fn gone(self) -> bool {
        self.view.try_with_untracked(|_| ()).is_none()
    }
}

/// The classes of what holds the page's columns, centered together: the file's
/// and the comments' side by side on a wide screen, and the file's outline
/// before them on a wider one.
/// The file's column is as wide as a page of the documentation site.
const LAYOUT: &str = "lg:grid lg:grid-cols-[minmax(0,43rem)_20rem] lg:justify-center lg:gap-x-8 \
                      lg:px-4 toc:grid-cols-[13rem_minmax(0,43rem)_20rem]";

/// The classes of the file's column.
const STAGE: &str =
    "relative min-w-0 px-4 py-6 lg:col-start-1 lg:row-start-1 lg:px-0 toc:col-start-2";

/// The classes of the file's column while the comments are a sheet over the
/// bottom of a narrow screen: room under the file to scroll its end out from
/// under the sheet.
const STAGE_UNDER_SHEET: &str = "relative min-w-0 px-4 py-6 max-lg:pb-[60dvh] lg:col-start-1 \
                                 lg:row-start-1 lg:px-0 toc:col-start-2";

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
        picked: RwSignal::new(None),
        // The heading the address names, as a link to one section does.
        reveal: StoredValue::new(dom::fragment().map(Reveal::Heading)),
        panel: RwSignal::new(false),
        pane: NodeRef::new(),
        stage: NodeRef::new(),
    };
    let inline = thread::inline(page);
    let site = site(page, inline);

    follow(page, id.clone(), seen);
    fetch_diff(page, id.clone());
    fetch_file(page, id);
    paint(page);
    show_card(page);
    thread::seat(page, inline);
    write::follow_selection(page);
    write::follow_keys(page);
    write::follow_history(page);
    write::drop_selection(page);

    main().child((header::top(page, site), columns(page, inline, site)))
}

/// Where the composer is drawn while it is open ([`layout::site`]), `inline`
/// saying whether it sits in the file's text.
fn site(page: Page, inline: Memo<Option<Inline>>) -> Memo<Option<Site>> {
    let kind = compose::kind(page.compose());

    Memo::new(move |_| {
        let in_file = inline.get().is_some_and(Inline::composing);

        kind.with(|kind| Some(layout::site(kind.as_ref()?, in_file, dom::wide())))
    })
}

/// The file's outline; the file's column, with the actions for a selection in
/// it and the element that sits under a passage, holding what `inline` says;
/// the comments' column; and the composer while `site` says it is in a sheet.
fn columns(page: Page, inline: Memo<Option<Inline>>, site: Memo<Option<Site>>) -> impl RenderHtml {
    div().class(LAYOUT).child((
        outline::outline(page),
        div()
            .class(move || {
                if page.panel.get() {
                    STAGE_UNDER_SHEET
                } else {
                    STAGE
                }
            })
            .node_ref(page.stage)
            .child((
                file::notices(page),
                move || file::body(page),
                compose::offer(page.compose(), move |mark| write::mark(page, mark)),
                thread::host(page, inline, site),
                progress::read_line(page),
            )),
        panel::panel(page, site),
        compose::at(page.compose(), site, Site::Sheet),
    ))
}

/// Move `host` under the text the bytes `under`, of the file shown, end in, as
/// the file is drawn in `pane`, at its place among what else sits there; or out
/// of the file's text, to the end of `stage`, when `under` is `None`, or none
/// of the text is drawn.
///
/// Returns where `host` sits in the file's text, or `None` when it was parked.
fn seat_under(
    page: Page,
    host: &Element,
    pane: Option<HtmlDivElement>,
    stage: &Element,
    under: Option<([usize; 2], Order)>,
) -> Option<Seat> {
    let seat = under.zip(pane).and_then(|((bytes, order), pane)| {
        page.file.with_untracked(|file| {
            let source = &file.as_ref()?.as_ref().ok()?.source;
            Some((seat::under(&pane, source, bytes)?, order))
        })
    });

    let Some((seat, order)) = seat else {
        seat::park(host, stage);
        return None;
    };
    seat::take(host, &seat, order);

    Some(seat)
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
    // The page was left meanwhile, or another version was asked for, whose
    // comments sit elsewhere.
    if page.view.try_with_untracked(|view| view.version) != Some(asked) {
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
            // `None` once the page was left.
            if key.try_get_untracked() == Some(Some((span, file, latest))) {
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
            // `None` once the page was left.
            if key.try_get_untracked() == Some(Some((version, file))) {
                page.file.set(Some(answer));
            }
        });
    });
}

/// The entry of `file` in `diff`; `None` when the file didn't change.
fn entry<'a>(diff: &'a DiffView, file: &str) -> Option<&'a FileDiff> {
    diff.files.iter().find(|entry| entry.path == file)
}

/// Highlight, over the file in the pane, the comments' passages in the file
/// view, and the changed lines in the changes view, each time either changes;
/// and scroll the file to what was asked for.
fn paint(page: Page) {
    Effect::new(move |_| {
        let element = page.pane.get();
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

        let changes = page.view.with(|view| view.changes);
        let (wanted, removed, added) = match changes {
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
            reveal(page, &element, &spans, &source, version);
        }));
    });
    on_cleanup(highlight::clear);
}

/// What the file view highlights in version `version` of `file`: the passages
/// approved, the comments' passages, the range being commented on, and the
/// passage of the comment picked.
fn marks(page: Page, file: &str, version: u32) -> Vec<(&'static str, [usize; 2])> {
    let resolved = page.view.with(|view| view.resolved);
    let picked = page.picked.get();
    let (mut wanted, picked) = page.review.with(|review| {
        let comments = review
            .as_ref()
            .map_or_default(|review| review.comments.as_slice());
        let approvals = review
            .as_ref()
            .map_or_default(|review| review.approvals.as_slice());
        let picked = comments
            .iter()
            .find(|comment| picked.is_some() && actions::number(comment) == picked)
            .and_then(|comment| layout::passage(comment, file));
        let mut wanted: Vec<_> = layout::approved(approvals, file)
            .into_iter()
            .map(|(_, bytes)| (APPROVED, bytes))
            .collect();
        wanted.extend(layout::highlights(comments, file, resolved));

        (wanted, picked)
    });
    if let Some((path, marked, bytes)) = page.compose().marked.get()
        && path == file
        && marked == version
    {
        wanted.push((PENDING, bytes));
    }
    wanted.extend(picked.map(|bytes| (FOCUS, bytes)));

    wanted
}

/// Scroll the file, drawn in `pane` as version `version` of `source`, to what
/// [`Page::reveal`] names, and forget the asking.
///
/// A pane that left the page leaves the asking for the next time the file is
/// drawn, and so does a passage while the file is drawn for another version
/// than the one the comments are placed in.
fn reveal(
    page: Page,
    pane: &Element,
    spans: &[(Element, mapping::Span)],
    source: &str,
    version: u32,
) {
    if !pane.is_connected() {
        return;
    }
    let number = match page.reveal.get_value() {
        None => return,
        Some(Reveal::Heading(anchor)) => {
            page.reveal.set_value(None);
            return dom::show_heading(pane, &anchor);
        }
        Some(Reveal::Passage(number)) => number,
    };
    let placed = page
        .review
        .with_untracked(|review| review.as_ref().map(|review| review.version));
    if placed != Some(version) {
        return;
    }
    page.reveal.set_value(None);

    let Some(file) = page.shown.get_untracked() else {
        return;
    };
    let passage = page.review.with_untracked(|review| {
        let comment = review
            .as_ref()?
            .comments
            .iter()
            .find(|comment| actions::number(comment) == Some(number))?;

        layout::passage(comment, &file)
    });
    let range =
        passage.and_then(|bytes| highlight::ranges(spans, source, bytes).into_iter().next());
    if let Some(range) = range {
        dom::scroll_to(&range);
    }
}

/// Scroll the comments to the card of the comment picked, each time one is.
fn show_card(page: Page) {
    Effect::new(move |_| {
        let Some(number) = page.picked.get() else {
            return;
        };
        // Once the comments are drawn, which the effect may run before: on a
        // narrow screen, picking a passage is what shows them.
        drop(request_animation_frame(move || {
            dom::show(&card::id(number));
        }));
    });
}
