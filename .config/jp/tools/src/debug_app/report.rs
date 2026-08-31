//! Reading back what a driven session recorded about its own performance.
//!
//! Read only, and idempotent: nothing here captures, stops, or advances an
//! offset, so the same question can be asked repeatedly at different scopes
//! against the same recording.
//! In particular the app's stream is read directly rather than through the
//! session, whose offset on it is what `debug_app_snapshot` uses to report
//! deltas.
//!
//! Two tiers answer two different questions, and the views split along that
//! line.
//!
//! [`timeline`], [`spans`] and [`views`] come from the app's own intervals,
//! which exist on every run and are readable while the app is still running.
//! They lead on counts, because a count is deterministic for the same steps and
//! a millisecond is not: "148 view bodies where step 1 had 26" is something to
//! assert, write a regression test against, and verify a fix by.
//!
//! [`hotspots`], [`callgraph`] and [`allocations`] come from a finalized
//! `.trace`, so they answer for closed recordings only.
//! An open bracket has no readable bundle, and a report says that rather than
//! showing an empty table.
//!
//! Nothing here needs a session.
//! `debug_app_quit` removes that record, and every path through this module
//! answers from the slot directory alone — which is the ordinary case, not an
//! edge.
//!
//! [`allocations`]: View::Allocations
//! [`callgraph`]: View::Callgraph
//! [`hotspots`]: View::Hotspots
//! [`spans`]: View::Spans
//! [`timeline`]: View::Timeline
//! [`views`]: View::Views

use camino::Utf8Path;
use chrono::{DateTime, Utc};
use jp_tool::Outcome;
use xct2cli::{
    Pid, TraceBundle,
    analysis::{CallgraphBuilder, CallgraphReport},
    trace::Toc,
};

use crate::{
    Error, Tool,
    debug_app::{
        capture::{self, Recording, Target, Tier, unix_millis},
        hotspots,
        marks::{self, Mark},
        session::{Session, state_dir},
        stream::{self, Counts, Interval, Tally},
    },
    util::{
        ToolResult, error,
        paths::{self, Shortening, shorten_within},
    },
};

/// How many rows any one table shows.
const MAX_ROWS: usize = 60;

/// How many named frames a bundle-backed view shows when the caller names no
/// count.
const DEFAULT_TOP: usize = 25;

/// How many of the busiest program counters to symbolicate before choosing
/// which to show.
///
/// Far more than any view shows, because most of what an app is doing on-CPU is
/// inside the dyld shared cache, which a trace carries no symbols for.
const EXAMINED_PCS: usize = 500;

/// Which question a report answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum View {
    /// Per-step counts, from the app's own intervals.
    Timeline,

    /// Every named interval, by how often it ran.
    Spans,

    /// The view bodies alone.
    Views,

    /// The busiest program counters, from a finalized bundle.
    Hotspots,

    /// Top functions, or the callees of one, from a finalized bundle.
    Callgraph,

    /// What the process occupied, and what an allocations bundle holds.
    Allocations,
}

impl View {
    /// The name a caller writes and a report prints.
    const fn label(self) -> &'static str {
        match self {
            View::Timeline => "timeline",
            View::Spans => "spans",
            View::Views => "views",
            View::Hotspots => "hotspots",
            View::Callgraph => "callgraph",
            View::Allocations => "allocations",
        }
    }

    /// Whether this view reads a finalized `.trace` rather than the app's own
    /// stream.
    const fn needs_bundle(self) -> bool {
        matches!(self, View::Hotspots | View::Callgraph | View::Allocations)
    }

    fn parse(name: &str) -> Result<View, Error> {
        match name {
            "timeline" => Ok(View::Timeline),
            "spans" => Ok(View::Spans),
            "views" => Ok(View::Views),
            "hotspots" => Ok(View::Hotspots),
            "callgraph" => Ok(View::Callgraph),
            "allocations" => Ok(View::Allocations),
            other => Err(format!(
                "`view` accepts \"timeline\", \"spans\", \"views\", \"hotspots\", \"callgraph\" \
                 or \"allocations\", not {other:?}."
            )
            .into()),
        }
    }
}

/// Everything a report was asked to narrow itself to.
#[derive(Debug, Clone, Default)]
pub(crate) struct Request {
    pub view: Option<String>,
    pub recording: Option<String>,
    pub against: Option<String>,
    pub step: Option<usize>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub span: Option<String>,
    pub function: Option<String>,
    pub top: Option<usize>,
}

impl Request {
    /// Read the arguments a `mode: "report"` call carries.
    pub(crate) fn from_tool(t: &Tool) -> Result<Request, Error> {
        Ok(Request {
            view: t.opt("view")?,
            recording: t.opt("recording")?,
            against: t.opt("against")?,
            step: t.opt("step")?,
            since: t.opt("since")?,
            until: t.opt("until")?,
            span: t.opt("span")?,
            function: t.opt("function")?,
            top: t.opt("top")?,
        })
    }

    /// Whether any of these arguments were given.
    ///
    /// What `mode: "start"` and `mode: "stop"` check, so a caller who scoped a
    /// report and asked to open a bracket is told rather than quietly given a
    /// recording they did not want.
    pub(crate) fn is_empty(&self) -> bool {
        self.view.is_none()
            && self.recording.is_none()
            && self.against.is_none()
            && self.step.is_none()
            && self.since.is_none()
            && self.until.is_none()
            && self.span.is_none()
            && self.function.is_none()
            && self.top.is_none()
    }

    /// The names that were given, for an error that says which to drop.
    pub(crate) fn named(&self) -> Vec<&'static str> {
        [
            ("view", self.view.is_some()),
            ("recording", self.recording.is_some()),
            ("against", self.against.is_some()),
            ("step", self.step.is_some()),
            ("since", self.since.is_some()),
            ("until", self.until.is_some()),
            ("span", self.span.is_some()),
            ("function", self.function.is_some()),
            ("top", self.top.is_some()),
        ]
        .into_iter()
        .filter_map(|(name, given)| given.then_some(name))
        .collect()
    }
}

