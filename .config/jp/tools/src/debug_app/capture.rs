//! The Instruments recording a profile bracket runs.
//!
//! A recording is a bracket inside a driven session rather than a property of
//! one: `debug_app_profile` opens it, a later call closes it, and a session can
//! hold several in sequence.
//! That is what keeps a report about the operation someone asked about rather
//! than about a mostly-idle app, and what keeps the recorder out of
//! `debug_app_launch` and `debug_app_quit`.
//!
//! Scope follows from when the bracket opens.
//! With a session already running there is a process to attach to and the trace
//! holds that process alone.
//! With no session there is nothing to attach to, so the recorder takes the
//! whole machine — the only way to cover an app's own startup, and minutes of
//! work at analysis time, because every process's samples are exported before
//! ours can be filtered out of them.
//!
//! Allocation attribution is the one tier the scope cannot give you.
//! The Allocations instrument refuses a target of all processes, so it is
//! reachable only by attaching — and attaching means the app is already
//! running, which it must have been launched with `MallocStackLogging` to be
//! any use, because libmalloc reads that at process start.
//! So `debug_app_launch` decides what an app is able to report, and a bracket
//! decides what is recorded of it.
//!
//! The recorder outlives the process that starts it — `start` and `stop` are
//! separate runs of this binary — so [`Recording`] is written to disk beside
//! the bundle, and [`stop`] reaches the recorder through a signal.
//!
//! Scope also decides what survives being read.
//! A system-wide bundle embeds the environment of every process on the machine,
//! so it is destroyed and only its summary is kept.
//! An attach bundle embeds this app's alone, so it stays, which is what makes
//! re-scoping and comparing two runs possible — bounded by an age window and a
//! byte budget, whichever bites first, oldest evicted first.
//! The same window and budget cover the app's own interval streams, archived
//! here one per run.

use std::{
    fs,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use xct2cli::Slide;

use crate::{
    Error,
    debug_app::{
        marks,
        session::{Session, Signal, Signals},
    },
};

/// The recorder, resolved through `xcrun` so it follows the selected Xcode.
const RECORDER: &str = "xcrun";

/// What the recorder's process is called once `xcrun` has handed off to it.
///
/// Checked before signalling, because a recorder that exited leaves its pid in
/// a record that outlives it, and the kernel hands that number out again.
const RECORDER_PROCESS: &str = "xctrace";

/// Where every recording's artifacts live, inside a slot's directory.
const PROFILES_DIR: &str = "profiles";

/// How long to wait for the recorder to report that it is recording.
pub(crate) const READY_TIMEOUT: Duration = Duration::from_mins(1);

/// How long the recorder gets to write the bundle out after `SIGINT`.
///
/// Generous, because finalizing is minutes of real work for a system-wide
/// recording and there is no way to shorten it: a recorder cut off partway
/// leaves a bundle nothing can open.
pub(crate) const FINALIZE_TIMEOUT: Duration = Duration::from_mins(5);

/// How long a bracket nobody closed stays reachable before it is reclaimed.
///
/// Whether the recorder is still alive deliberately does not enter into it.
/// A recorder that failed on its own still wrote a bundle and still said why in
/// its log, and treating that as abandoned would destroy the diagnostic along
/// with it — which is exactly how a failed recording becomes invisible.
/// Age is the discriminator instead.
///
/// Two days rather than hours, because the data is gone for good once this
/// elapses: long enough to come back the next morning, notice a bracket that
/// failed, and still be able to read what it recorded.
const PENDING_WINDOW: Duration = Duration::from_hours(48);

/// How long a closed recording's artifacts stay readable.
///
/// The same span as [`PENDING_WINDOW`], for the same reason: long enough to
/// come back the next morning and ask a second question of what a run recorded.
const RETENTION_WINDOW: Duration = Duration::from_hours(48);

/// How many bytes of retained artifacts one slot keeps.
///
/// Age bounds nothing about size.
/// One system-wide recording pulled in around 450 symbol archives, and filling
/// the disk is a demonstrated failure here rather than a hypothetical one, so
/// the two limits run together and whichever bites first evicts — oldest
/// first, either way.
const RETENTION_BUDGET: u64 = 2 * 1024 * 1024 * 1024;

/// Extension on an archived stream of the app's own intervals.
const STREAM_EXTENSION: &str = "jsonl";

/// Prefix naming an archived stream, matching [`new_id`]'s shape for a
/// recording.
const STREAM_PREFIX: &str = "trace-";

/// Poll interval while waiting on the recorder.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// What the recorder prints when a run it recorded misbehaved.
const RUN_ISSUES_MARKER: &str = "Run issues were detected";

/// One instrument a recording holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Tier {
    /// Time Profiler: periodic backtraces, attributed per core and per pid.
    Sampling,

    /// Allocations: every allocation with the stack that made it, which needs
    /// `MallocStackLogging` in the target's environment.
    Allocations,
}

