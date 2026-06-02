//! Shared helpers for `debug_jp_*` tools.
//!
//! Hosts the harness (`sandbox` + `build` + `launch`) that every tool in this
//! family composes, plus the per-tool parse/render helpers.
//! Tool orchestration lives one level up in `debug_jp/<tool>.rs`.

use camino::Utf8Path;

pub(crate) mod build;
pub(crate) mod launch;
pub(crate) mod profile_heap_parse;
pub(crate) mod profile_heap_render;
pub(crate) mod profile_sampling_parse;
pub(crate) mod profile_sampling_render;
pub(crate) mod sandbox;
pub(crate) mod trace_parse;
pub(crate) mod trace_render;

/// Render `path` relative to `root` when it lives under it; otherwise return it
/// as-is.
///
/// Used to keep workspace-internal absolute paths out of the reports the tools
/// attach to a conversation — a report showing `tmp/profiling/trace-N.jsonl`
/// reads cleanly regardless of where the workspace lives on disk.
pub(crate) fn relative_to(root: &Utf8Path, path: &Utf8Path) -> String {
    path.strip_prefix(root)
        .map_or_else(|_| path.to_string(), Utf8Path::to_string)
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

#[cfg(test)]
#[path = "util_tests.rs"]
mod tests;