/// A window on the timeline, in milliseconds since the epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Window {
    from_ms: u64,
    to_ms: u64,
}

impl Window {
    /// Everything a slot has ever held.
    const fn everything(now_ms: u64) -> Window {
        Window {
            from_ms: 0,
            to_ms: now_ms,
        }
    }

    /// The overlap of two windows, which is how scoping composes.
    fn narrowed(self, other: Window) -> Window {
        Window {
            from_ms: self.from_ms.max(other.from_ms),
            to_ms: self.to_ms.min(other.to_ms),
        }
    }

    const fn holds(self, at_ms: u64) -> bool {
        at_ms >= self.from_ms && at_ms <= self.to_ms
    }
}

/// What a recording occupied on the timeline.
///
/// An open bracket runs to now, which is what makes the stream-backed views
/// answer for it while it is still recording.
fn window_of(recording: &Recording, now_ms: u64) -> Window {
    Window {
        from_ms: recording.started_unix.saturating_mul(1000),
        to_ms: recording
            .stopped_unix
            .map_or(now_ms, |stopped| stopped.saturating_mul(1000) + 999),
    }
}

/// Render a report.
///
/// Every way this can go wrong is caller-correctable — a view that does not
/// exist, a recording that does not, a tier a recording lacks — so all of them
/// come back as a tool error carrying what to run next, and all of them go
/// through the same path shortening a successful report does.
pub(crate) fn run(root: &Utf8Path, dir: &Utf8Path, request: &Request) -> ToolResult {
    let shortenings = paths::shortenings(root);

    match render(dir, request, &shortenings) {
        Ok(report) => Ok(Outcome::Success {
            content: shorten_within(&report, &shortenings),
        }),
        Err(e) => error(shorten_within(&e.to_string(), &shortenings)),
    }
}

/// Assemble the report a request asks for.
fn render(dir: &Utf8Path, request: &Request, shortenings: &[Shortening]) -> Result<String, Error> {
    let view = match &request.view {
        Some(name) => View::parse(name)?,
        None => View::Timeline,
    };

    if let Some(problem) = misapplied(view, request) {
        return Err(problem.into());
    }

    let recordings = capture::recordings(dir);
    let now_ms = unix_millis();

    let named = match &request.recording {
        None => None,
        Some(id) => Some(resolve(&recordings, id, "recording")?),
    };
    let compared = match &request.against {
        None => None,
        Some(id) => Some(resolve(&recordings, id, "against")?),
    };

    let body = if view.needs_bundle() {
        let recording = match named {
            Some(recording) => recording,
            None => sole_closed(&recordings, dir, view)?,
        };

        from_bundle(view, &recording, dir, request, shortenings)?
    } else {
        from_stream(
            view,
            dir,
            request,
            named.as_ref(),
            compared.as_ref(),
            now_ms,
        )?
    };

    Ok(format!(
        "{body}{}{}",
        held(&recordings, dir),
        next(view, request, &recordings, dir)
    ))
}

/// Why a set of arguments does not apply to a view.
///
/// Refused loudly rather than ignored: an argument that was silently dropped
/// leaves a caller believing they scoped something.
fn misapplied(view: View, request: &Request) -> Option<String> {
    if view.needs_bundle() {
        if request.step.is_some() {
            return Some(format!(
                "`step` scopes the app's own intervals, which `view: \"{}\"` does not read. A \
                 bundle holds samples with no notion of which step caused them. Use `view: \
                 \"timeline\"` or `view: \"views\"` for per-step counts.",
                view.label()
            ));
        }

        if request.against.is_some() {
            return Some(format!(
                "`against` compares counts, and `view: \"{}\"` reports sample counts — which are \
                 time, and so noisy between runs. Comparing two of them chases ghosts. Compare \
                 `view: \"timeline\"`, `view: \"spans\"` or `view: \"views\"` instead, which \
                 count work the app did rather than moments a sampler caught it.",
                view.label()
            ));
        }

        if request.span.is_some() {
            return Some(format!(
                "`span` names an interval the app timed, which `view: \"{}\"` does not read. Use \
                 `function` to narrow a bundle-backed view to a symbol.",
                view.label()
            ));
        }
    } else {
        if request.function.is_some() {
            return Some(format!(
                "`function` names a symbol in the app's binary, which `view: \"{}\"` does not \
                 read. Use `span` to narrow to an interval the app timed, or `view: \"hotspots\"` \
                 to narrow to a symbol.",
                view.label()
            ));
        }

        if request.top.is_some() {
            return Some(format!(
                "`top` bounds a bundle-backed table, and `view: \"{}\"` shows every interval it \
                 found. Narrow it with `span`, `step`, or a time window instead.",
                view.label()
            ));
        }
    }

    None
}

/// The recording an id names.
fn resolve(recordings: &[Recording], id: &str, argument: &str) -> Result<Recording, Error> {
    // A suffix match, because a report abbreviates an id and the abbreviation is
    // what gets pasted back into the next call.
    let matched: Vec<&Recording> = recordings
        .iter()
        .filter(|recording| recording.id == id || recording.id.ends_with(id))
        .collect();

    match matched.as_slice() {
        [recording] => Ok((*recording).clone()),
        [] if recordings.is_empty() => Err(format!(
            "`{argument}` names `{id}`, and this slot holds no recordings at all. Open one with \
             `mode: \"start\"`, drive the operation, and close it with `mode: \"stop\"`."
        )
        .into()),
        [] => Err(format!(
            "`{argument}` names `{id}`, which no recording in this slot matches. It holds: {}.",
            ids(recordings)
        )
        .into()),
        many => Err(format!(
            "`{argument}` names `{id}`, which {} recordings match: {}. Name more of the id.",
            many.len(),
            many.iter()
                .map(|recording| format!("`{}`", recording.id))
                .collect::<Vec<_>>()
                .join(", ")
        )
        .into()),
    }
}

