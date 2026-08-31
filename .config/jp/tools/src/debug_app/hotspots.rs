//! Reading a recorded bundle down to a summary worth keeping.
//!
//! A `.trace` embeds the environment of every process it recorded, and `xctrace
//! export --toc` prints it.
//! A system-wide recording therefore holds whatever every process on the
//! machine had exported, and the caller destroys that bundle as soon as this
//! returns — see [`Recording::retire`].
//!
//! Extraction goes through `xct2cli`, which strips `<environment>` from
//! everything it parses and keeps its raw-XML accessors crate-private, so there
//! is no path by which a recorded environment reaches a report.
//!
//! Never commit a bundle or attach one to a bug report.
//!
//! [`Recording::retire`]: super::capture::Recording::retire

use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use xct2cli::{
    Pid, TraceBundle,
    analysis::{HotspotReport, HotspotsBuilder, SlideMode},
};

use crate::{
    Error,
    debug_app::capture::{Recording, Target, Tier},
    util::paths::{Shortening, shorten},
};

/// How many of the busiest program counters to symbolicate.
///
/// Far more than the report shows, because most of what an app is doing on-CPU
/// is inside the dyld shared cache, which a trace carries no symbols for.
/// Only after symbolication is it known which counters belong to code we can
/// name, and taking the busiest 25 before that yields a table of bare
/// addresses.
const EXAMINED_PCS: usize = 500;

/// How many named frames a summary shows.
pub(crate) const TOP_FRAMES: usize = 25;

/// A recording, reduced to what can safely be kept.
pub(crate) struct Summary {
    pub path: Utf8PathBuf,
    pub content: String,
}

/// Read the bundle and write a summary beside it.
///
/// Does not delete the bundle.
/// The caller does that unconditionally, so that a failure here still leaves
/// nothing behind.
pub(crate) fn summarize(
    recording: &Recording,
    dir: &Utf8Path,
    target: Option<&Target>,
    shortenings: &[Shortening],
) -> Result<Summary, Error> {
    let extract = extract(recording, dir, target)?;
    let content = render(recording, &extract, target, shortenings);
    let path = recording.summary(dir);

    fs::write(&path, &content).map_err(|e| format!("Failed to write {path}: {e}"))?;

    Ok(Summary { path, content })
}

/// The parts of the bundle worth keeping.
struct Extract {
    /// Absent when there was no app to attribute samples to.
    hotspots: Option<HotspotReport>,

    /// How long the recording ran, in seconds, as the bundle reports it.
    duration: Option<String>,

    /// Why the recording ended.
    end_reason: Option<String>,

    /// The instrument tables the bundle holds.
    tables: Vec<String>,
}

fn extract(
    recording: &Recording,
    dir: &Utf8Path,
    target: Option<&Target>,
) -> Result<Extract, Error> {
    let bundle = TraceBundle::open(recording.bundle(dir).as_std_path())?;

    let toc = bundle.toc()?;
    let run = toc.first_run();
    let summary = run.and_then(|r| r.info.summary.as_ref());
    let mut tables: Vec<String> = run
        .map(|r| r.tables.iter().map(|t| t.schema.clone()).collect())
        .unwrap_or_default();
    tables.sort_unstable();
    tables.dedup();

    let hotspots = match target {
        None => None,
        Some(target) => Some(read_hotspots(&bundle, target, None, EXAMINED_PCS)?),
    };

    Ok(Extract {
        hotspots,
        duration: summary.and_then(|s| s.duration.clone()),
        end_reason: summary.and_then(|s| s.end_reason.clone()),
        tables,
    })
}

/// Render the summary that replaces the bundle.
fn render(
    recording: &Recording,
    extract: &Extract,
    target: Option<&Target>,
    shortenings: &[Shortening],
) -> String {
    let mut out = format!("# `{}`\n\n", recording.id);

    out.push_str(&format!("- recorded: {}\n", recording.describe()));
    out.push_str(&format!(
        "- scope: {}\n",
        if recording.scope.is_system() {
            "every process on the machine"
        } else {
            "the app alone"
        }
    ));

    match target {
        Some(target) => out.push_str(&format!(
            "- app: pid {}, `{}`\n",
            target.pid, target.configuration
        )),
        None => out.push_str("- app: none was launched into this recording\n"),
    }

    if let Some(duration) = &extract.duration {
        out.push_str(&format!("- recording: {duration}s\n"));
    }
    if let Some(reason) = &extract.end_reason {
        out.push_str(&format!("- ended: {reason}\n"));
    }
    if !extract.tables.is_empty() {
        out.push_str(&format!("- tables: {}\n", extract.tables.join(", ")));
    }

    out.push_str("\n## Time profile\n\n");
    match &extract.hotspots {
        None => out.push_str(
            "Nothing to attribute: the bracket recorded the machine but no app was launched into \
             it.\n",
        ),
        Some(hotspots) => out.push_str(&render_hotspots(hotspots, shortenings, TOP_FRAMES)),
    }

    if recording.holds(Tier::Allocations) {
        out.push_str(
            "\n## Allocations\n\nRecorded, and readable only while the bundle existed. Timings \
             above ran under `MallocStackLogging`, which costs 2x to 10x and not evenly — compare \
             them against another allocations recording, never against a sampling one.\n",
        );
    }

    if recording.keeps_bundle() {
        out.push_str(
            "\nThe `.trace` bundle is kept, so `debug_app_profile` with `mode: \"report\"` can \
             ask it further questions.\n",
        );
    } else {
        out.push_str(
            "\nThe `.trace` bundle this came from was deleted: recorded system-wide, it embeds \
             the environment of every process on the machine.\n",
        );
    }

    out
}

