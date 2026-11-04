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
//! That PID leads, through the process table, to the shell and its working
//! directory, which places the session in a worktree.
//! Sessions named some other way (`$JP_SESSION`, `$TMUX_PANE`) name no process,
//! and are left out.
//!
//! Every worktree of the repository shares one user-local store, so the locks
//! and sessions of all of them are found from any one.

use std::{
    collections::{HashMap, HashSet},
    process::Command,
    sync::Arc,
    time::{Duration, Instant},
};

use camino::Utf8PathBuf;
use chrono::{DateTime, Utc};
use jp_config::PartialAppConfig;
use jp_conversation::ConversationId;
use jp_storage::backend::{
    ConversationFilter, FsStorageBackend, LoadBackend as _, LockBackend as _,
};
use jp_workspace::{SessionOverview, Workspace};

use super::{Error, canonical, ghostty::tty_name, output, stdout};
use crate::model::{JpRun, JpSession, SessionConversation, Status};

/// Conversations considered per session: the most recently activated ones.
/// Their metadata is read to order them by activity.
const CONSIDERED: usize = 20;

/// Conversations listed per session, after ordering by activity.
const LISTED: usize = 10;

/// How long an idle conversation's title and last event are trusted before they
/// are read again.
/// A conversation that is running, or was at the last read, is read every time.
const METADATA_TTL: Duration = Duration::from_mins(1);

/// What the JP source reports on each read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JpState {
    pub runs: Vec<JpRun>,
    pub sessions: Vec<JpSession>,
}

/// Reads the running conversations and sessions of the JP workspace containing
/// a directory.
pub struct Jp {
    dir: Utf8PathBuf,

    /// Opened on the first successful read and kept: every read it is used for
    /// goes to disk, so it never serves a stale answer.
    opened: Option<(Workspace, Arc<FsStorageBackend>)>,

    /// Conversation metadata from earlier reads.
    /// Loading it parses the whole event file, too much to do for every listed
    /// conversation every read.
    metadata: HashMap<ConversationId, Metadata>,

    /// For each conversation a live `jp` holds, whether it waits on the user,
    /// as of the stream it was last worked out from.
    waiting: HashMap<ConversationId, Waiting>,
}

/// Whether a held conversation waits on the user, and the stream it was worked
/// out from, known by its event count and last event.
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
    #[must_use]
    pub fn new(dir: Utf8PathBuf) -> Self {
        Self {
            dir,
            opened: None,
            metadata: HashMap::new(),
            waiting: HashMap::new(),
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

        Ok(JpState { runs, sessions })
    }

    /// Work out which held conversations wait on the user: those with an
    /// unanswered question, which `jp` writes before it prompts for it.
    ///
    /// A conversation's stream is loaded again only when its event count or
    /// last event moved since it was last loaded; while a turn waits on the
    /// user, neither does.
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

/// The session `overview` describes, whose shell is `shell`.
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
    // Bounded, so a table read mid-change cannot loop forever.
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

/// Whether the shell `pid` sits at its prompt: it is the foreground process
/// group of its terminal, so nothing it started is running in front of it.
///
/// Asked right before typing into the shell's tab.
/// Anything typed while another program runs in the foreground goes to that
/// program.
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
