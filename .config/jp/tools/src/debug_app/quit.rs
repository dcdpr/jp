//! `debug_app_quit` — stop the running app, keeping its state for a relaunch.
//!
//! Removes `session.json`, because that record describes a live process.
//! The state and user-data directories stay: a quit followed by a launch with
//! `fresh = false` is how state restoration gets tested at all.
//!
//! A profile bracket left open is closed first, before the app: `xctrace`
//! attached to a process that is going away has no predictable behaviour, and
//! nobody brackets a quit deliberately.
//! Closing it reports the summary `debug_app_profile` would have, and stamps
//! the record with what it was recording — the last moment at which that is
//! knowable, since the record being removed here is where it comes from.
//!
//! Retained artifacts are swept on the way out, under the age window and byte
//! budget `capture` holds.

use std::{
    thread,
    time::{Duration, Instant},
};

use camino::Utf8Path;
use jp_tool::Outcome;

use crate::{
    Context, Error, Tool,
    debug_app::{
        capture::{self, Recording, Stop, Target},
        hotspots::{self, Summary},
        profile::swept_note,
        session::{RealSignals, Session, Signal, Signals, Slot},
    },
    util::{
        ToolResult, error,
        paths::{self, Shortening, shorten},
    },
};

/// How long the app is given to exit after `SIGTERM` before it is killed.
const TERM_GRACE: Duration = Duration::from_secs(10);

/// Poll interval while waiting for the process to disappear.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How a launched app ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Termination {
    /// Already gone when the tool ran.
    Absent,

    /// Exited after `SIGTERM`.
    Terminated,

    /// Ignored `SIGTERM` and was killed.
    Killed,
}

/// Tool entrypoint.
#[allow(clippy::unused_async, reason = "awaited by the debug_app dispatcher")]
pub(crate) async fn debug_app_quit(ctx: &Context, _t: &Tool) -> ToolResult {
    if ctx.action.is_format_arguments() {
        return Ok(format_preview().into());
    }

    if !cfg!(target_os = "macos") {
        return error(
            "debug_app_quit only supports macOS: it stops an app started by `debug_app_launch`.",
        );
    }

    let dir = Session::dir(&ctx.root, &Slot::for_context(ctx));
    run(&ctx.root, &dir, TERM_GRACE, &RealSignals)
}

fn format_preview() -> String {
    "`debug_app_quit`\n\nStops the app recorded in `tmp/debug-app/session.json` with `SIGTERM`, \
     escalating to\n`SIGKILL` if it does not exit.\n\nA profile bracket left open is closed first, \
     before the app, and its summary comes\nback here. Closing a system-wide bracket takes \
     minutes, so close it yourself with\n`debug_app_profile` beforehand if that \
     matters.\n\nRemoves the session record. Keeps `tmp/debug-app/state/` and \
     `tmp/debug-app/data/`, so\na following `debug_app_launch` with `fresh = false` reopens what \
     this app had\nopen.\n\nReturns whatever the app wrote to its console since the last call.\n"
        .to_owned()
}

/// What became of a bracket left open.
struct Closed {
    id: String,
    stop: Stop,

    /// How long the recorder took to write the bundle out.
    elapsed: Duration,

    /// What the recorder said about problems with what it recorded, if
    /// anything.
    issues: String,

    /// The summary that replaced the bundle, or why there is none.
    summary: Result<Summary, String>,
}

/// Close a bracket that was left open, and destroy its bundle.
fn close(
    recording: &Recording,
    dir: &Utf8Path,
    session: &Session,
    signals: &dyn Signals,
    timeout: Duration,
    shortenings: &[Shortening],
) -> Closed {
    let (stop, elapsed) = capture::stop(recording.recorder_pid, signals, timeout);
    let said = recording.said(dir);
    let mut recording = recording.clone();

    let summary = if stop == Stop::Stuck {
        Err(format!(
            "The recorder (pid {}) had not finished writing `{}` after {}s, and was left running: \
             `xctrace` finalizes on its way out, so killing it leaves a bundle nothing can open. \
             Send it `kill -INT {}`, wait, and then delete the bundle by hand — it holds the \
             environment of every process it recorded.",
            recording.recorder_pid,
            recording.id,
            timeout.as_secs(),
            recording.recorder_pid,
        ))
    } else if recording.bundle(dir).exists() {
        // Stamped before the read: the session record is about to be removed, so
        // this is the last moment at which the recording can be told what it was
        // recording.
        let target = Target::for_session(session);
        drop(recording.close(Some(target.clone()), dir));

        // A system-wide bundle goes whatever the read did: it is credential
        // material, and a failure that leaves one behind is the case nobody is
        // watching.
        let read = hotspots::summarize(&recording, dir, Some(&target), shortenings)
            .map_err(|e| e.to_string());
        drop(recording.retire(dir));
        read
    } else {
        drop(recording.discard(dir));
        Err(format!(
            "The recorder left no bundle for `{}`. It said:\n\n```\n{}\n```",
            recording.id,
            said.trim_end()
        ))
    };

    Closed {
        id: recording.id.clone(),
        stop,
        elapsed,
        issues: if capture::run_issues(&said) {
            capture::run_issue_lines(&said)
        } else {
            String::new()
        },
        summary,
    }
}