/// Where a symbolicator gets the load address to subtract.
///
/// The address the app reported for itself when there is one.
/// Recovering it from the trace's image-load events is the fallback, and it
/// only works for a recording that was already running when those images were
/// mapped — which an attached bracket never is.
/// Against an attached recording the fallback silently resolves to a zero
/// slide, and every frame comes back as a bare address or, more confusingly, as
/// whatever symbol happens to live at that offset.
///
/// Every read of a bundle goes through this, so a new one cannot resolve frames
/// against a different address than the others do.
pub(crate) fn slide_mode(target: &Target) -> SlideMode {
    match target.slide {
        Some(slide) => SlideMode::Manual(slide),
        None => SlideMode::Auto,
    }
}

/// Read the busiest program counters out of an open bundle.
///
/// `filter` keeps only frames whose resolved name holds it, which costs
/// symbolicating every examined counter rather than only the ones kept.
pub(crate) fn read_hotspots(
    bundle: &TraceBundle,
    target: &Target,
    filter: Option<String>,
    examined: usize,
) -> Result<HotspotReport, Error> {
    HotspotsBuilder::new(bundle)
        .pid(Pid::new(i64::from(target.pid)))
        .binary(Some(target.binary.as_std_path().to_owned()))
        .dsym(target.dsym.as_ref().map(|p| p.as_std_path().to_owned()))
        .slide(slide_mode(target))
        .filter(filter)
        .top(examined)
        .run()
        .map_err(Into::into)
}

/// Render the frames that could be named, and account for those that could not.
pub(crate) fn render_hotspots(
    hotspots: &HotspotReport,
    shortenings: &[Shortening],
    top: usize,
) -> String {
    let total = hotspots.total_samples;
    if total == 0 {
        return "No samples landed in the app. Either it spent the bracket blocked — which is the \
                normal state for an app nobody is driving — or the bracket closed before it did \
                any work.\n"
            .to_owned();
    }

    let examined = hotspots.top_pcs.len();
    let named: Vec<_> = hotspots
        .top_pcs
        .iter()
        .filter(|h| h.function.is_some())
        .collect();

    let mut out = format!("{total} samples landed in the app.\n\n");

    if named.is_empty() {
        out.push_str(&format!(
            "None of the {examined} busiest program counters resolved to a symbol, so all of this \
             time is in code the trace carries no symbols for — the dyld shared cache, most \
             likely. A run under a bracket that covers real work should look different; if it \
             does not, the slide is wrong.\n"
        ));

        return out;
    }

    // Without this a reader cannot tell whether the frames below are all there
    // were, or the visible corner of a much longer tail.
    if named.len() > top {
        out.push_str(&format!(
            "Showing the {top} busiest of {} named frames.\n\n",
            named.len()
        ));
    }

    out.push_str("| samples | share | function | site |\n| ---: | ---: | --- | --- |\n");
    for hotspot in named.iter().take(top) {
        #[allow(clippy::cast_precision_loss, reason = "display only")]
        let share = (hotspot.samples as f64 / total as f64) * 100.0;

        let function = hotspot.function.clone().unwrap_or_default();

        // DWARF holds these as absolute paths on the machine that built the
        // binary, and a summary is meant to be pasteable into an issue.
        let site = match (&hotspot.file, hotspot.line) {
            (Some(file), Some(line)) => format!("{}:{line}", shorten(file, shortenings)),
            (Some(file), None) => shorten(file, shortenings),
            _ => String::new(),
        };

        out.push_str(&format!(
            "| {} | {share:.1}% | `{}` | {site} |\n",
            hotspot.samples,
            function.replace('|', "\\|")
        ));
    }

    let unnamed = examined - named.len();
    if unnamed > 0 {
        out.push_str(&format!(
            "\n{unnamed} of the {examined} busiest program counters had no symbol in the app's \
             binary. That is system code in the dyld shared cache, which a trace carries no \
             symbols for.\n"
        ));
    }

    out
}

#[cfg(all(test, unix))]
#[path = "hotspots_tests.rs"]
mod tests;