impl Tier {
    /// The Instruments instrument name.
    const fn instrument(self) -> &'static str {
        match self {
            Tier::Sampling => "Time Profiler",
            Tier::Allocations => "Allocations",
        }
    }

    /// The name a caller writes and a report prints.
    const fn label(self) -> &'static str {
        match self {
            Tier::Sampling => "sampling",
            Tier::Allocations => "allocations",
        }
    }
}

/// The tiers a `capture` argument asks for.
///
/// Sampling is always in the result and cannot be named: it has no toggle, so
/// accepting the word would imply one.
/// Saying that is better than ignoring it, because a caller who passes
/// `["sampling"]` believes they turned something on.
pub(crate) fn parse_tiers(requested: &[String]) -> Result<Vec<Tier>, Error> {
    let mut tiers = vec![Tier::Sampling];

    for name in requested {
        match name.as_str() {
            "allocations" => {
                if !tiers.contains(&Tier::Allocations) {
                    tiers.push(Tier::Allocations);
                }
            }
            "sampling" => {
                return Err(
                    "`capture` does not accept \"sampling\": every recording holds a time \
                     profile, so there is nothing to ask for. Pass `[]` for that alone, or \
                     `[\"allocations\"]` to add allocation attribution."
                        .into(),
                );
            }
            other => {
                return Err(format!(
                    "`capture` does not accept {other:?}. The only value is \"allocations\"; \
                     sampling is always recorded."
                )
                .into());
            }
        }
    }

    Ok(tiers)
}

/// What the recorder is pointed at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Scope {
    /// One running process, named by pid.
    Attach(u32),

    /// Every process on the machine.
    System,
}

impl Scope {
    /// Whether analysis has to sift other processes out of this trace.
    pub(crate) const fn is_system(self) -> bool {
        matches!(self, Scope::System)
    }
}

/// The app a recording is attributed to.
///
/// Everything symbolication needs, in one place that survives the session.
/// `debug_app_quit` removes the session record, and reading a recording after
/// quitting is the ordinary case rather than an edge, so a closed bracket
/// writes this into its own sidecar and answers for itself from then on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Target {
    pub pid: u32,

    /// The executable inside the launched bundle.
    pub binary: Utf8PathBuf,

    /// The dSYM the build produced, when it produced one.
    pub dsym: Option<Utf8PathBuf>,

    /// The ASLR slide the app reported for its own main image.
    ///
    /// `None` falls back to recovering it from the trace's image-load events,
    /// which only works for a recording that was already running when the app's
    /// images were mapped.
    pub slide: Option<Slide>,

    pub configuration: String,

    /// The `LC_UUID` of the binary this recording's samples came from.
    ///
    /// The paths above do not survive a rebuild.
    /// `binary` points into the slot's staged bundle, which every launch
    /// deletes and copies afresh, and `dsym` into derived data, which Xcode
    /// overwrites in place.
    /// A recording is kept for two days, so record, edit, rebuild, report is
    /// both the ordinary loop and enough to leave those paths holding a
    /// different build.
    ///
    /// Symbolicating against the wrong binary does not fail.
    /// It resolves each address to whatever now lives at that offset, so the
    /// table comes back full of plausible names for code that never ran.
    /// The UUID is what makes that detectable: it is stamped into the binary at
    /// link time, so a rebuild changes it even when the path does not.
    ///
    /// `None` for a binary that could not be read when the bracket closed,
    /// which leaves the read unguarded rather than refused.
    #[serde(default)]
    pub uuid: Option<[u8; 16]>,
}

