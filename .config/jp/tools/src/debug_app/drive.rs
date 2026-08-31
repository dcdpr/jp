//! `debug_app_drive` — run a step list against the running app, reporting what
//! each step changed.
//!
//! The list is [data]; this module is one harness that walks it.
//! Each step is one `jpdrive act` call, followed by a reading of the
//! accessibility tree and of the console, so a report answers three questions
//! per step: did it do the thing, what moved, and what did `AppKit` complain
//! about while it moved.
//!
//! Readings are reported as deltas against the previous one.
//! A whole tree per step in a nine-step run is most of a context window and
//! almost all of it unchanged; a delta is the handful of lines that answer
//! whether the step had the effect it was written for.
//!
//! A run stops at its first failing step.
//! The steps after it were written against a state the app never reached, so
//! running them would report on something nobody asked for.
//!
//! Each step's wall-clock window is written to [`marks`], which is what lets
//! `debug_app_profile` attribute an interval the app traced to the step that
//! caused it.
//! Nothing else records when a step ran.
//!
//! [`marks`]: super::marks
//! [data]: super::steps

use camino::Utf8Path;
use jp_tool::Outcome;
use serde_json::Value;

use crate::{
    Context, Error, Tool,
    debug_app::{
        ambient, driver,
        marks::{self, Mark},
        session::{Session, Slot},
        steps::{self, Step},
        tree,
    },
    util::{
        ToolResult,
        diff::text_diff,
        error,
        paths::{self, Shortening, shorten},
        runner::{DuctProcessRunner, ProcessRunner},
    },
};

/// Lines of context around each change in a tree delta.
///
/// Two, because a changed line in a tree means little without the elements it
/// sits between.
const DIFF_CONTEXT: usize = 2;

/// Longest tree or delta block reported for one step.
///
/// A switch to a different workspace replaces the whole tree, and a report of
/// that is thousands of lines saying one thing.
const MAX_BLOCK_LINES: usize = 200;

/// Tool entrypoint.
#[allow(clippy::unused_async, reason = "awaited by the debug_app dispatcher")]
pub(crate) async fn debug_app_drive(ctx: &Context, t: &Tool) -> ToolResult {
    let steps = steps::parse(&t.req::<Value>("steps")?)?;
    let reads = Reads::parse(t.opt::<String>("reads")?.as_deref())?;
    let opts = tree::Options {
        identifier: t.opt("identifier")?,
        max_matches: t.opt("max_matches")?,
        max_siblings: tree::DEFAULT_MAX_SIBLINGS,
        ..tree::Options::default()
    };

    if ctx.action.is_format_arguments() {
        return Ok(format_preview(&steps, &opts, reads).into());
    }

    if !cfg!(target_os = "macos") {
        return error("debug_app_drive only supports macOS: it drives an AppKit application.");
    }

    let dir = Session::dir(&ctx.root, &Slot::for_context(ctx)?);
    run(&ctx.root, &dir, &steps, &opts, reads, &DuctProcessRunner)
}

/// Render the preview shown before execution.
fn format_preview(steps: &[Step], opts: &tree::Options, reads: Reads) -> String {
    let listing = steps
        .iter()
        .enumerate()
        .map(|(index, step)| format!("{}. {}", index + 1, step.label()))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "`debug_app_drive`\n\nWill run these steps against the app recorded in \
         `tmp/debug-app/session.json`:\n\n{listing}\n\nEach step is a `jpdrive act` call, \
         followed by a reading of the accessibility tree\nand of the console. {}\n\nChanges the \
         app's state. Stops at the first failing step.\n",
        scope(opts, reads)
    )
}

/// Whether the tree is read between steps.
///
/// A reading is the evidence a driven run produces, so it is on by default.
/// It is also work the app does, on the thread it draws on, and a run that
/// measures the app has to be able to stop paying for it: an unscoped read
/// walks every element in the application, and the transcript publishes
/// elements in proportion to how much of a conversation is on screen.
/// Measuring a resize through reads charges the reads to the resize, and does
/// so in proportion to the very thing under study.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reads {
    /// Read after every step, and report what changed.
    EveryStep,

    /// Read nothing.
    /// The run reports what each step did and the console, and no tree at all.
    None,
}

