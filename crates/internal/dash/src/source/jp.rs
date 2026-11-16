//! Running `jp` processes and JP sessions, from the workspace's store.
//!
//! A `jp` command that works on a conversation holds that conversation's lock
//! for as long as it runs, and records its PID in the lock file.
//! The lock is read through `jp_storage`'s backend traits, never taken: a probe
//! that took the lock, however briefly, could make a `jp` starting at that
//! moment fail to get it.
//! Whether the holder is still alive is asked of the OS instead.
//!
//! A session is a shell's conversation history, recorded under the shell's
//! session-leader PID.
//! That PID resolves via the process table to the shell and its working
//! directory, which places the session in a worktree.
//! Environment-derived sessions (`$JP_SESSION`, `$TMUX_PANE`) have no
//! session-leader PID and are omitted.
//!
//! Every worktree of the repository shares one user-local store, so the locks
//! and sessions of all of them are found from any one.
//!
//! Because conversations do not record a working directory and session mappings
//! resolve to a directory only while their shell is alive, `Jp` caches the last
//! conversation observed running in each directory on disk (see
//! [`super::cache`]).

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    process::Command,
    sync::Arc,
    time::{Duration, Instant},
};

use camino::{Utf8Path, Utf8PathBuf};
use chrono::{DateTime, Utc};
use jp_config::PartialAppConfig;
use jp_conversation::ConversationId;
use jp_storage::backend::{
    ConversationFilter, FsStorageBackend, LoadBackend as _, LockBackend as _,
};
use jp_workspace::{SessionOverview, Workspace};

use super::{Error, cache, canonical, ghostty::tty_name, output, stdout};
use crate::model::{JpRecent, JpRun, JpSession, SessionConversation, Status};

/// Conversations considered per session: the most recently activated ones.
/// Their metadata is read to order them by activity.
const CONSIDERED: usize = 20;

/// Conversations listed per session, after ordering by activity.
const LISTED: usize = 10;

/// How long an idle conversation's title and last event are trusted before they
/// are read again.
/// A conversation that is running, or was at the last read, is read every time.
const METADATA_TTL: Duration = Duration::from_mins(1);

/// Snapshot of running `jp` processes, active shell sessions, and recent runs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JpState {
    pub runs: Vec<JpRun>,
    pub sessions: Vec<JpSession>,

    /// The conversation last seen running in each directory.
    pub recent: Vec<JpRecent>,
}

/// Reads the running conversations and sessions of the JP workspace containing
/// a directory.
pub struct Jp {
    dir: Utf8PathBuf,

    /// Opened lazily on the first successful poll and reused across reads.
    opened: Option<(Workspace, Arc<FsStorageBackend>)>,

    /// Conversation metadata from earlier reads.
    /// Loading it parses the whole event file, too much to do for every listed
    /// conversation every read.
    metadata: HashMap<ConversationId, Metadata>,

    /// Cached pending-inquiry state for each currently locked conversation.
    waiting: HashMap<ConversationId, Waiting>,

    /// The conversation last seen running in each directory, by the directory's
    /// path.
    ran: BTreeMap<String, ConversationId>,

    /// Where `ran` is kept between runs; `None` when there is no cache
    /// directory.
    file: Option<Utf8PathBuf>,

    /// Whether `ran` changed since it was last written to `file`.
    dirty: bool,
}

/// Cached inquiry-wait state for a locked conversation, keyed by event count
/// and last-event timestamp.
#[derive(Debug, Clone, Copy)]
struct Waiting {
    events: usize,
    last_event_at: Option<DateTime<Utc>>,
    since: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
struct Metadata {
    title: Option<String>,
    pinned: bool,
    events: usize,
    last_event_at: Option<DateTime<Utc>>,
    read_at: Instant,
    running: bool,
}

impl Jp {
    /// Read the workspace containing `dir`, starting from the runs the last
    /// dashboard remembered.
    #[must_use]
    pub fn new(dir: Utf8PathBuf) -> Self {
        let file = cache::file(&dir, "jp");
        let mut ran: BTreeMap<String, ConversationId> =
            file.as_deref().map(cache::load).unwrap_or_default();

        // A directory that is gone, such as a removed worktree, has no row.
        let remembered = ran.len();
        ran.retain(|cwd, _| Utf8Path::new(cwd).exists());

        Self {
            dir,
            opened: None,
            metadata: HashMap::new(),
            waiting: HashMap::new(),
            dirty: ran.len() != remembered,
            ran,
            file,
        }
    }

