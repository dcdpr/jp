//! Shared helpers for `debug_jp_*` tools.
//!
//! Hosts the harness (`sandbox` + `build` + `launch`) that every tool in this
//! family composes, plus the per-tool parse/render helpers.
//! Tool orchestration lives one level up in `debug_jp/<tool>.rs`.

use camino::Utf8Path;

use crate::util::paths;

pub(crate) mod build;
pub(crate) mod launch;
pub(crate) mod profile_heap_parse;
pub(crate) mod profile_heap_render;
pub(crate) mod profile_sampling_parse;
pub(crate) mod profile_sampling_render;
pub(crate) mod sandbox;
pub(crate) mod trace_render;

/// Every absolute path in `report`, named by the variable it lives under.
///
/// Applied to the finished report rather than to each value that goes into it.
/// These reports quote a subprocess's stderr verbatim, render dhat frames
/// carrying source locations, and print trace fields naming whatever jp was
/// reading — there is no enumerating where a path can turn up, so the whole
/// text gets one pass.
/// That also covers the artifact paths in the footer, which is why nothing
/// upstream of here relativizes anything.
pub(crate) fn shorten_paths(report: &str, root: &Utf8Path) -> String {
    paths::shorten_within(report, &paths::shortenings(root))
}

/// Prepend a shutdown-warning banner to `report` when jp didn't exit on its own
/// (it was shut down or force-killed by the run timeout).
/// A naturally-exited run is returned unchanged.
pub(crate) fn with_termination_note(report: String, result: &launch::LaunchResult) -> String {
    match result.note() {
        Some(note) => format!("> [!WARNING]\n> {note}\n\n{report}"),
        None => report,
    }
}

#[cfg(all(test, unix))]
#[path = "util_tests.rs"]
mod tests;