impl Target {
    /// What a running session says about the app a bracket is recording.
    pub(crate) fn for_session(session: &Session) -> Target {
        let binary = app_binary(&session.bundle);

        Target {
            uuid: binary_uuid(&binary),
            pid: session.pid,
            binary,
            dsym: session.dsym.clone(),
            slide: session.reported_slide(),
            configuration: session.configuration.clone(),
        }
    }

    /// Why this recording's binary can no longer be trusted, if it cannot.
    ///
    /// `None` when the binary still matches, or when there is nothing to
    /// compare: a recording made before the UUID was stored, or a binary that
    /// could not be read either time, is read as it always was.
    pub(crate) fn stale(&self) -> Option<String> {
        let recorded = self.uuid?;
        let found = binary_uuid(&self.binary)?;

        if found == recorded {
            return None;
        }

        Some(format!(
            "The binary at `{}` is not the one this was recorded from \u{2014} it has been \
             rebuilt since. Symbolicating against it would name whatever now sits at each \
             address, which reads as a plausible answer and is not one. Record a new bracket \
             against the current build.",
            self.binary
        ))
    }
}

/// The `LC_UUID` of a Mach-O binary, or `None` when it cannot be read.
fn binary_uuid(binary: &Utf8Path) -> Option<[u8; 16]> {
    xct2cli::symbol::macho::BinaryInfo::open(binary.as_std_path())
        .ok()?
        .uuid
}

/// The executable inside a launched app bundle.
///
/// Named after the bundle, which is what Xcode does and what staging preserves.
pub(crate) fn app_binary(bundle: &Utf8Path) -> Utf8PathBuf {
    let name = bundle.file_stem().unwrap_or("JP");

    bundle.join("Contents/MacOS").join(name)
}

/// One recording, as written beside its bundle.
///
/// On disk rather than in the session record, because a recording can open
/// before a session exists and can outlive the record `debug_app_quit` removes.
/// It is also what lets a sweep tell an abandoned bundle from one still being
/// written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Recording {
    /// Names this recording, and every file belonging to it.
    pub id: String,

    /// The instruments being recorded.
    pub tiers: Vec<Tier>,

    /// What the recorder was pointed at.
    pub scope: Scope,

    /// The `xctrace` process writing the bundle.
    pub recorder_pid: u32,

    /// When the bracket opened, in seconds since the epoch.
    pub started_unix: u64,

    /// When the bracket closed, if it has.
    ///
    /// Stamped before the bundle is read, so a read that fails leaves a closed
    /// recording rather than a bracket that looks open forever and blocks the
    /// next one.
    #[serde(default)]
    pub stopped_unix: Option<u64>,

    /// The app this recording is attributed to, as far as it is known.
    ///
    /// Written when the bracket closes.
    /// `None` for a bracket nothing was ever launched into, which leaves the
    /// samples with nothing to attribute.
    #[serde(default)]
    pub target: Option<Target>,
}

impl Recording {
    /// Whether this recording holds `tier`.
    pub(crate) fn holds(&self, tier: Tier) -> bool {
        self.tiers.contains(&tier)
    }

