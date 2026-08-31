//! `debug_app_profile` — open and close an Instruments recording.
//!
//! Profiling is a bracket, not a property of a session.
//! One session can hold several in sequence: drive the app into a state, open a
//! bracket, drive the operation in question, close it, read the summary, carry
//! on.
//! That is what keeps a report about the operation rather than about a
//! mostly-idle app.
//!
//! When the bracket opens decides what it can see, and nothing else does:
//!
//! - With a session running there is a process to attach to.
//!   The trace holds that process alone, and closing the bracket is quick.
//! - With no session there is nothing to attach to, so the recorder takes the
//!   whole machine.
//!   That is the only way to cover the app's own startup, and it costs minutes
//!   to close, because every process's samples are exported before the app's
//!   can be sifted out of them.
//!
//! There is no parameter for the choice.
//! The only case a flag could express is "a session exists but record
//! everything anyway", which buys nothing.
//!
//! Allocations are the exception to all of that.
//! The Allocations instrument refuses a target of all processes, so it exists
//! only in the attach case — and only against an app `debug_app_launch` was
//! told to keep allocation stacks for.

use camino::Utf8Path;
use jp_tool::Outcome;

use crate::{
    Context, Error, Tool,
    debug_app::{
        capture::{
            self, Recording, Scope, Spawner, Target, Tier, new_id, parse_tiers, pending,
            record_args,
        },
        hotspots,
        report::{self, Request},
        session::{RealSignals, Session, Signals, Slot},
    },
    util::{
        ToolResult, error,
        paths::{self, Shortening, shorten},
        runner::{DuctProcessRunner, ProcessRunner},
    },
};

/// The entitlement the recorder needs of a process it attaches to.
const GET_TASK_ALLOW: &str = "get-task-allow";

/// Tool entrypoint.
#[allow(clippy::unused_async, reason = "awaited by the debug_app dispatcher")]
pub(crate) async fn debug_app_profile(ctx: &Context, t: &Tool) -> ToolResult {
    let mode: String = t.req("mode")?;
    let capture: Vec<String> = t.opt("capture")?.unwrap_or_default();
    let discard = t.opt::<bool>("discard")?.unwrap_or(false);
    let request = Request::from_tool(t)?;

    match mode.as_str() {
        "start" => {
            if discard {
                return error(
                    "`discard` applies to `mode: \"stop\"`, which is where a recording is thrown \
                     away. Starting one has nothing to discard.",
                );
            }

            if let Some(problem) = report_only(&request, "start") {
                return error(problem);
            }

            let tiers = parse_tiers(&capture)?;
            if ctx.action.is_format_arguments() {
                return Ok(preview_start(&tiers).into());
            }
            guard_macos()?;

            start(
                &ctx.root,
                &Session::dir(&ctx.root, &Slot::for_context(ctx)?),
                &tiers,
                &DuctProcessRunner,
                &capture::RealSpawner,
            )
        }

        "stop" => {
            if !capture.is_empty() {
                return error(
                    "`capture` applies to `mode: \"start\"`, which is where the instruments are \
                     chosen. Stopping a recording reads back whatever it was started with.",
                );
            }

            if let Some(problem) = report_only(&request, "stop") {
                return error(problem);
            }

            if ctx.action.is_format_arguments() {
                return Ok(preview_stop(discard).into());
            }
            guard_macos()?;

            stop(
                &ctx.root,
                &Session::dir(&ctx.root, &Slot::for_context(ctx)?),
                discard,
                &RealSignals,
            )
        }

        "report" => {
            if !capture.is_empty() {
                return error(
                    "`capture` applies to `mode: \"start\"`, which is where the instruments are \
                     chosen. A report reads back what a recording already holds.",
                );
            }

            if discard {
                return error(
                    "`discard` applies to `mode: \"stop\"`. A report changes nothing and destroys \
                     nothing, so there is nothing for it to throw away.",
                );
            }

            if ctx.action.is_format_arguments() {
                return Ok(preview_report(&request).into());
            }

            report::run(
                &ctx.root,
                &Session::dir(&ctx.root, &Slot::for_context(ctx)?),
                &request,
            )
        }

        other => error(format!(
            "`mode` accepts \"start\", \"stop\" or \"report\", not {other:?}."
        )),
    }
}

/// Why report arguments do not apply to opening or closing a bracket.
fn report_only(request: &Request, mode: &str) -> Option<String> {
    if request.is_empty() {
        return None;
    }

    Some(format!(
        "{} {} `mode: \"report\"`, which reads back what a recording holds. `mode: \"{mode}\"` \
         records; it has nothing to scope. Drop {} and ask for the report as its own call.",
        request
            .named()
            .iter()
            .map(|name| format!("`{name}`"))
            .collect::<Vec<_>>()
            .join(", "),
        if request.named().len() == 1 {
            "applies to"
        } else {
            "apply to"
        },
        if request.named().len() == 1 {
            "it"
        } else {
            "them"
        },
    ))
}

