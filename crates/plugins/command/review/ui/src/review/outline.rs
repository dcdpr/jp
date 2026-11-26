//! The outline of the file shown: its sections, each a link to its heading,
//! with the one being read marked.
//!
//! It shows beside the file on a screen wide enough for three columns, as "On
//! this page" does on the documentation site.

use std::{cell::Cell, rc::Rc};

use jp_review_api::Heading;
use leptos_dom::helpers::{document, request_animation_frame, window_event_listener_untyped};
use reactive_graph::{effect::Effect, owner::on_cleanup, prelude::*, signal::RwSignal};
use tachys::{
    html::element::{a, li, nav, p, ul},
    prelude::*,
};

use super::Page;
use crate::layout;

/// How far under the top of the window the line is that a heading is read from,
/// in CSS pixels: a little under where a link to a heading scrolls it.
const LINE: f64 = 96.0;

/// The classes of the outline: a column of the page's grid, as tall as the
/// window under the top bar, which stays in view while the file scrolls.
const OUTLINE: &str = "sticky top-12 hidden h-[calc(100dvh-3rem)] self-start overflow-y-auto py-6 \
                       pr-2 text-sm toc:col-start-1 toc:row-start-1 toc:block";

/// The classes of a link to a section, and of one to a subsection.
const SECTION: &str = "-ml-px block truncate border-l py-1 pl-4 leading-6";
const SUBSECTION: &str = "-ml-px block truncate border-l py-1 pl-8 leading-6";

/// The classes a link carries beside those: of the heading being read, and of
/// any other.
const READING: &str = "border-accent font-medium text-ink";
const OTHER: &str = "border-transparent text-muted hover:text-ink";

/// The headings the outline lists of the file shown; none while the file isn't
/// shown rendered.
fn listed(page: Page) -> Vec<Heading> {
    if page
        .view
        .with(|view| view.changes.is_some_and(|changes| changes.unified))
    {
        return vec![];
    }

    page.file.with(|file| {
        file.as_ref()
            .and_then(|file| file.as_ref().ok())
            .map_or_default(|file| {
                layout::outline(&file.headings)
                    .into_iter()
                    .cloned()
                    .collect()
            })
    })
}

/// The outline of the file shown.
pub(super) fn outline(page: Page) -> impl RenderHtml {
    // The anchor of the heading being read.
    let reading = RwSignal::new(None::<String>);
    follow(page, reading);

    nav()
        .class(OUTLINE)
        .attr("aria-label", "On this page")
        .child(move || {
            let headings = listed(page);
            if headings.is_empty() {
                return None;
            }
            let links = headings
                .into_iter()
                .map(|heading| li().child(entry(heading, reading)))
                .collect::<Vec<_>>();

            Some((
                p().class("mb-1 pl-4 text-sm leading-8 font-semibold")
                    .child("On this page"),
                ul().class("border-l border-line").child(links),
            ))
        })
}

/// A link to `heading`, marked while `reading` names it.
///
/// The browser follows it: the heading carries the anchor as its `id`.
fn entry(heading: Heading, reading: RwSignal<Option<String>>) -> impl RenderHtml {
    let shape = if heading.level > 2 {
        SUBSECTION
    } else {
        SECTION
    };
    let anchor = heading.anchor.clone();

    a().attr("href", format!("#{}", heading.anchor))
        .class(move || {
            let state = if reading.with(|reading| reading.as_deref() == Some(anchor.as_str())) {
                READING
            } else {
                OTHER
            };
            format!("{shape} {state}")
        })
        .child(heading.text)
}

/// Keep `reading` naming the heading being read, as the page scrolls and each
/// time the file is drawn.
fn follow(page: Page, reading: RwSignal<Option<String>>) {
    let measure = move || {
        let anchors: Vec<String> = page.file.with_untracked(|file| {
            file.as_ref()
                .and_then(|file| file.as_ref().ok())
                .map_or_default(|file| {
                    layout::outline(&file.headings)
                        .into_iter()
                        .map(|heading| heading.anchor.clone())
                        .collect()
                })
        });
        // A heading the page doesn't hold, as while the file loads, is far
        // below the line.
        let tops: Vec<f64> = anchors
            .iter()
            .map(|anchor| {
                document()
                    .get_element_by_id(anchor)
                    .map_or(f64::INFINITY, |heading| {
                        heading.get_bounding_client_rect().top()
                    })
            })
            .collect();
        let now = layout::reading(&tops, LINE).map(|index| anchors[index].clone());
        if reading.with_untracked(|reading| *reading != now) {
            reading.set(now);
        }
    };

    // One measure a frame, however many times the page says it scrolled.
    let waiting = Rc::new(Cell::new(false));
    let schedule = move || {
        if waiting.replace(true) {
            return;
        }
        let waiting = Rc::clone(&waiting);
        drop(request_animation_frame(move || {
            waiting.set(false);
            measure();
        }));
    };

    let scrolled = window_event_listener_untyped("scroll", {
        let schedule = schedule.clone();
        move |_| schedule()
    });
    on_cleanup(move || scrolled.remove());

    Effect::new(move |_| {
        page.file.track();
        page.view.track();
        schedule();
    });
}