    pub fn read(&mut self) -> Result<JpState, Error> {
        if self.opened.is_none() {
            let workspace = Workspace::open_read_only(&self.dir)?;
            let storage = workspace.fs_storage().cloned().ok_or(Error::NoStorage)?;
            self.opened = Some((workspace, storage));
        }
        let Some((workspace, storage)) = &self.opened else {
            return Err(Error::NoStorage);
        };

        let holders: Vec<(ConversationId, u32)> = storage
            .load_conversation_ids(ConversationFilter::default())
            .into_iter()
            .filter_map(|id| Some((id, storage.lock_info(&id.to_string())?.pid)))
            .collect();

        // Each session is placed by its shell, the leader's child when the
        // leader is `login` (as Ghostty starts shells), or the leader itself.
        let overviews: Vec<_> = workspace
            .session_overviews()
            .into_iter()
            .filter_map(|overview| {
                let pid = u32::try_from(overview.session.leader_pid()?).ok()?;
                Some((pid, overview))
            })
            .collect();
        let table = process_table()?;
        let shells: Vec<(u32, u32)> = overviews
            .iter()
            .filter_map(|(leader, _)| Some((*leader, shell_of(&table, *leader)?)))
            .collect();

        let pids: Vec<u32> = holders
            .iter()
            .map(|(_, pid)| *pid)
            .chain(shells.iter().map(|(_, shell)| *shell))
            .collect();
        let processes = processes(&pids)?;

        // A lock file outlives a holder killed with SIGKILL, and its PID may
        // since belong to something else, so only a `jp` process counts.
        let mut running: HashMap<ConversationId, Held> = holders
            .into_iter()
            .filter_map(|(id, pid)| {
                let process = processes.get(&pid)?;
                (process.command == "jp").then(|| {
                    (id, Held {
                        pid,
                        cwd: process.cwd.clone(),
                        waiting_since: None,
                    })
                })
            })
            .collect();

        let seen = running.iter().map(|(id, held)| (held.cwd.as_str(), *id));
        self.dirty |= remember(&mut self.ran, seen);

        let listed: HashSet<ConversationId> = overviews
            .iter()
            .flat_map(|(_, overview)| {
                overview
                    .conversations
                    .iter()
                    .take(CONSIDERED)
                    .chain(overview.active.as_ref())
            })
            .chain(running.keys())
            .chain(self.ran.values())
            .copied()
            .collect();
        self.refresh_metadata(&listed, &running.keys().copied().collect());
        self.refresh_waiting(&mut running);

        let runs = running
            .iter()
            .map(|(id, held)| JpRun {
                pid: held.pid,
                cwd: held.cwd.clone(),
                conversation: title(&self.metadata, id),
                waiting_since: held.waiting_since,
            })
            .collect();

        let sessions = shells
            .iter()
            .filter_map(|(leader, shell)| {
                let (_, overview) = overviews.iter().find(|(pid, _)| pid == leader)?;
                let shell = Shell {
                    leader: *leader,
                    pid: *shell,
                    ghostty: under_ghostty(&table, *leader),
                    tty: table.get(shell).and_then(|entry| tty_name(&entry.tty)),
                    cwd: processes.get(shell)?.cwd.clone(),
                };
                Some(session(shell, overview, &running, &self.metadata))
            })
            .collect();

        let recent = recent(&self.ran, &self.metadata);
        self.persist();

        Ok(JpState {
            runs,
            sessions,
            recent,
        })
    }

    /// Write `ran` to its file when it changed.
    fn persist(&mut self) {
        let Some(file) = &self.file else {
            return;
        };

        // Ignore cache write errors; a failed write only drops cached recent
        // runs on the next start.
        if self.dirty && cache::store(file, &self.ran).is_ok() {
            self.dirty = false;
        }
    }

    /// Update `waiting_since` for each locked conversation by checking for
    /// unanswered inquiries in its last turn.
    ///
    /// Reloads a conversation's event stream only when its `events_count` or
    /// `last_event_at` changes.
    fn refresh_waiting(&mut self, running: &mut HashMap<ConversationId, Held>) {
        let Some((_, storage)) = &self.opened else {
            return;
        };

        self.waiting.retain(|id, _| running.contains_key(id));
        for (id, held) in running.iter_mut() {
            let (events, last_event_at) = self.metadata.get(id).map_or((0, None), |metadata| {
                (metadata.events, metadata.last_event_at)
            });

            let cached = self.waiting.get(id).filter(|waiting| {
                waiting.events == events && waiting.last_event_at == last_event_at
            });
            if let Some(waiting) = cached {
                held.waiting_since = waiting.since;
                continue;
            }

            // A stream that fails to load says nothing about a question, so it
            // reads as not waiting.
            let since = storage
                .load_conversation_stream(id, &PartialAppConfig::empty())
                .ok()
                .and_then(|stream| {
                    stream
                        .pending_inquiries()
                        .iter()
                        .map(|event| event.timestamp)
                        .min()
                });
            self.waiting.insert(*id, Waiting {
                events,
                last_event_at,
                since,
            });
            held.waiting_since = since;
        }
    }

