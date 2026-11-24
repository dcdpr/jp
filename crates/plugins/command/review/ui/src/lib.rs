//! The browser UI of `jp review serve`, rendered on the client with Leptos.
//!
//! The crate uses Leptos's renderer (`tachys`) and reactivity
//! (`reactive_graph`) directly, without the `leptos` crate, and [`start`]
//! mounts the UI itself.
//!
//! `/` lists the reviews; `/reviews/{id}` shows a review, a file of it with the
//! comments on it, and what changed in that file between versions.
//! The server sends the page a shell that loads this module, which mounts
//! [`start`] and talks to the server's JSON API, whose types it shares through
//! `jp-review-api`.
//!
//! - `route`, `blocks`, `mapping`, `selection`, `layout`, `actions`,
//!   `composer`, `poll`, `text`, and the URLs, calls, and answers of `client`
//!   are pure, and tested on the host.
//! - `client`, `dom`, `highlight`, `capture`, and the views (`app`, `list`,
//!   `review`, `card`, `compose`) touch the browser: `highlight` through the
//!   CSS Custom Highlight API, `capture` through the selection API, and the
//!   browser tests in `tests/` drive both in a headless browser.
//!
//! See `docs/rfd/140-browser-frontend-for-the-review-engine.md`.

pub mod actions;
mod app;
pub mod blocks;
pub mod capture;
mod card;
pub mod client;
mod compose;
pub mod composer;
mod dom;
pub mod highlight;
pub mod layout;
mod list;
pub mod mapping;
pub mod poll;
mod review;
pub mod route;
pub mod selection;
pub mod text;

use std::mem;

use any_spawner::Executor;
use reactive_graph::owner::Owner;
use tachys::{
    dom::body,
    view::{Mountable, Render},
};
use wasm_bindgen::prelude::wasm_bindgen;

/// Mount the UI on the page's `<body>`, which runs as soon as the module is
/// loaded.
///
/// Effects and spawned tasks run on the browser's event loop.
/// The UI stays mounted for as long as the page shows.
#[wasm_bindgen(start)]
pub fn start() {
    // `Err` means an executor is set up already, and the UI uses that one.
    let _ = Executor::init_wasm_bindgen();

    // Every signal and effect the UI creates belongs to `owner`, and dropping
    // it would dispose of them; dropping `mounted` would remove the UI.
    let owner = Owner::new();
    let mounted = owner.with(|| {
        let mut mounted = app::app().build();
        mounted.mount(&body(), None);
        mounted
    });
    mem::forget((owner, mounted));
}