fn guard_macos() -> Result<(), Error> {
    if cfg!(target_os = "macos") {
        return Ok(());
    }

    Err("debug_app_profile only supports macOS: it records with Instruments.".into())
}

fn preview_start(tiers: &[Tier]) -> String {
    let attach = record_args(
        Utf8Path::new("tmp/debug-app/<slot>/profiles/<id>.trace"),
        tiers,
        Scope::Attach(0),
    )
    .join(" ");
    let system = record_args(
        Utf8Path::new("tmp/debug-app/<slot>/profiles/<id>.trace"),
        tiers,
        Scope::System,
    )
    .join(" ");

    let retention = "\nThe bundle from an attached recording is kept, so `mode: \"report\"` can \
                     ask it further\nquestions. A system-wide one is destroyed at stop: it embeds \
                     the environment of every\nprocess it recorded.\n";

    let allocations = if tiers.contains(&Tier::Allocations) {
        "\nAllocation attribution needs `MallocStackLogging` in the app's environment, and \
         libmalloc\nreads that at process start, so it is decided at launch and cannot be added to \
         a\nrunning app. The instrument also refuses a target of all processes, so there is \
         exactly\none order that works: `debug_app_launch` with `allocation_stacks: true`, then \
         open this\nbracket against it.\n\nWhat comes back is a bundle for Instruments, not a \
         table: `xctrace export` surfaces\nnone of the Allocations instrument's data. For a \
         machine-readable number, `mode:\n\"report\"` with `view: \"allocations\"` reports the \
         footprint the app measures for itself.\n"
    } else {
        ""
    };

    format!(
        "`debug_app_profile` (start)\n\nWith a session already running, attaches to \
         it:\n\n```sh\nxcrun {attach}\n```\n\nWith no session, records the whole machine, which \
         is the only way to cover the app's\nown startup:\n\n```sh\nxcrun {system}\n```\n\nLeaves \
         a recorder running until `mode: \"stop\"`, which reads a summary out of \
         the\nbundle.\n{retention}{allocations}"
    )
}

fn preview_report(request: &Request) -> String {
    let view = request.view.as_deref().unwrap_or("timeline");

    format!(
        "`debug_app_profile` (report)\n\nReads back what this slot recorded, at `view: \
         \"{view}\"`.\n\nReads only. Nothing is captured, nothing is destroyed, and the offsets \
         `debug_app_snapshot`\nuses to report deltas are left alone — so the same question can be \
         asked again at a\ndifferent scope.\n\n`timeline`, `spans` and `views` come from the \
         app's own intervals and answer while it\nruns. `hotspots`, `callgraph` and `allocations` \
         read a finalized `.trace`, so they\nanswer for closed recordings only.\n"
    )
}

fn preview_stop(discard: bool) -> String {
    let tail = if discard {
        "Throws the recording away without reading it, which is what to do after a bracket \
         that\nwent wrong: reading a system-wide one costs minutes.\n"
    } else {
        "Reads the app's samples out of the bundle and writes a summary beside it. A \
         system-wide\nrecording takes minutes here, because every process's samples are exported \
         before\nthe app's can be sifted out of them.\n"
    };

    format!(
        "`debug_app_profile` (stop)\n\nInterrupts the recorder and waits for it to write its \
         bundle out.\n\n{tail}\nAn attached recording's bundle is kept, for `mode: \"report\"` to \
         read again. A\nsystem-wide one is destroyed: a `.trace` embeds the environment of every \
         process\nit recorded.\n"
    )
}