/// Stop the recorded app and report how it went.
fn run(root: &Utf8Path, dir: &Utf8Path, grace: Duration, signals: &dyn Signals) -> ToolResult {
    let shortenings = paths::shortenings(root);

    let Some(mut session) = Session::load(dir)? else {
        return error(format!(
            "No app session recorded at {}, so there is nothing to stop.",
            Session::path(dir)
        ));
    };

    // Before the app, because a recorder attached to a process on its way out
    // has nothing useful to do and no defined behaviour.
    let closed = capture::pending(dir).map(|recording| {
        close(
            &recording,
            dir,
            &session,
            signals,
            capture::FINALIZE_TIMEOUT,
            &shortenings,
        )
    });

    let termination = if session.is_running() {
        stop(session.pid, grace, signals)?
    } else {
        Termination::Absent
    };

    // Read the console before dropping the record: these offsets are the only
    // thing that knows what has already been reported.
    let out = session.stdout.delta()?;
    let err = session.stderr.delta()?;

    let path = Session::path(dir);
    std::fs::remove_file(&path).map_err(|e| format!("Failed to remove {path}: {e}"))?;

    let swept = capture::sweep(dir);

    Ok(Outcome::Success {
        content: report(
            &session,
            termination,
            closed.as_ref(),
            &swept,
            &out,
            &err,
            &shortenings,
        ),
    })
}

/// Signal the app and wait for it to go away, escalating once.
fn stop(pid: u32, grace: Duration, signals: &dyn Signals) -> Result<Termination, Error> {
    signals.send(pid, Signal::Term);
    if wait_for_exit(pid, grace, signals) {
        return Ok(Termination::Terminated);
    }

    signals.send(pid, Signal::Kill);
    if wait_for_exit(pid, grace, signals) {
        return Ok(Termination::Killed);
    }

    Err(format!("The app (pid {pid}) is still running after SIGKILL.").into())
}

/// Poll until `pid` is gone or `timeout` elapses.
/// `true` when it is gone.
fn wait_for_exit(pid: u32, timeout: Duration, signals: &dyn Signals) -> bool {
    let deadline = Instant::now() + timeout;

    loop {
        if !signals.is_alive(pid) {
            return true;
        }

        if Instant::now() >= deadline {
            return false;
        }

        thread::sleep(POLL_INTERVAL);
    }
}

/// Render the quit report.
fn report(
    session: &Session,
    termination: Termination,
    closed: Option<&Closed>,
    swept: &[String],
    out: &str,
    err: &str,
    shortenings: &[Shortening],
) -> String {
    let mut report = match termination {
        Termination::Absent => format!(
            "The app recorded as pid {} was already gone. Cleared the session record.\n",
            session.pid
        ),
        Termination::Terminated => {
            format!("Stopped the app (pid {}) with SIGTERM.\n", session.pid)
        }
        Termination::Killed => format!(
            "The app (pid {}) ignored SIGTERM and was killed. Anything it writes only on a clean \
             exit is missing.\n",
            session.pid
        ),
    };

    report.push_str(&format!(
        "\nKept for a relaunch with `fresh = false`:\n\n- state: `{}`\n- user data: `{}`\n",
        shorten(session.state_dir.as_str(), shortenings),
        shorten(session.user_data_dir.as_str(), shortenings)
    ));

    if let Some(closed) = closed {
        report.push_str(&render_closed(closed, shortenings));
    }

    report.push_str(&swept_note(swept));

    for (name, content) in [("stdout", out), ("stderr", err)] {
        if content.trim().is_empty() {
            continue;
        }

        report.push_str(&format!(
            "\nConsole ({name}), since the last call:\n\n```\n{}\n```\n",
            content.trim_end()
        ));
    }

    report
}

/// Render what became of a bracket left open.
fn render_closed(closed: &Closed, shortenings: &[Shortening]) -> String {
    let elapsed = format!("{:.1}s", closed.elapsed.as_secs_f64());

    let mut out = match closed.stop {
        Stop::Finalized => format!(
            "\nA profile bracket was still open. Closed `{}`; the recorder finished writing its \
             bundle in {elapsed}.\n",
            closed.id
        ),
        Stop::Absent => format!(
            "\nA profile bracket was still open as `{}`, but its recorder was already gone.\n",
            closed.id
        ),
        Stop::Stuck => format!(
            "\nA profile bracket was still open as `{}`, and its recorder was still writing after \
             {elapsed}.\n",
            closed.id
        ),
    };

    if !closed.issues.is_empty() {
        out.push_str(&format!(
            "\nIt reported run issues, so parts of the trace may be missing. What it \
             said:\n\n```\n{}\n```\n",
            closed.issues
        ));
    }

    match &closed.summary {
        Ok(summary) => out.push_str(&format!(
            "\nSummary at `{}`:\n\n{}",
            shorten(summary.path.as_str(), shortenings),
            summary.content
        )),
        Err(why) => out.push_str(&format!("\nNo summary: {why}\n")),
    }

    out
}

#[cfg(test)]
#[path = "quit_tests.rs"]
mod tests;