    /// The tiers, as a phrase for a report.
    pub(crate) fn describe(&self) -> String {
        self.tiers
            .iter()
            .map(|t| t.label())
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub(crate) fn bundle(&self, dir: &Utf8Path) -> Utf8PathBuf {
        profiles_dir(dir).join(format!("{}.trace", self.id))
    }

    pub(crate) fn log(&self, dir: &Utf8Path) -> Utf8PathBuf {
        profiles_dir(dir).join(format!("{}.log", self.id))
    }

    pub(crate) fn sidecar(&self, dir: &Utf8Path) -> Utf8PathBuf {
        profiles_dir(dir).join(format!("{}.json", self.id))
    }

    pub(crate) fn summary(&self, dir: &Utf8Path) -> Utf8PathBuf {
        profiles_dir(dir).join(format!("{}.md", self.id))
    }

    /// What the recorder said, as far as it has been written.
    pub(crate) fn said(&self, dir: &Utf8Path) -> String {
        fs::read_to_string(self.log(dir)).unwrap_or_default()
    }

    /// Write the record beside its bundle.
    ///
    /// Through a temporary file and a rename, so the record is either the
    /// previous one or the new one and never half of either.
    /// A partial write would be worse than no write at all: `recordings` cannot
    /// parse it, so the recording disappears from `pending` and from every
    /// sweep, while `orphaned_bundles` sees a file with the right name and
    /// leaves the bundle alone.
    /// A live recorder and a bundle holding recorded environments would then
    /// sit there with nothing able to name either.
    pub(crate) fn store(&self, dir: &Utf8Path) -> Result<(), Error> {
        let path = self.sidecar(dir);
        fs::create_dir_all(profiles_dir(dir))?;
        let json = serde_json::to_string_pretty(self)?;

        // Beside the destination rather than in the system temp directory, so
        // the rename stays on one filesystem and cannot degrade to a copy.
        let staging = path.with_extension("json.writing");
        fs::write(&staging, format!("{json}\n"))
            .map_err(|e| format!("Failed to write {staging}: {e}"))?;

        fs::rename(&staging, &path)
            .map_err(|e| format!("Failed to move {staging} into place at {path}: {e}").into())
    }

    /// Whether this bracket is still open.
    ///
    /// A stop stamp or a summary means it already closed, whatever else is on
    /// disk.
    /// Liveness of the recorder says only whether stopping it needs a signal,
    /// not whether there is anything to stop for.
    pub(crate) fn is_pending(&self, dir: &Utf8Path) -> bool {
        self.stopped_unix.is_none() && !self.summary(dir).exists() && self.age() < PENDING_WINDOW
    }

    /// Record that the bracket closed, and what it was recording.
    pub(crate) fn close(&mut self, target: Option<Target>, dir: &Utf8Path) -> Result<(), Error> {
        self.stopped_unix = Some(unix_seconds());
        self.target = target;

        self.store(dir)
    }

    /// How long ago the bracket opened.
    fn age(&self) -> Duration {
        Duration::from_secs(unix_seconds().saturating_sub(self.started_unix))
    }

    /// Delete the bundle, the recorder's output, and this record.
    ///
    /// The summary is left: it is the point of the exercise, and it holds
    /// nothing the bundle held.
    pub(crate) fn discard(&self, dir: &Utf8Path) -> Result<(), Error> {
        self.discard_bundle(dir)?;
        remove_file(&self.sidecar(dir))?;

        Ok(())
    }

    /// Delete the bundle and the recorder's output, keeping the record.
    ///
    /// The record is what attributes everything else, so it outlives the
    /// bundle: a report can still say which app a summary belongs to, and why
    /// the bundle itself is not there to re-read.
    pub(crate) fn discard_bundle(&self, dir: &Utf8Path) -> Result<(), Error> {
        let bundle = self.bundle(dir);
        remove_dir(&bundle).map_err(|e| {
            format!(
                "Failed to delete the trace bundle at {bundle}: {e}. It holds the environment of \
                 every process it recorded — delete it by hand and do not attach it to anything."
            )
        })?;

        remove_file(&self.log(dir))
    }

    /// Whether this recording's bundle is safe to keep once it has been read.
    ///
    /// A bundle embeds the environment of every process it recorded.
    /// Recorded system-wide, that is the whole machine's, so it goes.
    /// Recorded by attaching, it is this app's alone — launchd's environment
    /// plus the values the launch passed — and keeping it is what makes
    /// re-scoping, comparing two runs, and recovering from a bad read possible
    /// at all.
    pub(crate) const fn keeps_bundle(&self) -> bool {
        !self.scope.is_system()
    }

    /// Close out a read recording, destroying whatever must not be kept.
    pub(crate) fn retire(&self, dir: &Utf8Path) -> Result<(), Error> {
        if self.keeps_bundle() {
            return Ok(());
        }

        self.discard_bundle(dir)
    }

    /// How many bytes this recording occupies on disk.
    fn bytes(&self, dir: &Utf8Path) -> u64 {
        dir_bytes(&self.bundle(dir)) + file_bytes(&self.log(dir)) + file_bytes(&self.sidecar(dir))
    }
}

/// Where every recording's artifacts live.
pub(crate) fn profiles_dir(dir: &Utf8Path) -> Utf8PathBuf {
    dir.join(PROFILES_DIR)
}

/// The open bracket, if there is one.
///
/// At most one: opening a second is refused.
pub(crate) fn pending(dir: &Utf8Path) -> Option<Recording> {
    recordings(dir)
        .into_iter()
        .find(|recording| recording.is_pending(dir))
}

/// Every recording this slot has a record of.
pub(crate) fn recordings(dir: &Utf8Path) -> Vec<Recording> {
    let Ok(entries) = fs::read_dir(profiles_dir(dir)) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = Utf8PathBuf::from_path_buf(entry.path()).unwrap_or_default();
        if path.extension() != Some("json") {
            continue;
        }

        if let Ok(raw) = fs::read_to_string(&path)
            && let Ok(recording) = serde_json::from_str::<Recording>(&raw)
        {
            out.push(recording);
        }
    }

    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Reclaim what is no longer worth keeping, and report which ids went.
///
/// Three rules, in order.
///
/// An open bracket survives untouched — including one whose recorder failed,
/// which still has a bundle and a reason worth reading.
///
/// A closed system-wide recording loses its bundle immediately, whatever its
/// age: that bundle embeds the environment of every process on the machine, so
/// one left behind is credential material sitting on disk.
///
/// Everything else is retained and then bounded, by [`RETENTION_WINDOW`] and
/// [`RETENTION_BUDGET`] together, oldest evicted first.
/// The step boundaries `debug_app_drive` records expire on the same window, by
/// line rather than by file: one file holds every run a slot has driven.
///
/// Summaries are never swept.
/// They are small, they are the product, and they hold none of what the bundle
/// held.
pub(crate) fn sweep(dir: &Utf8Path, signals: &dyn Signals) -> Vec<String> {
    let mut swept = Vec::new();
    let mut retained = Vec::new();

    let expired = marks::sweep(dir, RETENTION_WINDOW);
    if expired > 0 {
        swept.push(format!("{expired} expired step boundaries"));
    }

    for recording in recordings(dir) {
        if recording.is_pending(dir) {
            continue;
        }

        // A bracket that aged out of `is_pending` may still have a recorder
        // behind it, and everything below this deletes the record naming its
        // pid. Interrupted first, or `xctrace` goes on writing to a path that
        // no longer exists with nothing left able to stop it.
        //
        // Liveness deliberately does not decide whether a bracket is *pending*
        // — a recorder that failed on its own still left a bundle and a reason
        // worth reading. It decides whether one can be *deleted*, which is a
        // different question with a different answer.
        if recording.stopped_unix.is_none()
            && signals.is_alive(recording.recorder_pid)
            && signals.is(recording.recorder_pid, RECORDER_PROCESS)
        {
            let (outcome, _) = stop(recording.recorder_pid, signals, FINALIZE_TIMEOUT);
            swept.push(format!(
                "stopped the recorder abandoned by `{}` ({outcome:?})",
                recording.id
            ));
        }

        if !recording.keeps_bundle()
            && recording.bundle(dir).exists()
            && recording.discard_bundle(dir).is_ok()
        {
            swept.push(recording.id.clone());
        }

        retained.push(Artifact::Bundle(recording));
    }

    retained.extend(streams(dir).into_iter().map(Artifact::Stream));

    swept.extend(orphaned_bundles(dir));
    swept.extend(enforce_limits(dir, retained, RETENTION_BUDGET));
    swept
}

/// One thing a slot keeps after the run that produced it.
enum Artifact {
    /// A recording, and its bundle when that was kept.
    Bundle(Recording),