/// The one closed recording a bundle-backed view can answer from, when the
/// caller named none.
fn sole_closed(recordings: &[Recording], dir: &Utf8Path, view: View) -> Result<Recording, Error> {
    let closed: Vec<&Recording> = recordings
        .iter()
        .filter(|recording| !recording.is_pending(dir) && recording.bundle(dir).exists())
        .collect();

    match closed.as_slice() {
        [recording] => Ok((*recording).clone()),
        [] => Err(no_readable_bundle(recordings, dir, view)),
        many => Err(format!(
            "`view: \"{}\"` reads one recording, and this slot holds {} with a readable bundle: \
             {}. Name one with `recording`.",
            view.label(),
            many.len(),
            many.iter()
                .map(|recording| format!("`{}`", recording.id))
                .collect::<Vec<_>>()
                .join(", ")
        )
        .into()),
    }
}

/// Why there is no bundle to read, and what to run instead.
fn no_readable_bundle(recordings: &[Recording], dir: &Utf8Path, view: View) -> Error {
    let open: Vec<&Recording> = recordings
        .iter()
        .filter(|recording| recording.is_pending(dir))
        .collect();

    if let [recording] = open.as_slice() {
        return format!(
            "`view: \"{}\"` reads a finalized `.trace`, and the only recording in this slot \
             (`{}`) is still open. Close it with `mode: \"stop\"` first. Until then, `view: \
             \"timeline\"`, `view: \"spans\"` and `view: \"views\"` answer from the app's own \
             intervals and work while it records.",
            view.label(),
            recording.id
        )
        .into();
    }

    if recordings.is_empty() {
        return format!(
            "`view: \"{}\"` reads a finalized `.trace`, and this slot holds no recordings. Open \
             one with `mode: \"start\"`, drive the operation in question, and close it with \
             `mode: \"stop\"`.",
            view.label()
        )
        .into();
    }

    format!(
        "`view: \"{}\"` reads a finalized `.trace`, and none of this slot's recordings ({}) has a \
         readable bundle. A recording made with no app running covers every process on the \
         machine, so its bundle embeds every process's environment and is destroyed as soon as it \
         is read — only the summary survives. Open the next bracket against a running app.",
        view.label(),
        ids(recordings)
    )
    .into()
}

