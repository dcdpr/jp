//! The application's accessibility tree: how it is asked for, how it arrives,
//! and how it reads.
//!
//! [`read`] shells out to `jpdrive tree`, which answers JSON; [`render`] turns
//! that into one line per element.
//! One line per element is what makes two readings comparable — a selection
//! change moves a `[focused]` marker rather than reflowing a block — so both
//! the snapshot tool and the drive harness report through it.

use camino::Utf8Path;
use serde::Deserialize;

use crate::{Error, debug_app::driver, util::runner::ProcessRunner};

/// Sibling cap when the caller names none.
///
/// `jpdrive` defaults to five, which is enough to see the shape of a list but
/// hides the row a caller just selected.
/// Reading everything is the right default for a reading meant to be diffed
/// against another.
pub(crate) const DEFAULT_MAX_SIBLINGS: u32 = 0;

/// What to read, and how much of it.
#[derive(Debug, Clone, Default)]
pub(crate) struct Options {
    /// Keep only elements whose identifier begins with this, and the ancestors
    /// leading to them.
    pub identifier: Option<String>,

    /// How many matches to find before stopping a filtered read.
    pub max_matches: Option<u32>,

    /// How deep to walk.
    pub depth: Option<u32>,

    /// How many children to walk per level, `0` for all of them.
    pub max_siblings: u32,

    /// Include each element's on-screen frame.
    pub frames: bool,

    /// Include the actions each element advertises.
    pub actions: bool,

    /// Walk into the menu bar.
    pub menus: bool,
}

/// The `jpdrive tree` command line.
pub(crate) fn args(pid: u32, opts: &Options) -> Vec<String> {
    let mut args = vec![
        "tree".to_owned(),
        "--pid".to_owned(),
        pid.to_string(),
        "--max-siblings".to_owned(),
        opts.max_siblings.to_string(),
    ];

    if let Some(prefix) = &opts.identifier {
        args.push("--identifier".to_owned());
        args.push(prefix.clone());
    }

    if let Some(matches) = opts.max_matches {
        args.push("--max-matches".to_owned());
        args.push(matches.to_string());
    }

    if let Some(depth) = opts.depth {
        args.push("--depth".to_owned());
        args.push(depth.to_string());
    }

    if opts.frames {
        args.push("--frames".to_owned());
    }

    // Passed rather than filtered out of what comes back. Both cost the driver
    // accessibility round-trips it would otherwise make and discard: actions are
    // a call per kept element, and the menu bar is a couple of hundred elements
    // that belong to macOS rather than to the app.
    if opts.actions {
        args.push("--actions".to_owned());
    }

    if opts.menus {
        args.push("--menus".to_owned());
    }

    args
}

/// The kind the driver reports when a prefix matched nothing.
const NO_MATCH: &str = "identifier_not_found";

/// Read the tree of the application running under `pid`.
///
/// `bin` is the `jpdrive` binary, as [`driver::locate`] found it.
///
/// `None` means the identifier prefix matched nothing, which is a reading and
/// not a failure: a view part-way through loading holds none of the identifiers
/// it will hold a moment later.
/// A driver that refused for lack of an Accessibility grant carries the
/// diagnosis of that refusal in the error.
pub(crate) fn read(
    bin: &Utf8Path,
    pid: u32,
    opts: &Options,
    root: &Utf8Path,
    runner: &dyn ProcessRunner,
) -> Result<Option<TreeNode>, Error> {
    let args = args(pid, opts);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = runner
        .run(bin.as_str(), &arg_refs, root)
        .map_err(|e| format!("Failed to spawn {bin}: {e}"))?;

    // The driver writes results and errors to the same stream, distinguished by
    // the exit status.
    if !output.success() {
        if driver::kind(&output.stdout).as_deref() == Some(NO_MATCH) {
            return Ok(None);
        }

        return Err(driver::describe_failure(
            "tree",
            bin,
            pid,
            root,
            runner,
            &output.stdout,
            &output.stderr,
        )
        .into());
    }

    serde_json::from_str(&output.stdout)
        .map(Some)
        .map_err(|e| format!("Failed to parse the tree `jpdrive` reported: {e}").into())
}

/// One element, as `jpdrive tree` reports it.
#[derive(Debug, Deserialize)]
pub(crate) struct TreeNode {
    role: String,
    identifier: Option<String>,
    label: Option<String>,
    value: Option<String>,
    enabled: Option<bool>,
    focused: Option<bool>,
    frame: Option<String>,
    #[serde(default)]
    actions: Vec<String>,
    #[serde(default)]
    children: Vec<TreeNode>,
    elided_children: Option<usize>,
}

/// The whole tree, rendered.
pub(crate) fn rendered(node: &TreeNode, opts: &Options) -> String {
    let mut out = String::new();
    render(node, 0, opts, &mut out);
    out
}

/// The role whose subtree is mostly not the app's.
const MENU_BAR_ROLE: &str = "AXMenuBar";

/// Render one element and its children, one line each.
///
/// The menu bar is left unwalked unless asked for.
/// Most of what hangs off it belongs to macOS rather than to the app — the
/// Apple menu, Services, the window tiling submenus — and it runs to some two
/// hundred lines that bury the handful describing the window.
pub(crate) fn render(node: &TreeNode, depth: usize, opts: &Options, out: &mut String) {
    out.push_str(&"  ".repeat(depth));
    out.push_str(&node.role);

    if let Some(identifier) = &node.identifier {
        out.push_str(&format!(" #{identifier}"));
    }

    if let Some(label) = &node.label {
        out.push_str(&format!(" {label:?}"));
    }

    if let Some(value) = &node.value {
        out.push_str(&format!(" = {value:?}"));
    }

    if node.enabled == Some(false) {
        out.push_str(" [disabled]");
    }

    if node.focused == Some(true) {
        out.push_str(" [focused]");
    }

    if let Some(frame) = &node.frame {
        out.push_str(&format!(" @{frame}"));
    }

    if !node.actions.is_empty() {
        out.push_str(&format!(" ({})", node.actions.join(", ")));
    }

    // The driver leaves the menu bar unwalked unless asked, so its children
    // arrive as a count rather than as elements. Named here rather than left as
    // a bare elision, because it is the one elision a caller can undo.
    match node.elided_children {
        Some(elided) if node.role == MENU_BAR_ROLE && !opts.menus => out.push_str(&format!(
            " ({elided} menus not walked, pass `menus` for them)"
        )),
        Some(elided) => out.push_str(&format!(" (+{elided} not shown)")),
        None => {}
    }

    out.push('\n');

    for child in &node.children {
        render(child, depth + 1, opts, out);
    }
}

#[cfg(test)]
#[path = "tree_tests.rs"]
mod tests;