impl Reads {
    fn parse(value: Option<&str>) -> Result<Self, Error> {
        match value {
            None | Some("every_step") => Ok(Self::EveryStep),
            Some("none") => Ok(Self::None),
            Some(other) => Err(format!(
                "`reads` takes `every_step` or `none`, not `{other}`. Leave it out to read after \
                 every step."
            )
            .into()),
        }
    }

    const fn is_none(self) -> bool {
        matches!(self, Self::None)
    }
}

/// One sentence naming how much of the tree each reading covers.
fn scope(opts: &tree::Options, reads: Reads) -> String {
    if reads.is_none() {
        return "The tree was not read, so no step reports what it changed. Asked for, because a \
                reading is work the app does on the thread it draws on and a run that measures \
                the app cannot afford it."
            .to_owned();
    }

    match &opts.identifier {
        Some(prefix) => format!("Readings cover the elements under `{prefix}`."),
        None => "Readings cover the whole application.".to_owned(),
    }
}

/// Walk the list, and report every step up to and including the one that
/// stopped it.
fn run(
    root: &Utf8Path,
    dir: &Utf8Path,
    steps: &[Step],
    opts: &tree::Options,
    reads: Reads,
    runner: &dyn ProcessRunner,
) -> ToolResult {
    let mut session = Session::resolve(dir)?;
    let bin = driver::locate(root, runner)?;

    // The baseline is read but not reported: it is the whole tree, and what a
    // caller is asking about is what the steps change about it.
    let mut before = if reads.is_none() {
        String::new()
    } else {
        reading(&bin, session.pid, opts, root, runner)?
    };

    // Captured before the first step, because a step that synthesizes input has
    // to bring the app forward to receive it and moves the pointer to do so. Both
    // belong to whoever is at the keyboard rather than to the app, so a run that
    // borrows them puts them back. A run of steps that reach through the
    // accessibility tree borrows nothing and captures nothing.
    let borrowed = if steps.iter().any(Step::perturbs_ambient_state) {
        Some(ambient::capture(&bin, root, runner))
    } else {
        None
    };

    let mut body = String::new();
    let mut ran = 0;
    let mut stopped = false;
    let run = marks::new_run();
    let mut marked = Vec::new();

    // Collected rather than propagated from inside the loop. Everything below is
    // owed whichever way the run went — the borrowed focus and pointer go back,
    // the console offsets record what has been reported, the marks record when
    // the steps ran — and a `?` in the loop would skip all three on exactly the
    // run that needs them most.
    let mut failure = None;

    for (index, step) in steps.iter().enumerate() {
        let began_ms = marks::now_ms();

        let step_result = (|| {
            let acted = act(&bin, session.pid, step, root, runner)?;
            let after = if reads.is_none() {
                String::new()
            } else {
                reading(&bin, session.pid, opts, root, runner)?
            };
            let out = session.stdout.delta()?;
            let err = session.stderr.delta()?;

            Ok::<_, Error>((acted, after, out, err))
        })();

        let (acted, after, out, err) = match step_result {
            Ok(read) => read,
            Err(e) => {
                failure = Some(e);
                stopped = true;
                break;
            }
        };

        marked.push(Mark {
            run: run.clone(),
            step: index + 1,
            label: step.label(),
            began_ms,
            ended_ms: marks::now_ms(),
        });

        body.push('\n');
        body.push_str(&section(
            index + 1,
            step,
            &acted,
            &before,
            &after,
            reads,
            &out,
            &err,
        ));
        before = after;

        if matches!(acted, Acted::Refused(_)) {
            stopped = true;
            break;
        }

        ran += 1;
    }

    // Restored whichever way the run went, and a failure is when it matters most:
    // a run abandoned half-way has taken focus and left the pointer somewhere the
    // person reading the failure did not put it.
    //
    // Window geometry is deliberately left as the run left it. A step that
    // resized a window did the thing it was asked to, and putting it back would
    // undo the effect under test.
    if let Some(borrowed) = borrowed {
        ambient::restore(&borrowed, &bin, root, runner);
    }

    // Both written whichever way the run went. The console offsets are the only
    // record of what has already been reported, and the marks are the only
    // record of when the steps that did run ran.
    session.store(dir)?;
    marks::append(dir, &marked)?;

    // After the housekeeping above, so a driver that died mid-run still hands
    // the desktop back before the error is reported.
    if let Some(e) = failure {
        return Err(e);
    }

    let report = format!(
        "{}{body}",
        header(
            &session,
            ran,
            steps.len(),
            opts,
            reads,
            stopped,
            &paths::shortenings(root)
        )
    );

    if stopped {
        return error(report);
    }

    Ok(Outcome::Success {
        content: format!(
            "{report}\nWhat each step cost the app: `debug_app_profile` with `mode: \"report\"`.\n"
        ),
    })
}

