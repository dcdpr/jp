//! What the UI reads from and asks of the browser around it: the location, the
//! history, whether the page shows, the clock, local storage, the part of the
//! page the reader sees, and what the page is scrolled to.

use std::{cell::Cell, future::Future, rc::Rc, time::Duration};

use leptos_dom::helpers::{document, set_interval, window, window_event_listener_untyped};
use reactive_graph::{owner::on_cleanup, prelude::*, signal::RwSignal, spawn_local};
use send_wrapper::SendWrapper;
use tachys::{
    html::{class::IntoClass, element::a, event as ev},
    prelude::*,
};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use web_sys::{
    Element, Event, HtmlElement, MouseEvent, Range, ScrollIntoViewOptions, ScrollLogicalPosition,
};

use crate::{
    client, layout, poll,
    popover::Rect,
    route::{Route, unescape},
};

/// The media query of a screen wide enough for the comments to sit beside the
/// file: Tailwind's `lg` breakpoint, which the views' classes switch at.
const WIDE: &str = "(min-width: 64rem)";

/// How far under the top of the window a passage scrolled to rests, in CSS
/// pixels: clear of the top bar.
const REST: f64 = 96.0;

/// How much of the window's height is left over the comments, where they are a
/// sheet over the bottom of a narrow screen.
const OVER_SHEET: f64 = 0.42;

/// Moves between the UI's pages without reloading, keeping the location and the
/// history in step with the page shown.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Nav {
    pub route: RwSignal<Route>,
}

impl Nav {
    /// The page the location names, following the browser's back and forward.
    pub(crate) fn new() -> Self {
        let route = RwSignal::new(location());
        let handle = window_event_listener_untyped("popstate", move |_| route.set(location()));
        on_cleanup(move || handle.remove());

        Self { route }
    }

    /// Show `to`, adding it to the history.
    pub(crate) fn go(self, to: Route) {
        if self.route.get_untracked() == to {
            return;
        }
        // A history that can't be written to still shows the page; only back
        // misses it.
        if let Ok(history) = window().history() {
            drop(history.push_state_with_url(&JsValue::NULL, "", Some(&to.href())));
        }
        self.route.set(to);
    }
}

/// The route the browser's location names.
pub(crate) fn location() -> Route {
    let location = window().location();
    let path = location.pathname().unwrap_or_default();
    let search = location.search().unwrap_or_default();

    Route::parse(&path, search.trim_start_matches('?'))
}

/// A link to `to`, followed without reloading the page on a plain click; a
/// click asking for a new tab or window is left to the browser.
pub(crate) fn link<C, K>(nav: Nav, to: Route, class: K, child: C) -> impl RenderHtml
where
    C: RenderHtml + 'static,
    K: IntoClass + 'static,
{
    a().attr("href", to.href())
        .class(class)
        .on(ev::click, move |event: MouseEvent| {
            if plain(&event) {
                event.prevent_default();
                nav.go(to.clone());
            }
        })
        .child(child)
}

/// Whether `event` is a plain click: the main button, with no key held that
/// asks the browser for a new tab or window, or to download.
pub(crate) fn plain(event: &MouseEvent) -> bool {
    event.button() == 0
        && !(event.ctrl_key() || event.meta_key() || event.shift_key() || event.alt_key())
}

/// Call `listener` with each `name` event fired at the document, until the
/// calling component is cleaned up.
///
/// For events, such as `selectionchange`, that don't bubble to the window.
pub(crate) fn document_listener(name: &'static str, listener: impl Fn(Event) + 'static) {
    let listener = Closure::<dyn Fn(Event)>::new(listener).into_js_value();
    // A listener that can't be added is a feature of the page that doesn't
    // work; the page itself still does.
    drop(document().add_event_listener_with_callback(name, listener.unchecked_ref()));

    // The page runs on one thread, which is the one that drops the wrapper.
    let listener = SendWrapper::new(listener);
    on_cleanup(move || {
        drop(document().remove_event_listener_with_callback(name, listener.unchecked_ref()));
    });
}

/// What local storage holds under `key`; `None` when nothing is, or the browser
/// keeps no storage for the page.
pub(crate) fn stored(key: &str) -> Option<String> {
    window().local_storage().ok()??.get_item(key).ok()?
}

/// Keep `value` in local storage under `key`, or remove what is there for
/// `None`.
///
/// A browser refusing to store it, as one in private mode may, loses the draft
/// on a reload and nothing else.
pub(crate) fn store(key: &str, value: Option<&str>) {
    let Ok(Some(storage)) = window().local_storage() else {
        return;
    };
    drop(match value {
        Some(value) => storage.set_item(key, value),
        None => storage.remove_item(key),
    });
}