/// Open a bracket.
fn start(
    root: &Utf8Path,
    dir: &Utf8Path,
    tiers: &[Tier],
    runner: &dyn ProcessRunner,
    spawner: &dyn Spawner,
) -> ToolResult {
    let swept = capture::sweep(dir, &RealSignals);

    if let Some(open) = pending(dir) {
        return error(format!(
            "A recording is already open as `{}`, started by pid {}. Close it with `mode: \
             \"stop\"` before opening another.",
            open.id, open.recorder_pid
        ));
    }

    // The session decides the scope, and there is no parameter for it: with an
    // app running there is something to attach to, and without one there is
    // not.
    let session = Session::load(dir)?.filter(Session::is_running);
    let scope = match &session {
        Some(session) => Scope::Attach(session.pid),
        None => Scope::System,
    };

    if tiers.contains(&Tier::Allocations) {
        let Some(session) = &session else {
            return error(
                "Allocations cannot be recorded with no app running. The instrument refuses a \
                 target of all processes, which is the only scope available without something to \
                 attach to. Run `debug_app_launch` with `allocation_stacks: true`, then open this \
                 bracket.",
            );
        };

        if !session.allocation_stacks {
            return error(format!(
                "The app running as pid {} was not launched with `allocation_stacks`, so it has \
                 kept no allocation stacks and the Allocations instrument would find nothing. \
                 libmalloc reads `MallocStackLogging` at process start, so it cannot be added to \
                 a running app. Either record the time profile alone, or `debug_app_quit` and \
                 `debug_app_launch` again with `allocation_stacks: true`.",
                session.pid
            ));
        }
    }

    if let Some(session) = &session {
        confirm_debuggable(&session.bundle, root, runner)?;
    }

    let (id, started_unix) = new_id();
    let recording = Recording {
        id,
        tiers: tiers.to_vec(),
        scope,
        recorder_pid: 0,
        started_unix,
        stopped_unix: None,

        // Known now for an attached bracket, and only now. If this app crashes
        // mid-bracket the session stops being "running", `debug_app_launch`
        // stops refusing, and a replacement can be recorded before anything
        // closes this — at which point reading the session would attribute this
        // trace to another app's pid, binary and load address.
        //
        // A system-wide bracket has nothing to name yet, by construction: it is
        // opened precisely because no app is running. That one is filled in at
        // stop.
        target: session.as_ref().map(Target::for_session),
    };

    let bundle = recording.bundle(dir);
    let log = recording.log(dir);
    let recorder_pid = spawner.start(
        &record_args(&bundle, tiers, scope),
        &log,
        root,
        capture::READY_TIMEOUT,
    )?;

    let recording = Recording {
        recorder_pid,
        ..recording
    };

    // A record that failed to land would leave a recorder nothing can find and
    // a bundle nothing will collect.
    if let Err(e) = recording.store(dir) {
        capture::stop(recorder_pid, &RealSignals, capture::FINALIZE_TIMEOUT);
        drop(recording.discard(dir));
        return Err(e);
    }

    Ok(Outcome::Success {
        content: report_start(&recording, session.as_ref(), &swept),
    })
}

/// Close a bracket.
fn stop(root: &Utf8Path, dir: &Utf8Path, discard: bool, signals: &dyn Signals) -> ToolResult {
    let Some(mut recording) = pending(dir) else {
        return error(format!(
            "No recording is open in {}. Open one with `mode: \"start\"`.",
            capture::profiles_dir(dir)
        ));
    };

    let (outcome, elapsed) =
        capture::stop(recording.recorder_pid, signals, capture::FINALIZE_TIMEOUT);

    if outcome == capture::Stop::Stuck {
        return error(format!(
            "The recorder (pid {}) had not finished writing `{}` after {}s, and was left running: \
             `xctrace` finalizes on its way out, so killing it leaves a bundle nothing can open. \
             Give it longer and stop again, or send it `kill -INT {}` and delete the bundle by \
             hand once it has gone — it holds the environment of every process it recorded.",
            recording.recorder_pid,
            recording.id,
            capture::FINALIZE_TIMEOUT.as_secs(),
            recording.recorder_pid,
        ));
    }

    let said = recording.said(dir);

    if discard {
        recording.discard(dir)?;

        return Ok(Outcome::Success {
            content: format!(
                "Discarded `{}` after {elapsed:.1?}, unread. The bundle is gone.\n",
                recording.id
            ),
        });
    }

    if !recording.bundle(dir).exists() {
        recording.discard(dir)?;

        return error(format!(
            "The recorder left no bundle for `{}`. It said:\n\n```\n{}\n```",
            recording.id,
            said.trim_end()
        ));
    }

    // Stamped and stored before the read, for two reasons. A read that failed
    // must not leave a bracket that looks open, and a retained recording has to
    // answer for itself once `debug_app_quit` has removed the session record.
    //
    // An attached bracket recorded its target when it opened, and that is the
    // app it recorded whatever is running now. Only a bracket that had nothing
    // to name then looks for one here.
    let target = match recording.target.clone() {
        Some(target) => Some(target),
        None => Session::load(dir)?.as_ref().map(Target::for_session),
    };
    recording.close(target.clone(), dir)?;

    // A system-wide bundle goes whatever the read did: it is credential
    // material, and a failure that leaves one behind is the case nobody is
    // watching. An attach bundle holds this app's environment alone and stays.
    let shortenings = paths::shortenings(root);
    let summary = hotspots::summarize(&recording, dir, target.as_ref(), &shortenings);
    recording.retire(dir)?;
    let summary = summary?;

    Ok(Outcome::Success {
        content: report_stop(&recording, &summary, outcome, elapsed, &said, &shortenings),
    })
}