    /// Load the metadata of each conversation in `listed` that is not cached,
    /// is running or was at the last read, or was read too long ago; and forget
    /// the rest.
    fn refresh_metadata(
        &mut self,
        listed: &HashSet<ConversationId>,
        running: &HashSet<ConversationId>,
    ) {
        let Some((_, storage)) = &self.opened else {
            return;
        };

        self.metadata.retain(|id, _| listed.contains(id));
        let stale: Vec<ConversationId> = listed
            .iter()
            .filter(|id| {
                self.metadata.get(id).is_none_or(|metadata| {
                    metadata.running
                        || running.contains(id)
                        || metadata.read_at.elapsed() >= METADATA_TTL
                })
            })
            .copied()
            .collect();

        for (id, loaded) in storage.load_conversation_metadata_batch(&stale) {
            let Ok(loaded) = loaded else {
                self.metadata.remove(&id);
                continue;
            };
            self.metadata.insert(id, Metadata {
                title: loaded.title,
                pinned: loaded.pinned_at.is_some(),
                events: loaded.events_count,
                last_event_at: loaded.last_event_at,
                read_at: Instant::now(),
                running: running.contains(&id),
            });
        }
    }
}

/// Record the active conversation for each working directory in `running`,
/// returning `true` if `ran` changed.
fn remember<'a>(
    ran: &mut BTreeMap<String, ConversationId>,
    running: impl IntoIterator<Item = (&'a str, ConversationId)>,
) -> bool {
    let mut changed = false;
    for (cwd, id) in running {
        changed |= ran.insert(cwd.to_owned(), id) != Some(id);
    }

    changed
}

/// Build [`JpRecent`] entries from `ran` and the loaded conversation
/// `metadata`.
fn recent(
    ran: &BTreeMap<String, ConversationId>,
    metadata: &HashMap<ConversationId, Metadata>,
) -> Vec<JpRecent> {
    ran.iter()
        .map(|(cwd, id)| JpRecent {
            cwd: Utf8PathBuf::from(cwd),
            last_event_at: metadata.get(id).and_then(|metadata| metadata.last_event_at),
        })
        .collect()
}

/// A conversation's title, or its ID when it has none or its metadata is not
/// loaded.
fn title(metadata: &HashMap<ConversationId, Metadata>, id: &ConversationId) -> String {
    metadata
        .get(id)
        .and_then(|metadata| metadata.title.clone())
        .unwrap_or_else(|| id.to_string())
}

/// A conversation a live `jp` process holds.
struct Held {
    pid: u32,
    cwd: Utf8PathBuf,
    waiting_since: Option<DateTime<Utc>>,
}

/// A session's shell, as the process table shows it.
struct Shell {
    leader: u32,
    pid: u32,
    ghostty: bool,
    tty: Option<String>,
    cwd: Utf8PathBuf,
}

/// Build a [`JpSession`] from `shell` and `overview`.
///
/// Lists the active conversation first, then the others by most recent
/// activity, up to [`LISTED`].
fn session(
    shell: Shell,
    overview: &SessionOverview,
    running: &HashMap<ConversationId, Held>,
    metadata: &HashMap<ConversationId, Metadata>,
) -> JpSession {
    let conversation = |id: &ConversationId| {
        let loaded = metadata.get(id);
        SessionConversation {
            id: id.to_string(),
            title: loaded.and_then(|m| m.title.clone()),
            active: overview.active == Some(*id),
            pinned: loaded.is_some_and(|m| m.pinned),
            events: loaded.map_or(0, |m| m.events),
            last_event_at: loaded.and_then(|m| m.last_event_at),
            status: running
                .get(id)
                .map_or(Status::Idle, |held| Status::held(held.waiting_since)),
        }
    };

    let mut conversations: Vec<SessionConversation> = overview
        .conversations
        .iter()
        .take(CONSIDERED)
        .chain(overview.active.as_ref())
        .collect::<HashSet<_>>()
        .into_iter()
        .map(conversation)
        .collect();
    conversations.sort_by(|a, b| {
        b.active
            .cmp(&a.active)
            .then_with(|| b.last_event_at.cmp(&a.last_event_at))
            .then_with(|| b.id.cmp(&a.id))
    });
    conversations.truncate(LISTED);

    JpSession {
        pid: shell.leader,
        shell: shell.pid,
        ghostty: shell.ghostty,
        tty: shell.tty,
        cwd: shell.cwd,
        more: overview
            .conversations
            .len()
            .saturating_sub(conversations.len()),
        conversations,
    }
}

/// A process table entry.
#[derive(Debug, PartialEq, Eq)]
struct Entry {
    ppid: u32,