/// The line a report opens with.
///
/// Names no process id, so two runs of the same list against the same state
/// produce the same report.
#[allow(clippy::too_many_arguments, reason = "one header names this much")]
fn header(
    session: &Session,
    ran: usize,
    total: usize,
    opts: &tree::Options,
    reads: Reads,
    stopped: bool,
    shortenings: &[Shortening],
) -> String {
    let workspace = shorten(session.workspace.as_str(), shortenings);
    let mut header = match (stopped, total) {
        (true, _) => format!(
            "Ran {ran} of {total} steps against the app on `{workspace}`, then stopped at step {}.",
            ran + 1
        ),
        (false, 1) => format!("Ran the step against the app on `{workspace}`."),
        (false, _) => format!("Ran all {total} steps against the app on `{workspace}`."),
    };

    let remaining = total - ran.min(total) - usize::from(stopped);
    if remaining == 1 {
        header.push_str(" The remaining step was not run.");
    } else if remaining > 1 {
        header.push_str(&format!(" The remaining {remaining} steps were not run."));
    }

    format!("{header}\n\n{}\n", scope(opts, reads))
}

/// Read the tree and render it.
///
/// A prefix that matches nothing renders as one line rather than as an error.
/// A view part-way through loading holds none of the identifiers it will hold a
/// moment later, and a step list that walks through such a state is the
/// ordinary case rather than a broken one — so the delta says the elements
/// went away and the next step says they came back.
fn reading(
    bin: &Utf8Path,
    pid: u32,
    opts: &tree::Options,
    root: &Utf8Path,
    runner: &dyn ProcessRunner,
) -> Result<String, Error> {
    let Some(node) = tree::read(bin, pid, opts, root, runner)? else {
        return Ok(format!(
            "(nothing matched `{}`)\n",
            opts.identifier.as_deref().unwrap_or_default()
        ));
    };

    Ok(tree::rendered(&node, opts))
}

/// What the driver said about a step.
#[derive(Debug)]
enum Acted {
    /// The driver ran the step and reported this.
    Ran(String),

    /// Nothing was asked of the driver.
    Observed,

    /// The driver refused, and said this.
    Refused(String),
}

/// Hand one step to the driver.
///
/// A refusal comes back as [`Acted::Refused`] rather than as an error: the
/// steps before it ran, and a report of them is what says how the app got into
/// the state the failing step met.
/// `Err` is for a driver that could not be started at all.
fn act(
    bin: &Utf8Path,
    pid: u32,
    step: &Step,
    root: &Utf8Path,
    runner: &dyn ProcessRunner,
) -> Result<Acted, Error> {
    if step.is_snapshot() {
        return Ok(Acted::Observed);
    }

    let pid_arg = pid.to_string();
    let json = step.json();
    let output = runner
        .run(
            bin.as_str(),
            &["act", "--pid", &pid_arg, "--json", &json],
            root,
        )
        .map_err(|e| format!("Failed to spawn {bin}: {e}"))?;

    if !output.success() {
        return Ok(Acted::Refused(driver::describe_failure(
            "act",
            bin,
            pid,
            root,
            runner,
            &output.stdout,
            &output.stderr,
        )));
    }

    Ok(Acted::Ran(result_line(&output.stdout)))
}

