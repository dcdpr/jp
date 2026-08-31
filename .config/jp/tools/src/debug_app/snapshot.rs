//! `debug_app_snapshot` — what the app looks like, and what it has complained
//! about.
//!
//! Two channels, because neither sees the other's failures.
//! The accessibility tree says what the interface structurally is, which is
//! what a caller acts on.
//! The console says what `AppKit` objected to, and a whole class of defect —
//! reentrancy warnings, constraint complaints, exceptions — appears there and
//! nowhere in the tree.
//!
//! Console output is reported as a delta, so a call answers "what happened
//! since I last looked" rather than replaying the run.
//!
//! The app's own trace is a third channel, summarized rather than quoted: it
//! says how long the work behind the tree took and what the process weighs,
//! which neither of the other two can.

use camino::Utf8Path;
use jp_tool::Outcome;

use crate::{
    Context, Tool,
    debug_app::{
        driver,
        session::{Session, Slot},
        trace, tree,
    },
    util::{
        ToolResult, error,
        paths::{self, Shortening, shorten},
        runner::{DuctProcessRunner, ProcessRunner},
        trace::parse_lines,
    },
};

/// Tool entrypoint.
#[allow(clippy::unused_async, reason = "awaited by the debug_app dispatcher")]
pub(crate) async fn debug_app_snapshot(ctx: &Context, t: &Tool) -> ToolResult {
    let opts = tree::Options {
        identifier: t.opt("identifier")?,
        max_matches: t.opt("max_matches")?,
        depth: t.opt("depth")?,
        max_siblings: t
            .opt::<u32>("max_siblings")?
            .unwrap_or(tree::DEFAULT_MAX_SIBLINGS),
        frames: t.opt::<bool>("frames")?.unwrap_or(false),
        actions: t.opt::<bool>("actions")?.unwrap_or(false),
        menus: t.opt::<bool>("menus")?.unwrap_or(false),
    };
    let pasteboard = t.opt::<bool>("pasteboard")?.unwrap_or(false);

    if ctx.action.is_format_arguments() {
        return Ok(format_preview(&opts, pasteboard).into());
    }

    if !cfg!(target_os = "macos") {
        return error(
            "debug_app_snapshot only supports macOS: it reads an application's accessibility tree.",
        );
    }

    let dir = Session::dir(&ctx.root, &Slot::for_context(ctx)?);
    run(&ctx.root, &dir, &opts, pasteboard, &DuctProcessRunner)
}

fn format_preview(opts: &tree::Options, pasteboard: bool) -> String {
    // The pid is only known once the session resolves, which happens after the
    // preview is rendered.
    let args = tree::args(0, opts)
        .join(" ")
        .replacen("--pid 0", "--pid <pid>", 1);

    let clipboard = if pasteboard { "\npbpaste\n" } else { "" };

    format!(
        "`debug_app_snapshot`\n\nWill execute:\n\n```sh\njust build-drive\njpdrive \
         {args}{clipboard}\n```\n\nReads the accessibility tree of the app recorded in \
         `tmp/debug-app/session.json`,\nand returns it alongside whatever the app has written to \
         its console since the\nlast call.\n\nReads only. Nothing about the app's state is \
         changed.\n"
    )
}

/// Read the tree and the console, and report both.
fn run(
    root: &Utf8Path,
    dir: &Utf8Path,
    opts: &tree::Options,
    pasteboard: bool,
    runner: &dyn ProcessRunner,
) -> ToolResult {
    let mut session = Session::resolve(dir)?;
    let bin = driver::locate(root, runner)?;

    let node = match tree::read(&bin, session.pid, opts, root, runner) {
        Ok(Some(node)) => node,
        Ok(None) => {
            return error(format!(
                "No element's identifier begins with `{}`. Drop `identifier` to see what the app \
                 reports, or check that the view holding it is on screen: a collapsed sidebar and \
                 a background tab are both absent from the tree entirely.",
                opts.identifier.as_deref().unwrap_or_default()
            ));
        }
        Err(e) => return error(e.to_string()),
    };

    let clipboard = if pasteboard {
        Some(read_pasteboard(root, runner)?)
    } else {
        None
    };

    let out = session.stdout.delta()?;
    let err = session.stderr.delta()?;

    let summary = trace::summarize(&parse_lines(&session.trace.delta()?));
    let traced = trace::render(&summary, session.reported_footprint_mb);
    if let Some(footprint) = summary.footprint_mb {
        session.reported_footprint_mb = Some(footprint);
    }

    session.store(dir)?;

    Ok(Outcome::Success {
        content: report(
            &session,
            &tree::rendered(&node, opts),
            clipboard.as_deref(),
            &out,
            &err,
            traced.as_deref(),
            &paths::shortenings(root),
        ),
    })
}

/// What the pasteboard holds.
///
/// Read through `pbpaste` rather than the driver: the pasteboard belongs to the
/// system rather than to the app, so it needs no accessibility grant and no
/// element to hang off.
fn read_pasteboard(root: &Utf8Path, runner: &dyn ProcessRunner) -> Result<String, crate::Error> {
    let output = runner
        .run("pbpaste", &[], root)
        .map_err(|e| format!("Failed to spawn `pbpaste`: {e}"))?;

    if !output.success() {
        return Err(format!("`pbpaste` failed: {}", output.stderr.trim_end()).into());
    }

    Ok(output.stdout)
}

/// Render the snapshot report.
fn report(
    session: &Session,
    tree: &str,
    pasteboard: Option<&str>,
    out: &str,
    err: &str,
    traced: Option<&str>,
    shortenings: &[Shortening],
) -> String {
    let mut report = format!(
        "Snapshot of the app (pid {}) on `{}`.\n\nAccessibility tree:\n\n```\n{tree}```\n",
        session.pid,
        shorten(session.workspace.as_str(), shortenings)
    );

    if let Some(contents) = pasteboard {
        if contents.is_empty() {
            report.push_str("\nThe pasteboard is empty.\n");
        } else {
            report.push_str(&format!(
                "\nPasteboard:\n\n```\n{}\n```\n",
                contents.trim_end()
            ));
        }
    }

    for (name, content) in [("stdout", out), ("stderr", err)] {
        if content.trim().is_empty() {
            continue;
        }

        report.push_str(&format!(
            "\nConsole ({name}), since the last call:\n\n```\n{}\n```\n",
            content.trim_end()
        ));
    }

    if out.trim().is_empty() && err.trim().is_empty() {
        report.push_str("\nNothing new on either console stream since the last call.\n");
    }

    if let Some(traced) = traced {
        report.push_str(&format!("\n{traced}\n"));
    }

    report
}

#[cfg(test)]
#[path = "snapshot_tests.rs"]
mod tests;