/// Ask the reader `question`, returning whether they agreed.
pub(crate) fn confirm(question: &str) -> bool {
    window().confirm_with_message(question).unwrap_or(false)
}

/// Whether the page matches the media query `query`; `false` in a browser that
/// can't say.
fn matches(query: &str) -> bool {
    window()
        .match_media(query)
        .ok()
        .flatten()
        .is_some_and(|list| list.matches())
}

/// Whether the comments sit beside the file, and not in a sheet over it.
pub(crate) fn wide() -> bool {
    matches(WIDE)
}

/// The fragment of the page's address, as a link to one heading ends in:
/// `commenting` of `#commenting`; `None` when the address has none.
pub(crate) fn fragment() -> Option<String> {
    let hash = window().location().hash().ok()?;
    let fragment = hash.trim_start_matches('#');

    (!fragment.is_empty()).then(|| unescape(fragment, false))
}

/// Make `anchor` the fragment of the page's address, which scrolls the page to
/// the heading with that anchor, and lets the browser's back button return.
pub(crate) fn go_to_heading(anchor: &str) {
    // A browser refusing the address leaves the page where it was.
    drop(window().location().set_hash(anchor));
}

/// Scroll the page to the heading with the anchor `anchor` in the file drawn in
/// `pane`, when it has one.
///
/// The heading rests where the stylesheet's `scroll-margin-top` puts it, clear
/// of the top bar.
pub(crate) fn show_heading(pane: &Element, anchor: &str) {
    let heading = document()
        .get_element_by_id(anchor)
        .filter(|heading| pane.contains(Some(heading)));
    if let Some(heading) = heading {
        heading.scroll_into_view();
    }
}

/// Scroll the page to its start.
pub(crate) fn show_top() {
    window().scroll_to_with_x_and_y(0.0, 0.0);
}

/// Open `address` in a tab of its own, which leaves this page as it is and
/// gives the page opened no handle on it.
pub(crate) fn open(address: &str) {
    // A browser that blocks the tab leaves the link unfollowed.
    drop(window().open_with_url_and_target_and_features(address, "_blank", "noopener"));
}

/// The size of the window, in CSS pixels.
fn window_size() -> (f64, f64) {
    let side = |side: Result<JsValue, JsValue>| {
        side.ok().and_then(|side| side.as_f64()).unwrap_or_default()
    };

    (side(window().inner_width()), side(window().inner_height()))
}

/// The part of the page the reader sees, in the coordinates
/// `getBoundingClientRect` answers in: the window, less what the on-screen
/// keyboard covers and what a pinch zoomed out of view.
pub(crate) fn visible() -> Rect {
    let Some(viewport) = window().visual_viewport() else {
        let (width, height) = window_size();
        return Rect {
            left: 0.0,
            top: 0.0,
            right: width,
            bottom: height,
        };
    };

    Rect {
        left: viewport.offset_left(),
        top: viewport.offset_top(),
        right: viewport.offset_left() + viewport.width(),
        bottom: viewport.offset_top() + viewport.height(),
    }
}

/// Scroll the page so the text `range` covers shows: under the top bar, and
/// over the comments where they are a sheet over the bottom of the screen.
///
/// Text that shows there already stays where it is.
pub(crate) fn scroll_to(range: &Range) {
    let (_, height) = window_size();
    let until = if wide() { height } else { height * OVER_SHEET };
    let rect = range.get_bounding_client_rect();
    let by = layout::scroll_for([rect.top(), rect.bottom()], [REST, until]);

    window().scroll_by_with_x_and_y(0.0, by);
}

/// Scroll what holds the element with the id `id` just far enough for the
/// element to show.
pub(crate) fn show(id: &str) {
    let Some(element) = document().get_element_by_id(id) else {
        return;
    };
    let nearest = ScrollIntoViewOptions::new();
    nearest.set_block(ScrollLogicalPosition::Nearest);

    element.scroll_into_view_with_scroll_into_view_options(&nearest);
}

/// Keep `--jp-viewport`, the height of the part of the page the reader sees,
/// and `--jp-keyboard`, how much of the window the on-screen keyboard covers,
/// set on the document's root as the visual viewport changes, for as long as
/// the page shows.
///
/// The composer sits above the keyboard by these; a browser without a visual
/// viewport leaves them unset, and the composer at the bottom of the window.
pub(crate) fn follow_viewport() {
    let Some(viewport) = window().visual_viewport() else {
        return;
    };
    measure_viewport();

    // Kept for the life of the page, as the viewport is.
    let listener = Closure::<dyn Fn()>::new(measure_viewport).into_js_value();
    for name in ["resize", "scroll"] {
        drop(viewport.add_event_listener_with_callback(name, listener.unchecked_ref()));
    }
}