/// Every recording's id, as a phrase.
fn ids(recordings: &[Recording]) -> String {
    recordings
        .iter()
        .map(|recording| format!("`{}`", recording.id))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Answer from the app's own intervals.
fn from_stream(
    view: View,
    dir: &Utf8Path,
    request: &Request,
    named: Option<&Recording>,
    compared: Option<&Recording>,
    now_ms: u64,
) -> Result<String, Error> {
    if !stream::is_present(dir) {
        return Err(stream::missing(dir));
    }

    let intervals = stream::load(dir);
    let all_marks = marks::load(dir);
    let window = scoped(request, named, &all_marks, now_ms)?;

    let selected = select(&intervals, window, view, request.span.as_deref());
    let steps = steps_in(&all_marks, named, window, request.step);

    let Some(compared) = compared else {
        return Ok(match view {
            View::Timeline => render_timelines(&selected, &steps, window, dir),
            View::Spans | View::Views => render_tally(view, &selected, request.span.as_deref()),
            _ => unreachable!("bundle-backed views do not reach the stream"),
        });
    };

    let against_window = window_of(compared, now_ms);
    let against_selected = select(&intervals, against_window, view, request.span.as_deref());
    let against_steps = steps_in(&all_marks, Some(compared), against_window, request.step);

    Ok(match view {
        View::Timeline => compare_timeline(
            (&selected, &steps),
            (&against_selected, &against_steps),
            compared,
        ),
        View::Spans | View::Views => {
            compare_tally(view, &selected, &against_selected, compared, dir)
        }
        _ => unreachable!("bundle-backed views do not reach the stream"),
    })
}

/// The window a request narrows to.
fn scoped(
    request: &Request,
    named: Option<&Recording>,
    all_marks: &[Mark],
    now_ms: u64,
) -> Result<Window, Error> {
    let mut window = Window::everything(now_ms);

    if let Some(recording) = named {
        window = window.narrowed(window_of(recording, now_ms));
    }

    if let Some(step) = request.step {
        let run = run_marks(all_marks, named, window);
        let Some(mark) = run.iter().find(|mark| mark.step == step) else {
            return Err(no_such_step(step, &run).into());
        };

        window = window.narrowed(Window {
            from_ms: mark.began_ms,
            to_ms: mark.ended_ms,
        });
    }

    if let Some(since) = &request.since {
        window = window.narrowed(Window {
            from_ms: instant(since, "since", now_ms)?,
            to_ms: now_ms,
        });
    }
    if let Some(until) = &request.until {
        window = window.narrowed(Window {
            from_ms: 0,
            to_ms: instant(until, "until", now_ms)?,
        });
    }

    Ok(window)
}

/// Why a step number names nothing.
fn no_such_step(step: usize, run: &[Mark]) -> String {
    if run.is_empty() {
        return format!(
            "`step: {step}` names a driven step, and nothing has been driven in this slot. Run \
             `debug_app_drive`, which records when each step ran, and then ask again."
        );
    }

    format!(
        "`step: {step}` names nothing in `{}`, which has {}: {}.",
        run[0].run,
        run.len(),
        run.iter()
            .map(|mark| mark.step.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// A moment, as either a duration back from now or an absolute timestamp.
fn instant(raw: &str, argument: &str, now_ms: u64) -> Result<u64, Error> {
    if let Some(back_ms) = duration_ms(raw) {
        return Ok(now_ms.saturating_sub(back_ms));
    }

    if let Ok(parsed) = DateTime::parse_from_rfc3339(raw) {
        return u64::try_from(parsed.timestamp_millis())
            .map_err(|_| format!("`{argument}` names {raw:?}, which is before the epoch.").into());
    }

    Err(format!(
        "`{argument}` accepts a duration back from now (`30s`, `5m`, `2h`) or an RFC 3339 \
         timestamp (`{}`), not {raw:?}.",
        Utc::now().to_rfc3339()
    )
    .into())
}

/// A duration like `30s`, in milliseconds.
fn duration_ms(raw: &str) -> Option<u64> {
    let (digits, unit) = raw.split_at(raw.len().checked_sub(1)?);
    let count: u64 = digits.parse().ok()?;

    let multiplier = match unit {
        "s" => 1_000,
        "m" => 60 * 1_000,
        "h" => 60 * 60 * 1_000,
        _ => return None,
    };

    Some(count.saturating_mul(multiplier))
}

/// The intervals a view reads, inside `window`.
fn select<'a>(
    intervals: &'a [Interval],
    window: Window,
    view: View,
    span: Option<&str>,
) -> Vec<&'a Interval> {
    intervals
        .iter()
        .filter(|interval| window.holds(interval.started_ms))
        .filter(|interval| view != View::Views || interval.is_view_body())
        .filter(|interval| span.is_none_or(|name| interval.name.contains(name)))
        .collect()
}

/// The marks belonging to the run a scope selects.
///
/// A named recording picks the run it overlaps; otherwise the most recent run,
/// which is what a caller asking about "the drive I just did" means.
fn run_marks(all_marks: &[Mark], named: Option<&Recording>, window: Window) -> Vec<Mark> {
    let Some(_) = named else {
        return marks::latest_run(all_marks);
    };

    marks::overlapping(all_marks, window.from_ms, window.to_ms)
}

/// The steps a report shows rows for.
fn steps_in(
    all_marks: &[Mark],
    named: Option<&Recording>,
    window: Window,
    step: Option<usize>,
) -> Vec<Mark> {
    let run = run_marks(all_marks, named, window);

    match step {
        None => run
            .into_iter()
            .filter(|mark| mark.began_ms <= window.to_ms && mark.ended_ms >= window.from_ms)
            .collect(),
        Some(step) => run.into_iter().filter(|mark| mark.step == step).collect(),
    }
}

/// Split marks into runs, oldest run first, each in step order.
///
/// A recording's window can hold several drives — a session is meant to hold
/// several, and a bracket left open across two of them is ordinary.
/// Rendering them as one table repeats step numbers and labels the lot with
/// whichever run happened to come first, so each is kept whole instead.
fn by_run(marks: &[Mark]) -> Vec<Vec<Mark>> {
    let mut runs: Vec<Vec<Mark>> = Vec::new();

    for mark in marks {
        match runs.iter_mut().find(|run| run[0].run == mark.run) {
            Some(run) => run.push(mark.clone()),
            None => runs.push(vec![mark.clone()]),
        }
    }

    for run in &mut runs {
        run.sort_by_key(|mark| mark.step);
    }

    runs.sort_by_key(|run| run[0].began_ms);
    runs
}

/// Render one table per driven run in the window.
fn render_timelines(
    selected: &[&Interval],
    steps: &[Mark],
    window: Window,
    dir: &Utf8Path,
) -> String {
    let runs = by_run(steps);

    if runs.len() < 2 {
        return render_timeline(selected, steps, window, dir);
    }

    let mut out = format!(
        "{} driven runs fall in this window, kept apart because a step number means something \
         different in each.\n\n",
        runs.len()
    );

    out.push_str(
        &runs
            .iter()
            .map(|run| render_timeline(selected, run, window, dir))
            .collect::<Vec<_>>()
            .join("\n"),
    );

    out
}

/// Render the per-step table.
fn render_timeline(
    selected: &[&Interval],
    steps: &[Mark],
    window: Window,
    dir: &Utf8Path,
) -> String {
    if steps.is_empty() {
        let counts = stream::count(selected);

        return format!(
            "No driven steps fall in this window, so there is nothing to attribute per step. The \
             window holds {} {}: {} traced, {} view {}, {} FFI {}.\n\nOnly `debug_app_drive` \
             records when a step ran, and its record lives at `{}`. A window covering work done \
             by hand — clicking the app, or its own launch — has no steps in it by \
             construction.\n\nAsk `view: \"spans\"` for what ran instead.\n",
            counts.intervals,
            plural(counts.intervals, "interval"),
            stream::millis_label(counts.traced_ms),
            counts.view_bodies,
            plural(counts.view_bodies, "body"),
            counts.ffi_calls,
            plural(counts.ffi_calls, "call"),
            marks::path(dir),
        );
    }

    let rows: Vec<(Mark, Counts)> = steps
        .iter()
        .map(|mark| {
            let held: Vec<&Interval> = selected
                .iter()
                .copied()
                .filter(|interval| mark.holds(interval.started_ms))
                .collect();

            (mark.clone(), stream::count(&held))
        })
        .collect();

    let mut out = format!(
        "`{}`: {}, from the app's own intervals.\n\n",
        rows[0].0.run,
        headline(rows.len())
    );

    out.push_str("| # | Step | Traced | View bodies | FFI calls | Footprint |\n");
    out.push_str("| -: | :--- | ---: | ---: | ---: | ---: |\n");

    for (mark, counts) in rows.iter().take(MAX_ROWS) {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            mark.step,
            cell(&mark.label),
            stream::millis_label(counts.traced_ms),
            counts.view_bodies,
            counts.ffi_calls,
            footprint(counts.footprint_mb),
        ));
    }

    if rows.len() > MAX_ROWS {
        out.push_str(&format!(
            "\n[{} more steps, not shown. Narrow with `step` or a time window.]\n",
            rows.len() - MAX_ROWS
        ));
    }

    if let Some(reading) = interpret(&rows) {
        out.push_str(&format!("\n{reading}\n"));
    }

    if window.from_ms == 0 {
        out.push_str(
            "\nThe `View bodies` column counts the two view bodies the app instruments, not the \
             whole view tree.\n",
        );
    }

    out
}