/// Confirm the app can be attached to.
///
/// Sampling a process is `task_for_pid`, which wants `get-task-allow`.
/// Worth confirming rather than assuming, because staging replaces the
/// signature the build produced: rewriting `Info.plist` invalidates it, and the
/// ad-hoc re-sign that follows is what the entitlement has to survive.
fn confirm_debuggable(
    bundle: &Utf8Path,
    root: &Utf8Path,
    runner: &dyn ProcessRunner,
) -> Result<(), Error> {
    let output = runner
        .run(
            "codesign",
            &["-d", "--entitlements", "-", "--xml", bundle.as_str()],
            root,
        )
        .map_err(|e| format!("Failed to spawn `codesign`: {e}"))?;

    // Both streams, because codesign puts the entitlements on one and its
    // commentary on the other, and which is which has moved between releases.
    let reported = format!("{}{}", output.stdout, output.stderr);
    if reported.contains(GET_TASK_ALLOW) {
        return Ok(());
    }

    Err(format!(
        "The app at {bundle} does not carry `{GET_TASK_ALLOW}`, so the recorder cannot attach to \
         it. `codesign` reported:\n\n```\n{}\n```",
        reported.trim_end()
    )
    .into())
}

fn report_start(recording: &Recording, session: Option<&Session>, swept: &[String]) -> String {
    let mut out = format!(
        "Opened `{}`, recording {}.\n",
        recording.id,
        recording.describe()
    );

    match session {
        Some(session) => out.push_str(&format!(
            "\nAttached to the app (pid {}), so the trace holds that process alone and closing \
             this bracket is quick.\n",
            session.pid
        )),
        None => out.push_str(
            "\nNo app was running, so this records **every process on the machine** — the only \
             way to cover an app's own startup. Closing the bracket will take minutes, because \
             every process's samples are exported before the app's can be sifted out of them. \
             `debug_app_launch` now to put an app inside the recording.\n",
        ),
    }

    if recording.holds(Tier::Allocations) {
        out.push_str(
            "\n**Timings in this bracket are distorted.** `MallocStackLogging` costs 2x to 10x \
             and not evenly: allocation-heavy paths slow disproportionately, so one operation \
             looking 3x another may mean only that it allocates more.\n",
        );
        out.push_str(
            "\n**Nothing here will be machine-readable.** The Allocations instrument writes to \
             the trace event store rather than to a table, and `xctrace export` surfaces none of \
             it, so the stacks are reachable only by opening the bundle in Instruments. For a \
             number an agent can act on, `view: \"allocations\"` reports the footprint the app \
             measures for itself — on every run, at no cost, with no bracket at all.\n",
        );
    }

    out.push_str("\nClose it with `mode: \"stop\"`, which reads a summary out of the bundle.\n");

    if recording.keeps_bundle() {
        out.push_str(
            "The bundle is then kept, so `mode: \"report\"` can ask it further questions.\n",
        );
    } else {
        out.push_str(
            "The bundle is then destroyed: recorded system-wide, it embeds the environment of \
             every process on the machine.\n",
        );
    }

    out.push_str(&swept_note(swept));
    out
}

fn report_stop(
    recording: &Recording,
    summary: &hotspots::Summary,
    outcome: capture::Stop,
    elapsed: std::time::Duration,
    said: &str,
    shortenings: &[Shortening],
) -> String {
    let mut out = match outcome {
        capture::Stop::Absent => format!(
            "Closed `{}`. The recorder had already exited on its own, so it stopped recording at \
             some point before this.\n",
            recording.id
        ),
        _ => format!(
            "Closed `{}`. The recorder finished writing its bundle in {elapsed:.1?}.\n",
            recording.id
        ),
    };

    if capture::run_issues(said) {
        out.push_str(&format!(
            "\nIt reported run issues, so parts of the trace may be missing. What it \
             said:\n\n```\n{}\n```\n",
            capture::run_issue_lines(said)
        ));
    }

    out.push_str(&format!(
        "\nSummary at `{}`:\n\n{}",
        shorten(summary.path.as_str(), shortenings),
        summary.content
    ));
    out
}

/// What a sweep reclaimed, when it reclaimed anything.
pub(crate) fn swept_note(swept: &[String]) -> String {
    if swept.is_empty() {
        return String::new();
    }

    format!(
        "\nAlso reclaimed {} earlier artifact(s): {}. A system-wide bundle goes as soon as it has \
         been read, and everything kept beyond that is bounded by age and by a byte budget.\n",
        swept.len(),
        swept.join(", ")
    )
}

#[cfg(test)]
#[path = "profile_tests.rs"]
mod tests;
