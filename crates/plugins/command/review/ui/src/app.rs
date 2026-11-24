//! The UI's root: the page the location names, and a banner when the server
//! behind the page now serves another workspace.

use leptos_dom::helpers::window;
use reactive_graph::{computed::Memo, prelude::*, signal::RwSignal};
use tachys::{
    html::{
        element::{a, button, div, h1, main, p},
        event as ev,
    },
    prelude::*,
};

use crate::{
    dom::{self, Nav},
    list, review,
    route::Route,
};

/// Which page shows; the review page keeps its state while only what it shows
/// of the review changes.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Page {
    List,
    Review(String),
    Unknown(String),
}

/// The whole UI.
pub(crate) fn app() -> impl RenderHtml {
    dom::follow_viewport();
    dom::follow_builds();
    let nav = Nav::new();
    let served = dom::workspace();
    let seen = RwSignal::new(None::<String>);
    let page = Memo::new(move |_| match nav.route.get() {
        Route::List => Page::List,
        Route::Review(view) => Page::Review(view.id),
        Route::Unknown(path) => Page::Unknown(path),
    });

    div().class("jp-app").child((
        move || moved(served.clone(), seen.get()),
        move || match page.get() {
            Page::List => list::page(nav, seen).into_any(),
            Page::Review(id) => review::page(nav, id, seen).into_any(),
            Page::Unknown(path) => unknown(&path).into_any(),
        },
    ))
}

/// The banner asking to reload, when the page was served for the workspace
/// `served` and the server now answers for `seen`, as it does when a tab stays
/// open while another workspace's server takes over the port.
fn moved(served: Option<String>, seen: Option<String>) -> Option<impl RenderHtml> {
    let (served, seen) = (served?, seen?);
    if served == seen {
        return None;
    }

    Some(
        div().class("jp-banner jp-banner-warn").child((
            p().child(format!(
                "This page was opened for {served}, and the server now serves {seen}."
            )),
            button()
                .attr("type", "button")
                .class("jp-button")
                .on(ev::click, |_| drop(window().location().reload()))
                .child("Reload"),
        )),
    )
}

/// A page for a location the UI has none for.
fn unknown(path: &str) -> impl RenderHtml {
    main().class("jp-page").child((
        h1().child("No page here"),
        p().child(format!("The review UI has no page at {path}.")),
        a().attr("href", "/").class("jp-link").child("All reviews"),
    ))
}