/// How a timeline opens, given how many steps it covers.
fn headline(steps: usize) -> String {
    if steps == 1 {
        return "One step".to_owned();
    }

    format!("{steps} steps")
}

/// What the numbers show, when they show something.
///
/// Says nothing rather than inventing a narrative.
/// An agent reading a table may not notice that one column is flat while
/// another doubles, but a report that guessed at a pattern would be worse than
/// one that stayed quiet.
fn interpret(rows: &[(Mark, Counts)]) -> Option<String> {
    if rows.len() < 3 {
        return None;
    }

    let first = &rows[0].1;
    let last = &rows[rows.len() - 1].1;

    let ffi_flat = rows
        .iter()
        .all(|(_, counts)| counts.ffi_calls == first.ffi_calls);
    let bodies_grow =
        last.view_bodies >= first.view_bodies.saturating_mul(2) && first.view_bodies > 0;

    if bodies_grow && ffi_flat {
        return Some(format!(
            "View-body count grows from {} to {} across these steps while FFI calls stay at {}. \
             The cost is re-evaluation, not loading.",
            first.view_bodies, last.view_bodies, first.ffi_calls
        ));
    }

    let climbing =
        rows.windows(2).all(
            |pair| match (pair[0].1.footprint_mb, pair[1].1.footprint_mb) {
                (Some(before), Some(after)) => after >= before,
                _ => false,
            },
        );
    let grew = match (first.footprint_mb, last.footprint_mb) {
        (Some(before), Some(after)) => after.saturating_sub(before),
        _ => 0,
    };

    if climbing && grew >= 20 {
        return Some(format!(
            "Footprint climbs {grew} MB across these steps and never falls."
        ));
    }

    None
}

/// Render the `spans` or `views` table.
fn render_tally(view: View, selected: &[&Interval], span: Option<&str>) -> String {
    let tallied = stream::tally(selected);

    if tallied.is_empty() {
        return match span {
            Some(name) => format!(
                "No interval in this window is named like `{name}`. Drop `span` to see what the \
                 app timed.\n"
            ),
            None => "The app timed nothing in this window.\n".to_owned(),
        };
    }

    let subject = if view == View::Views {
        "view bodies"
    } else {
        "intervals"
    };
    let total: usize = tallied.iter().map(|(_, tally)| tally.count).sum();

    let mut out = format!(
        "{total} {subject} in this window, over {} {}.\n\n",
        tallied.len(),
        plural(tallied.len(), "name")
    );

    out.push_str("| ran | name | total | mean | slowest |\n| ---: | :--- | ---: | ---: | ---: |\n");
    for (name, tally) in tallied.iter().take(MAX_ROWS) {
        out.push_str(&format!(
            "| {} | `{}` | {} | {} | {} |\n",
            tally.count,
            cell(name),
            stream::millis_label(tally.total_ms),
            stream::millis_label(tally.mean_ms()),
            stream::millis_label(tally.max_ms),
        ));
    }

    if tallied.len() > MAX_ROWS {
        out.push_str(&format!(
            "\n[{} more names, not shown. Narrow with `span`.]\n",
            tallied.len() - MAX_ROWS
        ));
    }

    out
}

/// Render two timelines as deltas.
fn compare_timeline(
    now: (&[&Interval], &[Mark]),
    against: (&[&Interval], &[Mark]),
    compared: &Recording,
) -> String {
    let (selected, steps) = now;
    let (against_selected, against_steps) = against;

    if steps.is_empty() || against_steps.is_empty() {
        return format!(
            "Nothing to compare: {} steps in this window and {} in `{}`. A comparison needs a \
             driven run on both sides.\n",
            steps.len(),
            against_steps.len(),
            compared.id
        );
    }

    let mut out = format!(
        "`{}`: {} here against {} in `{}`, on counts. Wall clock is left out: it is not \
         comparable between runs.\n\n",
        steps[0].run,
        headline(steps.len()),
        against_steps.len(),
        compared.id
    );

    out.push_str("| # | Step | View bodies | Δ | FFI calls | Δ |\n");
    out.push_str("| -: | :--- | ---: | ---: | ---: | ---: |\n");

    let mut mismatched = false;

    for mark in steps.iter().take(MAX_ROWS) {
        let here = counts_for(selected, mark);
        let at_position = against_steps.iter().find(|other| other.step == mark.step);

        // Paired on the label as well as the number. Two runs of different lists
        // can hold the same count of steps, and then position alone pairs a
        // selection against a resize and prints the difference as though the two
        // measured one thing.
        let there = at_position
            .filter(|other| other.label == mark.label)
            .map(|other| counts_for(against_selected, other));

        let (bodies, calls) = match (&there, at_position) {
            (Some(there), _) => (
                delta(here.view_bodies, there.view_bodies),
                delta(here.ffi_calls, there.ffi_calls),
            ),
            (None, Some(_)) => {
                mismatched = true;
                ("(other step)".to_owned(), "(other step)".to_owned())
            }
            (None, None) => ("(absent)".to_owned(), "(absent)".to_owned()),
        };

        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            mark.step,
            cell(&mark.label),
            here.view_bodies,
            bodies,
            here.ffi_calls,
            calls,
        ));
    }

    if steps.len() > MAX_ROWS {
        out.push_str(&format!(
            "\n[{} more steps, not shown.]\n",
            steps.len() - MAX_ROWS
        ));
    }

    if steps.len() != against_steps.len() {
        out.push_str(
            "\nThe two runs drove different numbers of steps, so a step number does not \
             necessarily name the same action on both sides. Check the labels.\n",
        );
    }

    if mismatched {
        out.push_str(
            "\nRows marked `(other step)` hold that position in both runs but do not do the same \
             thing, so there is no difference to report between them. Compare two runs of the \
             same list.\n",
        );
    }

    out
}