    /// One earlier run's stream of the app's own intervals.
    Stream(Utf8PathBuf),
}

impl Artifact {
    fn id(&self) -> String {
        match self {
            Artifact::Bundle(recording) => recording.id.clone(),
            Artifact::Stream(path) => path.file_stem().unwrap_or_default().to_owned(),
        }
    }

    /// When the run that produced this began, in seconds since the epoch.
    fn started_unix(&self) -> u64 {
        match self {
            Artifact::Bundle(recording) => recording.started_unix,
            Artifact::Stream(path) => stream_started_unix(path),
        }
    }

    fn bytes(&self, dir: &Utf8Path) -> u64 {
        match self {
            Artifact::Bundle(recording) => recording.bytes(dir),
            Artifact::Stream(path) => file_bytes(path),
        }
    }

    fn evict(&self, dir: &Utf8Path) -> Result<(), Error> {
        match self {
            Artifact::Bundle(recording) => recording.discard(dir),
            Artifact::Stream(path) => remove_file(path),
        }
    }
}

/// Evict retained artifacts until both limits hold, oldest first.
fn enforce_limits(dir: &Utf8Path, mut retained: Vec<Artifact>, budget: u64) -> Vec<String> {
    let mut swept = Vec::new();
    retained.sort_by_key(Artifact::started_unix);

    let now = unix_seconds();
    let mut kept = Vec::new();
    let mut total = 0_u64;

    for artifact in retained {
        if now.saturating_sub(artifact.started_unix()) > RETENTION_WINDOW.as_secs() {
            if artifact.evict(dir).is_ok() {
                swept.push(artifact.id());
            }
            continue;
        }

        let bytes = artifact.bytes(dir);
        total = total.saturating_add(bytes);
        kept.push((artifact, bytes));
    }

    for (artifact, bytes) in kept {
        if total <= budget {
            break;
        }

        if artifact.evict(dir).is_ok() {
            total = total.saturating_sub(bytes);
            swept.push(artifact.id());
        }
    }

    swept
}

/// Move the app's own trace stream into the retained set.
///
/// Returns the id it was archived under, or `None` when there was nothing there
/// to archive.
///
/// A launch that truncated this file would make cross-run comparison of the
/// app's own timings impossible whatever happened to the bundles — the
/// per-step counts live here and nowhere else — so the previous run's stream
/// is kept under the same window and budget as everything else.
pub(crate) fn archive_stream(dir: &Utf8Path, stream: &Utf8Path) -> Result<Option<String>, Error> {
    if file_bytes(stream) == 0 {
        return Ok(None);
    }

    let id = format!("{STREAM_PREFIX}{}", unix_millis());
    let archived = profiles_dir(dir).join(format!("{id}.{STREAM_EXTENSION}"));

    fs::create_dir_all(profiles_dir(dir))?;
    fs::rename(stream, &archived)
        .map_err(|e| format!("Failed to archive {stream} as {archived}: {e}"))?;

    Ok(Some(id))
}

/// Every archived stream this slot holds, oldest first.
pub(crate) fn streams(dir: &Utf8Path) -> Vec<Utf8PathBuf> {
    let Ok(entries) = fs::read_dir(profiles_dir(dir)) else {
        return Vec::new();
    };

    let mut out: Vec<Utf8PathBuf> = entries
        .flatten()
        .filter_map(|entry| Utf8PathBuf::from_path_buf(entry.path()).ok())
        .filter(|path| path.extension() == Some(STREAM_EXTENSION))
        .collect();

    out.sort_by_key(stream_started_unix);
    out
}

/// When the run behind an archived stream began, read out of its name.
///
/// An unparseable name reads as the epoch, which makes it the oldest thing in
/// the slot and the first evicted.
/// That is the right end to fail towards: a file nothing can date is a file
/// nothing can attribute either.
fn stream_started_unix(path: &Utf8PathBuf) -> u64 {
    path.file_stem()
        .and_then(|stem| stem.strip_prefix(STREAM_PREFIX))
        .and_then(|millis| millis.parse::<u64>().ok())
        .map_or(0, |millis| millis / 1000)
}

/// Bundles with no record at all, from a bracket that died between creating the
/// bundle and writing its record.
fn orphaned_bundles(dir: &Utf8Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(profiles_dir(dir)) else {
        return Vec::new();
    };

    let mut swept = Vec::new();
    for entry in entries.flatten() {
        let path = Utf8PathBuf::from_path_buf(entry.path()).unwrap_or_default();
        if path.extension() != Some("trace") {
            continue;
        }

        // Read rather than merely counted: a file that cannot be parsed names
        // nothing, so a bundle beside it is as unowned as one with no file at
        // all. Treating the name as proof of ownership is what would leave a
        // recorded environment on disk after an interrupted write.
        let id = path.file_stem().unwrap_or_default().to_owned();
        let sidecar = profiles_dir(dir).join(format!("{id}.json"));
        if fs::read_to_string(&sidecar)
            .ok()
            .is_some_and(|raw| serde_json::from_str::<Recording>(&raw).is_ok())
        {
            continue;
        }

        drop(remove_file(&sidecar));

        if remove_dir(&path).is_ok() {
            swept.push(id);
        }
    }

    swept
}

/// The `xcrun xctrace record` command line for `tiers` over `scope`.
///
/// `--instrument` rather than `--template`: on Xcode 26 a template produces a
/// bundle whose export fails with "Document Missing Template Error".
///
/// `--no-prompt` is deliberately absent: with it a recording can abort about
/// 34ms in.
pub(crate) fn record_args(bundle: &Utf8Path, tiers: &[Tier], scope: Scope) -> Vec<String> {
    let mut args = vec!["xctrace".to_owned(), "record".to_owned()];

    for tier in tiers {
        args.push("--instrument".to_owned());
        args.push(tier.instrument().to_owned());
    }

    match scope {
        Scope::Attach(pid) => {
            args.push("--attach".to_owned());
            args.push(pid.to_string());
        }
        Scope::System => args.push("--all-processes".to_owned()),
    }

    args.push("--output".to_owned());
    args.push(bundle.to_string());

    args
}

/// Starting a process that outlives this one.
///
/// A seam, because the recorder cannot be held as a `Child`: the process that
/// spawns it exits when `debug_app_profile` returns, and a later run of this
/// binary is what stops it.
pub(crate) trait Spawner {
    /// Start the recorder, and return its pid once it is recording.
    ///
    /// Both its streams go to `log`.
    fn start(
        &self,
        args: &[String],
        log: &Utf8Path,
        working_dir: &Utf8Path,
        timeout: Duration,
    ) -> Result<u32, Error>;
}

/// Production [`Spawner`]: a real `xcrun` process.
pub(crate) struct RealSpawner;

impl Spawner for RealSpawner {
    fn start(
        &self,
        args: &[String],
        log: &Utf8Path,
        working_dir: &Utf8Path,
        timeout: Duration,
    ) -> Result<u32, Error> {
        if let Some(parent) = log.parent() {
            fs::create_dir_all(parent)?;
        }

        let out = fs::File::create(log).map_err(|e| format!("Failed to create {log}: {e}"))?;
        let err = out.try_clone()?;

        let mut child = Command::new(RECORDER)
            .args(args)
            .current_dir(working_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::from(out))
            .stderr(Stdio::from(err))
            .spawn()
            .map_err(|e| format!("Failed to spawn `{RECORDER} {}`: {e}", args.join(" ")))?;

        let pid = child.id();
        let deadline = Instant::now() + timeout;

        loop {
            let said = fs::read_to_string(log).unwrap_or_default();

            // `try_wait` rather than a liveness check: until this process reaps
            // it, a recorder that died is a zombie, and a zombie still answers
            // `kill(pid, 0)`.
            if let Some(status) = child.try_wait()? {
                return Err(format!(
                    "The recorder exited with status {status} before it started recording. It \
                     said:\n\n```\n{}\n```",
                    said.trim_end()
                )
                .into());
            }

            if is_recording(&said) {
                return Ok(pid);
            }

            if Instant::now() >= deadline {
                drop(child.kill());
                drop(child.wait());

                return Err(format!(
                    "The recorder never reported that it started recording within {}s. It \
                     said:\n\n```\n{}\n```",
                    timeout.as_secs(),
                    said.trim_end()
                )
                .into());
            }

            thread::sleep(POLL_INTERVAL);
        }
    }
}

/// Whether the recorder has said it is recording.
///
/// Matched liberally: this is another tool's human-facing output and the
/// wording has moved between Xcode versions.
/// Every phrase here means the same thing — the recorder is live and waiting
/// to be interrupted.
fn is_recording(said: &str) -> bool {
    let said = said.to_lowercase();

    said.contains("ctrl-c") || said.contains("ctrl+c") || said.contains("starting recording")
}

/// Whether the recorder reported a problem with what it recorded.
///
/// The exit status is no help and is not consulted: a completed `xctrace` run
/// exits non-zero, carrying the status of what it recorded, so treating that as
/// failure throws away good traces.
/// What the recorder says on the way out is one signal; a bundle on disk is the
/// other.
pub(crate) fn run_issues(said: &str) -> bool {
    said.contains(RUN_ISSUES_MARKER)
}

/// What the recorder said about the run issues it reported.
///
/// The marker line and the bulleted reasons under it, which is where an
/// instrument that refused its target says so.
/// The rest of the log is progress chatter.
pub(crate) fn run_issue_lines(said: &str) -> String {
    said.lines()
        .skip_while(|line| !line.contains(RUN_ISSUES_MARKER))
        .take_while(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// How stopping the recorder went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stop {
    /// Interrupted, and exited having written the bundle out.
    Finalized,

    /// Already gone before it was asked to stop.
    Absent,

    /// Still running when the wait ran out.
    Stuck,
}

/// Interrupt the recorder and wait for it to write the bundle out.
///
/// `SIGINT` and nothing harsher, at any point.
/// `xctrace` finalizes the bundle on its way out, so a recorder that is killed
/// leaves one nothing can open — which makes a stuck recorder something to
/// report rather than escalate against.
pub(crate) fn stop(pid: u32, signals: &dyn Signals, timeout: Duration) -> (Stop, Duration) {
    let started = Instant::now();

    if !signals.is_alive(pid) {
        return (Stop::Absent, started.elapsed());
    }

    signals.send(pid, Signal::Int);
    let deadline = started + timeout;

    loop {
        if !signals.is_alive(pid) {
            return (Stop::Finalized, started.elapsed());
        }

        if Instant::now() >= deadline {
            return (Stop::Stuck, started.elapsed());
        }

        thread::sleep(POLL_INTERVAL);
    }
}

/// An id for a new recording, and the moment it belongs to.
pub(crate) fn new_id() -> (String, u64) {
    (format!("profile-{}", unix_millis()), unix_seconds())
}

/// Seconds since the epoch, or zero if the clock is before it.
pub(crate) fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Milliseconds since the epoch, or zero if the clock is before it.
pub(crate) fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// How many bytes a file holds, or zero when it is not there.
fn file_bytes(path: &Utf8Path) -> u64 {
    fs::metadata(path).map_or(0, |meta| meta.len())
}

/// How many bytes a directory tree holds, or zero when it is not there.
///
/// Walked rather than asked of the directory itself, because a `.trace`
/// bundle's size is entirely in the symbol archives inside it.
fn dir_bytes(path: &Utf8Path) -> u64 {
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };

    entries
        .flatten()
        .filter_map(|entry| {
            let path = Utf8PathBuf::from_path_buf(entry.path()).ok()?;
            let meta = entry.metadata().ok()?;

            Some(if meta.is_dir() {
                dir_bytes(&path)
            } else {
                meta.len()
            })
        })
        .sum()
}

/// Remove a directory, tolerating one that is already gone.
fn remove_dir(path: &Utf8Path) -> Result<(), std::io::Error> {
    match fs::remove_dir_all(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Remove a file, tolerating one that is already gone.
fn remove_file(path: &Utf8Path) -> Result<(), Error> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Failed to delete {path}: {e}").into()),
    }
}

#[cfg(all(test, unix))]
#[path = "capture_tests.rs"]
mod tests;
