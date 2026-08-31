//! Assistant-callable tools for driving the macOS app.
//!
//! `debug_jp` inverted.
//! There `jp` is short-lived and a tool wraps a whole run; here the app
//! outlives the call, so the running instance *is* the session and each tool
//! attaches to it.
//! [`session`] holds that record and refuses to act on an instance that is not
//! the one which was launched.
//!
//! Tools currently exposed:
//!
//! - `debug_app_launch` — build, launch an isolated instance, record the
//!   session.
//!
//! - `debug_app_snapshot` — the accessibility tree, the console delta, a
//!   summary of what the app traced, and the pasteboard.
//!
//! - `debug_app_screenshot` — a PNG of the app's window, for what the tree
//!   cannot express.
//!
//! - `debug_app_pixels` — the colours along one row or column of that window,
//!   for the drawn things that carry neither a frame nor a colour in the tree.
//!
//! - `debug_app_drive` — run a step list, reporting what each step changed.
//!
//! - `debug_app_quit` — stop it, keeping its state for a relaunch.
//!
//! - `debug_app_profile` — open and close an Instruments recording around the
//!   operation in question, and read back what a session recorded.
//!
//! Two tiers of performance data, and only the cheaper one is unconditional.
//! The app instruments itself into `trace.jsonl` on every run, and every
//! snapshot reports those intervals and its footprint.
//! Instruments is the escalation from there: [`profile`] brackets it,
//! [`capture`] runs the recorder and bounds what is kept, and [`hotspots`]
//! reduces a bundle to a summary.
//!
//! [`report`] reads both tiers back at a scope a caller chooses — [`stream`]
//! parses the app's own intervals, [`marks`] says which driven step caused
//! them.
//! It needs no session, because `debug_app_quit` removes that record and
//! reading a run afterwards is the ordinary case.
//!
//! [`steps`] holds the action vocabulary, as data, independent of the harness
//! that walks a list.
//! [`drive`] is the harness that walks one live; a harness that runs the same
//! list under a profiler would need no second copy of the vocabulary.
//!
//! Isolation is by environment and covers the app's recent-workspace list and
//! its conversation store.
//! It does not cover window state saved by `@SceneStorage`, which is keyed by
//! bundle identifier.
//! See [`launch`] for what each variable does and why.
//!
//! The tests here are `#[cfg(all(test, unix))]`, and so are the fixtures they
//! reach for.
//! They send signals, ask whether a pid is alive, and match on `/`-separated
//! paths; on Windows `pid_is_alive` answers `true` for everything and
//! `RealSignals::send` does nothing, so those tests assert against a process
//! model that is not there.
//! Linux runs them all.

use crate::{
    Context, Tool,
    util::{ToolResult, unknown_tool},
};

pub(crate) mod ambient;
pub(crate) mod capture;
pub(crate) mod drive;
pub(crate) mod driver;
pub(crate) mod hotspots;
pub(crate) mod launch;
pub(crate) mod marks;
pub(crate) mod pixels;
pub(crate) mod profile;
pub(crate) mod quit;
pub(crate) mod report;
pub(crate) mod screenshot;
pub(crate) mod session;
pub(crate) mod snapshot;
pub(crate) mod steps;
pub(crate) mod stream;
pub(crate) mod trace;
pub(crate) mod tree;

pub async fn run(ctx: Context, t: Tool) -> ToolResult {
    match t.name.trim_start_matches("debug_app_") {
        "drive" => drive::debug_app_drive(&ctx, &t).await,
        "launch" => launch::debug_app_launch(&ctx, &t).await,
        "pixels" => pixels::debug_app_pixels(&ctx, &t).await,
        "profile" => profile::debug_app_profile(&ctx, &t).await,
        "quit" => quit::debug_app_quit(&ctx, &t).await,
        "screenshot" => screenshot::debug_app_screenshot(&ctx, &t).await,
        "snapshot" => snapshot::debug_app_snapshot(&ctx, &t).await,
        _ => unknown_tool(t),
    }
}