/// The counts inside one step's window.
fn counts_for(selected: &[&Interval], mark: &Mark) -> Counts {
    let held: Vec<&Interval> = selected
        .iter()
        .copied()
        .filter(|interval| mark.holds(interval.started_ms))
        .collect();

    stream::count(&held)
}

/// Render two tallies as deltas.
fn compare_tally(
    view: View,
    selected: &[&Interval],
    against_selected: &[&Interval],
    compared: &Recording,
    dir: &Utf8Path,
) -> String {
    let here = stream::tally(selected);
    let there: Vec<(String, Tally)> = stream::tally(against_selected);

    if here.is_empty() && there.is_empty() {
        return format!(
            "Neither window holds anything the app timed. `{}` is at `{}` if you want to check \
             what was recorded.\n",
            compared.id,
            state_dir(dir)
        );
    }

    let subject = if view == View::Views {
        "view bodies"
    } else {
        "intervals"
    };

    let mut out = format!(
        "{subject} here against `{}`, on how often each ran.\n\n",
        compared.id
    );

    out.push_str("| name | ran | in `against` | Δ |\n| :--- | ---: | ---: | ---: |\n");

    let mut names: Vec<String> = here.iter().map(|(name, _)| name.clone()).collect();
    let only_there: Vec<String> = there
        .iter()
        .map(|(name, _)| name.clone())
        .filter(|name| !names.contains(name))
        .collect();
    names.extend(only_there);

    for name in names.iter().take(MAX_ROWS) {
        let ours = here
            .iter()
            .find(|(other, _)| other == name)
            .map_or(0, |(_, tally)| tally.count);
        let theirs = there
            .iter()
            .find(|(other, _)| other == name)
            .map_or(0, |(_, tally)| tally.count);

        out.push_str(&format!(
            "| `{}` | {ours} | {theirs} | {} |\n",
            cell(name),
            delta(ours, theirs)
        ));
    }

    if names.len() > MAX_ROWS {
        out.push_str(&format!(
            "\n[{} more names, not shown.]\n",
            names.len() - MAX_ROWS
        ));
    }

    out
}

/// Answer from a finalized bundle.
fn from_bundle(
    view: View,
    recording: &Recording,
    dir: &Utf8Path,
    request: &Request,
    shortenings: &[Shortening],
) -> Result<String, Error> {
    if recording.is_pending(dir) {
        return Err(format!(
            "`{}` is still open, so its bundle is not finalized and nothing can read it: \
             `xctrace` writes a bundle out on its way to exiting. Close it with `mode: \"stop\"`, \
             or ask `view: \"timeline\"`, which reads the app's own intervals and answers while a \
             bracket records.",
            recording.id
        )
        .into());
    }

    if !recording.bundle(dir).exists() {
        return Err(bundle_gone(recording, view).into());
    }

    let Some(target) = target_for(recording, dir) else {
        return Err(format!(
            "`{}` has nothing to attribute its samples to: no app was recorded with it, and this \
             slot's session record is gone. Open the next bracket against a running app, which is \
             what writes the binary, dSYM and load address a symbol needs.",
            recording.id
        )
        .into());
    };

    // Before the bundle is opened, because the answer would otherwise be a full
    // table of names belonging to a build that never ran.
    if let Some(stale) = target.stale() {
        return Err(format!("`{}` cannot be read. {stale}", recording.id).into());
    }

    let bundle = TraceBundle::open(recording.bundle(dir).as_std_path())?;
    let top = request.top.unwrap_or(DEFAULT_TOP);

    let mut out = format!(
        "`{}` ({}, {}), {}.\n\n",
        recording.id,
        if recording.scope.is_system() {
            "every process on the machine"
        } else {
            "the app alone"
        },
        recording.describe(),
        view.label()
    );

    match view {
        View::Hotspots => {
            let report =
                hotspots::read_hotspots(&bundle, &target, request.function.clone(), EXAMINED_PCS)?;
            out.push_str(&hotspots::render_hotspots(&report, shortenings, top));
        }
        View::Callgraph => out.push_str(&render_callgraph(
            &CallgraphBuilder::new(&bundle)
                .pid(Pid::new(i64::from(target.pid)))
                .binary(Some(target.binary.as_std_path().to_owned()))
                .dsym(target.dsym.as_ref().map(|p| p.as_std_path().to_owned()))
                .slide(hotspots::slide_mode(&target))
                .function(request.function.clone())
                .top(top)
                .run()?,
        )),
        View::Allocations => out.push_str(&render_allocations(&bundle, recording, dir)),
        _ => unreachable!("stream-backed views do not reach the bundle"),
    }

    Ok(out)
}

/// Why a recording has no bundle left to read.
fn bundle_gone(recording: &Recording, view: View) -> String {
    if recording.scope.is_system() {
        return format!(
            "`{}` was recorded with no app running, so it covers every process on the machine — \
             its bundle embeds every one of their environments and was destroyed as soon as it \
             was read. Only the summary survives, at `{}.md` under this slot's `profiles/`. \
             Record the next bracket against a running app for `view: \"{}\"` to have something \
             to read.",
            recording.id,
            recording.id,
            view.label()
        );
    }

    format!(
        "`{}` no longer has a bundle. A retained bundle is bounded by age and by a byte budget, \
         and this one has been reclaimed; its summary is kept at `{}.md` under this slot's \
         `profiles/`. Record a new bracket for `view: \"{}\"`.",
        recording.id,
        recording.id,
        view.label()
    )
}