    /// The controlling terminal, as `ps` names it (`ttys001`, or `??`).
    tty: String,
    command: String,
}

/// Every process's parent, controlling terminal, and command.
fn process_table() -> Result<HashMap<u32, Entry>, Error> {
    let out = stdout(
        "ps",
        Command::new("ps").args(["-A", "-o", "pid=,ppid=,tty=,comm="]),
    )?;
    Ok(parse_ps(&out))
}

/// Parse `ps -o pid=,ppid=,tty=,comm=` output.
/// The command is last, so it may hold spaces.
fn parse_ps(out: &str) -> HashMap<u32, Entry> {
    out.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let ppid = fields.next()?.parse().ok()?;
            let tty = fields.next()?.to_owned();
            let command = fields.collect::<Vec<_>>().join(" ");
            Some((pid, Entry { ppid, tty, command }))
        })
        .collect()
}

/// The shell of the session led by `leader`: the leader's oldest child when the
/// leader is `login`, which runs the shell as its child, otherwise the leader
/// itself.
/// `None` when the leader has exited.
fn shell_of(table: &HashMap<u32, Entry>, leader: u32) -> Option<u32> {
    let entry = table.get(&leader)?;
    if entry.command.rsplit('/').next() != Some("login") {
        return Some(leader);
    }

    table
        .iter()
        .filter(|(_, child)| child.ppid == leader)
        .map(|(pid, _)| *pid)
        .min()
        .or(Some(leader))
}

/// Whether `pid` descends from a Ghostty process.
fn under_ghostty(table: &HashMap<u32, Entry>, pid: u32) -> bool {
    let mut current = pid;
    // Bound parent traversal in case a concurrent process-table change
    // introduces a cycle.
    for _ in 0..64 {
        let Some(entry) = table.get(&current) else {
            return false;
        };
        if entry.command.rsplit('/').next() == Some("ghostty") {
            return true;
        }
        if entry.ppid == current || entry.ppid == 0 {
            return false;
        }
        current = entry.ppid;
    }
    false
}

/// Return `true` if `pid`'s process group is the foreground process group of
/// its controlling terminal.
pub fn shell_is_idle(pid: u32) -> Result<bool, Error> {
    let out = stdout(
        "ps",
        Command::new("ps").args(["-o", "pgid=,tpgid=", "-p", &pid.to_string()]),
    )?;
    parse_foreground(&out).ok_or_else(|| Error::Failed {
        program: "ps",
        message: format!("no process group for {pid} in {out:?}"),
    })
}

/// Parse `ps -o pgid=,tpgid=`: whether the process group is the terminal's
/// foreground group.
fn parse_foreground(out: &str) -> Option<bool> {
    let mut fields = out.split_whitespace();
    let group: i64 = fields.next()?.parse().ok()?;
    let foreground: i64 = fields.next()?.parse().ok()?;
    Some(group == foreground)
}

/// A process's command name and working directory.
#[derive(Debug, PartialEq, Eq)]
struct Process {
    command: String,
    cwd: Utf8PathBuf,
}

/// Look up the command name and working directory of each live PID.
///
/// PIDs of processes that have exited are absent from the result.
fn processes(pids: &[u32]) -> Result<HashMap<u32, Process>, Error> {
    if pids.is_empty() {
        return Ok(HashMap::new());
    }

    let pids = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");

    // `lsof` exits 1 when any listed PID has no process, which is expected
    // here, so the status says nothing and the output is read regardless.
    let out = output(
        "lsof",
        Command::new("lsof").args(["-a", "-d", "cwd", "-F", "pcn", "-p", &pids]),
    )?;

    Ok(parse_lsof(&String::from_utf8_lossy(&out.stdout))
        .into_iter()
        .map(|(pid, process)| {
            let cwd = canonical(process.cwd.as_str());
            (pid, Process { cwd, ..process })
        })
        .collect())
}

/// Parse `lsof -F pcn` output: a `p<pid>` line opens each process, followed by
/// its `c<command>`, and an `n<path>` for its working directory.
fn parse_lsof(out: &str) -> HashMap<u32, Process> {
    let mut processes = HashMap::new();
    let mut pid = None;
    let mut command = String::new();

    for line in out.lines() {
        let Some(tag) = line.chars().next() else {
            continue;
        };
        let value = &line[tag.len_utf8()..];

        match tag {
            'p' => {
                pid = value.parse().ok();
                command.clear();
            }
            'c' => value.clone_into(&mut command),
            'n' => {
                if let Some(pid) = pid {
                    processes.insert(pid, Process {
                        command: command.clone(),
                        cwd: value.into(),
                    });
                }
            }
            _ => {}
        }
    }

    processes
}

#[cfg(test)]
#[path = "jp_tests.rs"]
mod tests;