/// Fields reported first, so a result line reads as what happened before it
/// reads as what was checked.
const LEADING_FIELDS: [&str; 3] = ["step", "identifier", "role"];

/// One line describing what the driver reported about a step.
///
/// Every field it reported is shown, rather than a chosen few: the driver owns
/// that document, and a mirror of it here would go quietly out of date.
fn result_line(stdout: &str) -> String {
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(stdout) else {
        return stdout.trim().to_owned();
    };

    let mut fields: Vec<String> = LEADING_FIELDS
        .iter()
        .filter_map(|key| map.get(*key).map(|value| field(key, value)))
        .collect();

    fields.extend(
        map.iter()
            .filter(|(key, _)| !LEADING_FIELDS.contains(&key.as_str()))
            .map(|(key, value)| field(key, value)),
    );

    fields.join(" ")
}

/// One `key=value` pair, with strings left unquoted.
fn field(key: &str, value: &Value) -> String {
    match value {
        Value::String(text) => format!("{key}={text}"),
        other => format!("{key}={other}"),
    }
}

/// Report one step.
#[allow(clippy::too_many_arguments, reason = "one section reports this much")]
fn section(
    position: usize,
    step: &Step,
    acted: &Acted,
    before: &str,
    after: &str,
    reads: Reads,
    out: &str,
    err: &str,
) -> String {
    let mut blocks = Vec::new();

    // Nothing about the tree, because nothing was read. Saying it did not change
    // would be reporting an observation that was never made.
    let tree = !reads.is_none();

    match acted {
        Acted::Ran(result) => {
            blocks.push(format!("{result}\n"));
            if tree {
                blocks.push(delta_block(before, after));
            }
        }
        Acted::Observed if tree => blocks.push(delta_block(before, after)),
        Acted::Observed => {}
        Acted::Refused(message) => {
            blocks.push(format!("Failed: {message}\n"));

            // The whole reading rather than a delta. A step that failed
            // usually changed nothing, and a delta of nothing does not answer
            // what the app held instead.
            if tree {
                blocks.push(format!(
                    "The tree at the failure:\n\n```\n{}```\n",
                    cap(after)
                ));
            }
        }
    }

    for (name, content) in [("stdout", out), ("stderr", err)] {
        if content.trim().is_empty() {
            continue;
        }

        blocks.push(format!(
            "Console ({name}):\n\n```\n{}\n```\n",
            content.trim_end()
        ));
    }

    format!("### {position}. {}\n\n{}", step.label(), blocks.join("\n"))
}

/// What changed between two readings.
fn delta_block(before: &str, after: &str) -> String {
    if before == after {
        return "The tree did not change.\n".to_owned();
    }

    let diff = text_diff(before, after);
    let mut unified = diff.unified_diff();
    unified.context_radius(DIFF_CONTEXT);

    format!("Tree delta:\n\n```diff\n{}```\n", cap(&unified.to_string()))
}

/// Cap a block at [`MAX_BLOCK_LINES`], naming what was left out.
///
/// The result always ends in a newline, so it sits inside a fence without
/// closing it on the same line.
fn cap(text: &str) -> String {
    let total = text.lines().count();
    if total <= MAX_BLOCK_LINES {
        return if text.ends_with('\n') {
            text.to_owned()
        } else {
            format!("{text}\n")
        };
    }

    let kept: Vec<&str> = text.lines().take(MAX_BLOCK_LINES).collect();
    format!(
        "{}\n\n[{} more lines, not shown]\n",
        kept.join("\n"),
        total - MAX_BLOCK_LINES
    )
}

#[cfg(all(test, unix))]
#[path = "drive_tests.rs"]
mod tests;