/// What a recording is attributed to.
///
/// The recording's own record first, which is what makes reading it work after
/// `debug_app_quit`.
/// A live session covers a record written before the field existed.
fn target_for(recording: &Recording, dir: &Utf8Path) -> Option<Target> {
    recording.target.clone().or_else(|| {
        Session::load(dir)
            .ok()
            .flatten()
            .as_ref()
            .map(Target::for_session)
    })
}

/// Render the top functions, or the callees of one.
fn render_callgraph(report: &CallgraphReport) -> String {
    if report.stats.is_empty() {
        return format!(
            "No stacks matched: {} ({} samples).\n",
            report.view, report.total_samples
        );
    }

    let mut out = format!(
        "{} — {} samples carried a stack.\n\n",
        report.view, report.total_samples
    );

    out.push_str("| samples | share | function |\n| ---: | ---: | :--- |\n");
    for stat in &report.stats {
        out.push_str(&format!(
            "| {} | {:.1}% | `{}` |\n",
            stat.samples,
            stat.fraction * 100.0,
            symbol(&stat.function)
        ));
    }

    out.push_str(&explain_callgraph(report));
    out
}

/// What a callgraph table does and does not say.
///
/// Two things about it mislead on sight.
/// A run of identical percentages looks like a bug and is not, and a column of
/// bare addresses looks like broken symbolication and usually is not.
fn explain_callgraph(report: &CallgraphReport) -> String {
    let mut notes = Vec::new();

    let tied = report.stats.first().is_some_and(|first| {
        report
            .stats
            .iter()
            .filter(|s| s.samples == first.samples)
            .count()
            > 2
    });
    if tied {
        notes.push(
            "Counting is inclusive: a function counts once for every stack it appears anywhere \
             in. Frames that share one call chain therefore share one count, which is why a run \
             of rows can be identical — they are the chain every sample passed through, not \
             several equally expensive functions.",
        );
    }

    let unnamed = report
        .stats
        .iter()
        .filter(|stat| stat.function.starts_with("0x"))
        .count();
    if unnamed > 0 {
        notes.push(
            "A bare address is a frame in code the trace carries no symbols for, which is most of \
             the system: only the app's own binary can be named here.",
        );
    }

    if notes.is_empty() {
        return String::new();
    }

    format!("\n{}\n", notes.join("\n\n"))
}

/// Any table schema naming allocation data, alongside every schema the bundle
/// holds.
///
/// Matched on a substring rather than on a known name, because there is no
/// known name: `xctrace export` surfaces none today, and a future Xcode that
/// starts surfacing them should be noticed rather than reported as absent.
///
/// Best-effort.
/// A bundle whose table of contents will not open costs this view its least
/// important paragraph, and the footprint it exists to report comes from the
/// app's own stream rather than from the bundle at all.
fn allocation_tables(bundle: &TraceBundle) -> (Vec<String>, Vec<String>) {
    let mut tables: Vec<String> = bundle
        .toc()
        .ok()
        .as_ref()
        .and_then(Toc::first_run)
        .map(|run| run.tables.iter().map(|t| t.schema.clone()).collect())
        .unwrap_or_default();
    tables.sort_unstable();
    tables.dedup();

    let allocation = tables
        .iter()
        .filter(|schema| schema.contains("alloc"))
        .cloned()
        .collect();

    (tables, allocation)
}

/// Render what a recording can say about memory.
///
/// The footprint the app measured for itself, which every run records and which
/// is the number macOS judges a process by.
/// Per-call-site attribution is absent because it is not reachable: the
/// Allocations instrument writes to the trace event store rather than to a
/// table, and `xctrace export` surfaces none of it, so the only reader is
/// Instruments itself.
fn render_allocations(bundle: &TraceBundle, recording: &Recording, dir: &Utf8Path) -> String {
    let (tables, allocation) = allocation_tables(bundle);

    let window = window_of(recording, unix_millis());
    let intervals = stream::load(dir);
    let mut sampled: Vec<&Interval> = intervals
        .iter()
        .filter(|interval| window.holds(interval.started_ms))
        .filter(|interval| interval.footprint_mb.is_some())
        .collect();

    // By when each sample was taken, which is when its interval *ended*.
    // `stream::load` orders by start, and an outer interval containing shorter
    // nested ones starts first and ends last — so taking the ends of the slice
    // as given reports the newest figure as "before" and an older one as
    // "after", reversing the trajectory and the conclusion drawn from it.
    sampled.sort_by_key(|interval| interval.at_ms);

    let mut out = String::new();

    match (sampled.first(), sampled.last()) {
        (Some(first), Some(last)) => {
            let before = first.footprint_mb.unwrap_or_default();
            let after = last.footprint_mb.unwrap_or_default();
            let peak = sampled
                .iter()
                .filter_map(|interval| interval.footprint_mb)
                .max()
                .unwrap_or_default();

            out.push_str(&format!(
                "Footprint over this recording: {before} MB at `{}`, {after} MB at `{}`, peaking \
                 at {peak} MB, over {} samples the app took of itself.\n\n",
                first.name,
                last.name,
                sampled.len()
            ));

            if peak > after {
                out.push_str(&format!(
                    "The peak sits {} MB above where it ended, so that much was transient rather \
                     than retained. Driving the same selections twice tells a high-water mark \
                     from a leak: a mark plateaus on the second visit, a leak keeps climbing.\n\n",
                    peak - after
                ));
            }
        }
        _ => out.push_str(
            "The app sampled its own footprint nowhere in this recording's window, so there is no \
             memory trajectory to show.\n\n",
        ),
    }

    if allocation.is_empty() {
        out.push_str(&per_call_site_note(recording, dir));
    } else {
        out.push_str(&format!(
            "This bundle carries allocation tables ({}), which `xctrace export` has not surfaced \
             before — worth reading into a real per-call-site view.\n",
            allocation.join(", ")
        ));
    }

    if !tables.is_empty() {
        out.push_str(&format!("\nTables in the bundle: {}.\n", tables.join(", ")));
    }

    out
}