fn measure_viewport() {
    let Some(viewport) = window().visual_viewport() else {
        return;
    };
    let Some(root) = document()
        .document_element()
        .and_then(|root| root.dyn_into::<HtmlElement>().ok())
    else {
        return;
    };
    let window_height = window()
        .inner_height()
        .ok()
        .and_then(|height| height.as_f64())
        .unwrap_or_default();
    let keyboard = (window_height - viewport.height() - viewport.offset_top()).max(0.0);

    let style = root.style();
    drop(style.set_property("--jp-viewport", &format!("{}px", viewport.height())));
    drop(style.set_property("--jp-keyboard", &format!("{keyboard}px")));
}

/// The name of the workspace the page was served for, from its `jp-workspace`
/// meta tag.
pub(crate) fn workspace() -> Option<String> {
    meta("jp-workspace")
}

/// The content of the page's meta tag named `name`.
fn meta(name: &str) -> Option<String> {
    document()
        .query_selector(&format!(r#"meta[name="{name}"]"#))
        .ok()
        .flatten()?
        .get_attribute("content")
}

/// Reload the page once the server names another build of the UI than the one
/// the page started with, for as long as the page shows.
///
/// Only a page served from a bundle directory (`jp review serve --ui-dir`)
/// names its build, in a `jp-ui-build` meta tag; for any other, this does
/// nothing.
/// A hidden page doesn't ask, and a question still out holds back the next.
pub(crate) fn follow_builds() {
    let Some(loaded) = meta("jp-ui-build") else {
        return;
    };
    let busy = Rc::new(Cell::new(false));

    // Kept for the life of the page; a page that reloads ends it anyway.
    drop(set_interval(
        move || {
            if document().hidden() || busy.replace(true) {
                return;
            }
            let (busy, loaded) = (Rc::clone(&busy), loaded.clone());
            spawn_local(async move {
                // A server that can't be reached, as while it restarts, names
                // no build.
                let now = client::get::<String>(client::BUILD_URL)
                    .await
                    .unwrap_or_default();
                if poll::rebuilt(&loaded, &now) {
                    drop(window().location().reload());
                }
                busy.set(false);
            });
        },
        Duration::from_millis(poll::BUILD_EVERY),
    ));
}

/// Milliseconds since the epoch, by the browser's clock.
pub(crate) fn now() -> f64 {
    js_sys::Date::now()
}

/// Run `fetch` whenever a poll is due, as [`poll::due`] decides, while the page
/// shows, and at once when it shows again.
///
/// `running` says whether a pass runs, which polls more often.
/// Each change of `refresh` makes a poll due at once.
/// A poll doesn't start while the one before it is still out.
/// Stops when the calling component is cleaned up.
pub(crate) fn keep_polling<F, Fut>(
    running: impl Fn() -> bool + 'static,
    refresh: RwSignal<u32>,
    fetch: F,
) where
    F: Fn() -> Fut + 'static,
    Fut: Future<Output = ()> + 'static,
{
    let last = Rc::new(Cell::new(None::<f64>));
    let busy = Rc::new(Cell::new(false));
    let refreshed = Rc::new(Cell::new(refresh.get_untracked()));
    let fetch = Rc::new(fetch);

    let tick = {
        let last = Rc::clone(&last);
        move || {
            let asked = refresh.get_untracked();
            if refreshed.replace(asked) != asked {
                last.set(None);
            }
            if document().hidden() || busy.get() || !poll::due(now(), last.get(), running()) {
                return;
            }

            busy.set(true);
            last.set(Some(now()));
            let (busy, fetch) = (Rc::clone(&busy), Rc::clone(&fetch));
            spawn_local(async move {
                fetch().await;
                busy.set(false);
            });
        }
    };
    let tick = Rc::new(tick);

    tick();
    let every = Rc::clone(&tick);
    // An interval that can't be set leaves the page as first fetched; the
    // browser refuses one only when it is going away.
    drop(set_interval(
        move || every(),
        Duration::from_millis(poll::TICK),
    ));

    // iOS suspends a hidden tab and may thaw it much later.
    let shown = window_event_listener_untyped("visibilitychange", move |_| {
        if !document().hidden() {
            last.set(None);
            tick();
        }
    });
    on_cleanup(move || shown.remove());
}