/// Why there is no table of allocations by call site, and what to do instead.
fn per_call_site_note(recording: &Recording, dir: &Utf8Path) -> String {
    if !recording.holds(Tier::Allocations) {
        return format!(
            "`{}` recorded {}, so it holds no allocation stacks. The footprint above needs none — \
             the app samples it on every run. For stacks, record a bracket with `capture: \
             [\"allocations\"]` against an app launched with `allocation_stacks: true`, and read \
             what comes back: it is a bundle for Instruments rather than a table.\n",
            recording.id,
            recording.describe()
        );
    }

    format!(
        "Allocation stacks were recorded, and cannot be read from here. The Allocations \
         instrument writes to the trace event store rather than to a table, and `xctrace export` \
         surfaces none of it — Apple's position is that Leaks and Allocations are built on a \
         different recording technology. So this is not a missing analysis: there is nothing on \
         the command line to analyse.\n\nOpen `{}` in Instruments for the call tree. It is kept \
         for exactly that, and it is not to be committed or attached to a bug report.\n",
        recording.bundle(dir)
    )
}

/// What this slot is holding, when it holds anything.
fn held(recordings: &[Recording], dir: &Utf8Path) -> String {
    if recordings.is_empty() {
        return String::new();
    }

    let mut out = String::from("\n## Recordings in this slot\n\n");
    out.push_str(
        "| id | scope | recorded | state | bundle |\n| :--- | :--- | :--- | :--- | :--- |\n",
    );

    for recording in recordings {
        out.push_str(&format!(
            "| `{}` | {} | {} | {} | {} |\n",
            recording.id,
            if recording.scope.is_system() {
                "system"
            } else {
                "attach"
            },
            recording.describe(),
            if recording.is_pending(dir) {
                "open"
            } else {
                "closed"
            },
            if recording.bundle(dir).exists() {
                "kept"
            } else {
                "gone"
            },
        ));
    }

    out
}

/// The calls that answer the questions this report raises.
///
/// Named on every report, because an agent that does not know a view exists
/// will not ask for it.
fn next(view: View, request: &Request, recordings: &[Recording], dir: &Utf8Path) -> String {
    let mut lines: Vec<String> = Vec::new();
    let readable: Vec<&Recording> = recordings
        .iter()
        .filter(|recording| !recording.is_pending(dir) && recording.bundle(dir).exists())
        .collect();

    match view {
        View::Timeline => {
            if request.step.is_none() {
                lines.push(
                    "view=`views` step=`<n>` — which bodies ran under one step, and how often"
                        .to_owned(),
                );
            }
            lines.push(
                "view=`spans` — every interval the app timed, by how often it ran".to_owned(),
            );
        }
        View::Spans => {
            lines.push("view=`views` — the view bodies alone".to_owned());
            lines.push(
                "view=`timeline` — the same intervals, attributed per driven step".to_owned(),
            );
        }
        View::Views => {
            lines.push("view=`spans` — every interval, not only the view bodies".to_owned());
            lines.push("view=`timeline` — the same counts, attributed per driven step".to_owned());
        }
        View::Hotspots => {
            lines.push(
                "view=`callgraph` function=`<name>` — what that function was calling".to_owned(),
            );
            lines.push("view=`timeline` — what each driven step cost, in counts".to_owned());
        }
        View::Callgraph => {
            lines.push(
                "view=`hotspots` — the busiest program counters, with source sites".to_owned(),
            );
        }
        View::Allocations => {
            lines.push("view=`hotspots` — where the time went in the same recording".to_owned());
        }
    }

    // Named whenever there is a bundle to read, because a stream-backed view says
    // which step is expensive and only a bundle-backed one says which code is.
    if !view.needs_bundle()
        && let Some(recording) = readable.last()
    {
        lines.push(format!(
            "view=`hotspots` recording=`{}` — the code the samples landed in",
            recording.id
        ));
    }

    if request.against.is_none()
        && let [first, .., last] = readable.as_slice()
    {
        lines.push(format!(
            "view=`timeline` recording=`{}` against=`{}` — the two compared on counts",
            last.id, first.id
        ));
    }

    if readable.is_empty() && !view.needs_bundle() {
        lines.push(
            "`mode: \"start\"`, drive the operation, `mode: \"stop\"` — then `view: \"hotspots\"` \
             can name the code responsible"
                .to_owned(),
        );
    }

    format!(
        "\n## Next\n\n{}\n",
        lines
            .iter()
            .map(|line| format!("- {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

/// A footprint, or a dash when nothing was sampled.
fn footprint(mb: Option<u64>) -> String {
    mb.map_or_else(|| "—".to_owned(), |mb| format!("{mb} MB"))
}

/// A difference between two counts, signed, or a dash when there is none.
fn delta(here: usize, there: usize) -> String {
    let change = here.cast_signed() - there.cast_signed();
    if change == 0 {
        return "—".to_owned();
    }

    format!("{change:+}")
}

/// `text` as a table cell: escaped, and short enough not to wrap the table.
fn cell(text: &str) -> String {
    let escaped = text.replace('|', "\\|");
    if escaped.chars().count() <= 60 {
        return escaped;
    }

    let kept: String = escaped.chars().take(59).collect();
    format!("{kept}…")
}

/// A symbol name as a table cell.
///
/// Escaped but never shortened.
/// A demangled Rust or Swift name routinely runs past a hundred characters, and
/// the generic parameters that make it long are also what distinguish it from
/// its neighbours — a truncated one names a function nobody can look up.
fn symbol(name: &str) -> String {
    name.replace('|', "\\|")
}

/// `word` pluralized for `count`.
fn plural(count: usize, word: &str) -> String {
    match (count, word) {
        (1, word) => word.to_owned(),
        (_, "body") => "bodies".to_owned(),
        (_, word) => format!("{word}s"),
    }
}

#[cfg(test)]
#[path = "report_tests.rs"]
mod tests;
