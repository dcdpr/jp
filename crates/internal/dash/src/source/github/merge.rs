//! Merge-queue driver that advances the front queue entry by one step per poll.
//!
//! On each poll, the driver updates the front entry with the default branch,
//! waits for all member checks to pass and for GitHub to report each member as
//! mergeable, and then merges the entry pinned to its verified head commit
//! SHAs:
//!
//! - A single pull request is updated via `PUT pulls/{n}/update-branch` and
//!   merged via `PUT pulls/{n}/merge`.
//! - A stack is rebased onto the default branch in a temporary worktree and
//!   atomically force-pushed with a lease ([`git::rebase_stack`]) on a
//!   background thread so polling continues while the rebase runs; the next
//!   poll after completion handles the result.
//!   Once up to date and green, the stack is merged via `PUT
//!   pulls/{n}/merge-async` on its top pull request, which executes
//!   asynchronously on GitHub and is polled on subsequent ticks until it
//!   completes.
//!
//! After a merge, each member's remote branch is deleted on GitHub unless the
//! repository has `delete_branch_on_merge` enabled, the branch belongs to a
//! fork, or another open pull request still builds on it.
//!
//! Merge conflicts or refused update/rebase/merge requests record a [`Hold`]
//! against the entry's head commit signature; pushing a new commit to any
//! member or re-queueing a member manually ([`QueueControl::clear`]) lifts the
//! hold.
//!
//! At most one `dash` process per repository drives the queue, coordinated via
//! a non-blocking exclusive file lock in the cache directory.
//! Other instances report [`QueueRole::Watching`] and retry the lock on every
//! poll.

use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    mem,
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use camino::Utf8Path;
use serde::{Deserialize, Serialize};

use super::{
    Error, GitHub, SUCCESS_FIRST, SUCCESS_LAST, api, api_message, cache, git, parse_json, slim,
};
use crate::{
    model::{Checks, PullRequest},
    queue::{self, Entry, Hold, Next, QueueRole, QueueState, Reason, Status, Step},
    source::git::Rebased,
};

/// Grace period with zero reported checks on the front entry's head commits
/// before assuming no checks will run.
const QUIET: Duration = Duration::from_mins(2);

/// Timeout for an in-flight `merge-async` request before re-submitting it.
const MERGE_PATIENCE: Duration = Duration::from_mins(10);

const MERGE_METHOD_KEY: &str = "dash.queueMergeMethod";

/// Shared merge-queue control state between the UI thread and the GitHub
/// source.
#[derive(Debug, Default)]
pub struct QueueControl {
    paused: AtomicBool,

    /// Pull request numbers manually re-queued since the last poll.
    cleared: Mutex<Vec<u64>>,
}

impl QueueControl {
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    #[must_use]
    pub fn paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    /// Lift any hold on the entry containing pull request `number` at the next
    /// poll.
    pub fn clear(&self, number: u64) {
        self.cleared
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(number);
    }

    pub(super) fn take_cleared(&self) -> Vec<u64> {
        mem::take(&mut *self.cleared.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// Result of a single GitHub poll and merge-queue step.
#[derive(Debug)]
pub struct Poll {
    pub prs: Result<Vec<PullRequest>, Error>,
    pub role: QueueRole,

    /// Deduplicated queue events emitted during this poll.
    pub events: Vec<Event>,

    /// Whether the queue mutated remote state or has an operation in flight,
    /// signaling that the source should re-poll after [`SOON`].
    ///
    /// [`SOON`]: crate::source::SOON
    pub changed: bool,
}

impl Default for Poll {
    fn default() -> Self {
        Self {
            prs: Ok(vec![]),
            role: QueueRole::Driving,
            events: vec![],
            changed: false,
        }
    }
}

/// Status event emitted by the merge queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub message: String,

    /// Whether to send a desktop notification (used for completed merges and
    /// entries held back for failures or conflicts).
    pub notify: bool,
}

/// In-flight `merge-async` request accepted by GitHub.
#[derive(Debug)]
struct Merging {
    /// [`Entry::signature`] of the stack being merged.
    signature: String,

    /// Number of the top pull request on which `merge-async` was called.
    top: u64,

    /// UUID returned by GitHub for polling `merge-async/{uuid}`.
    request: String,

    since: Instant,
}

/// Background stack rebase started by the driver.
#[derive(Debug)]
struct Rebasing {
    key: u64,

    /// [`Entry::signature`] when the rebase started.
    signature: String,

    /// Target base branch for the rebase.
    base: String,

    handle: JoinHandle<Result<Rebased, String>>,
}

#[derive(Debug, Default)]
pub(super) struct Driver {
    /// Exclusive lock file held while this process drives the repository's
    /// merge queue.
    lock: Option<File>,

    /// [`Entry::key`] of the entry currently at the front of the queue.
    pub(super) front: Option<u64>,

    /// Active holds keyed by [`Entry::key`].
    pub(super) holds: HashMap<u64, Hold>,

    /// Pull request numbers whose entry holds should be cleared on the next
    /// [`drive`] call.
    ///
    /// [`drive`]: Self::drive
    cleared: Vec<u64>,

    /// [`Entry::signature`] of the last entry updated or rebased, preventing
    /// duplicate updates when GitHub briefly serves stale head SHAs.
    updated: Option<String>,

    /// Front entry's [`Entry::signature`] and the instant it was first
    /// observed.
    seen: Option<(String, Instant)>,

    merging: Option<Merging>,

    rebasing: Option<Rebasing>,

    /// Branches of pull requests merged during the current poll, queued for
    /// [`delete_merged`].
    ///
    /// [`delete_merged`]: Self::delete_merged
    merged: Vec<Merged>,

    /// Deduplication keys of events already emitted.
    reported: HashSet<String>,
}

/// Remote branch of a merged pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Merged {
    number: u64,
    branch: String,

    /// `(stack_number, position)` if the pull request belonged to a stack.
    stack: Option<(u64, usize)>,
}

impl Driver {
    pub(super) fn new(holds: HashMap<u64, Hold>) -> Self {
        Self {
            holds,
            ..Self::default()
        }
    }

    /// Clear any hold on an entry containing one of `numbers` on the next
    /// [`drive`] call.
    ///
    /// [`drive`]: Self::drive
    pub(super) fn forget(&mut self, numbers: &[u64]) {
        self.cleared.extend_from_slice(numbers);
    }

    /// Advance the front of the merge queue by one step and update each pull
    /// request's [`PullRequest::queue`] state.
    pub(super) fn drive(
        &mut self,
        github: &mut GitHub,
        paused: bool,
        prs: &mut [PullRequest],
        poll: &mut Poll,
    ) -> QueueRole {
        self.release(prs);

        let role = if paused {
            QueueRole::Paused
        } else if self.acquire(&github.dir) {
            QueueRole::Driving
        } else {
            QueueRole::Watching
        };

        let mut step = None;
        if role == QueueRole::Driving {
            self.report_failing(prs, poll);
            step = self.step(github, prs, poll);
            self.delete_merged(github, prs, poll);
        }

        queue::assign(prs, self.front, &self.holds);
        if let Some((key, step)) = step {
            mark_front(prs, key, step);
        }
        role
    }

    /// Drop holds whose entry left the queue, got a new head commit, or was put
    /// back in the queue by hand.
    fn release(&mut self, prs: &[PullRequest]) {
        let entries = queue::entries(prs);
        let cleared = mem::take(&mut self.cleared);

        self.holds.retain(|key, hold| {
            entries.iter().any(|entry| {
                entry.key() == *key
                    && entry.signature() == hold.sha
                    && !cleared.iter().any(|number| entry.contains(*number))
            })
        });
    }

    /// Report each entry failing its checks or conflicting with its base, once
    /// per set of head commits.
    fn report_failing(&mut self, prs: &[PullRequest], poll: &mut Poll) {
        let failing: Vec<(String, String)> = queue::entries(prs)
            .iter()
            .filter_map(|entry| {
                let (kind, what) = match entry.members[0].queue {
                    Some(QueueState::Failing) => ("failed", "failed its checks"),
                    Some(QueueState::Conflicts) => ("conflicts", "conflicts with its base"),
                    _ => return None,
                };
                Some((
                    format!("{}:{}:{kind}", entry.key(), entry.signature()),
                    format!("{} {what}: {}", entry.name(), entry.top().title),
                ))
            })
            .collect();

        for (key, message) in failing {
            self.report(key, message, true, poll);
        }
    }

    /// Decide and take the next step for the front of the queue, returning its
    /// key and the step to show, or `None` when the queue is empty or the front
    /// was held.
    fn step(
        &mut self,
        github: &mut GitHub,
        prs: &[PullRequest],
        poll: &mut Poll,
    ) -> Option<(u64, Step)> {
        if let Some(key) = self.collect(prs, poll) {
            return Some((key, Step::Updating));
        }

        let Some(entry) = queue::front(prs) else {
            self.front = None;
            self.merging = None;
            return None;
        };
        let key = entry.key();
        let signature = entry.signature();
        self.front = Some(key);

        if let Some(merging) = self.merging.take()
            && merging.signature == signature
        {
            return self.follow(github, &entry, merging, poll);
        }

        let since = match &self.seen {
            Some((seen, since)) if *seen == signature => *since,
            _ => {
                let now = Instant::now();
                self.seen = Some((signature, now));
                now
            }
        };

        let status = match github.status(&entry, since.elapsed() >= QUIET) {
            Ok(status) => status,
            Err(error) => {
                let message = format!("Merge queue: reading {} failed: {error}", entry.name());
                self.report(message.clone(), message, false, poll);
                return Some((key, Step::Checking));
            }
        };

        match queue::next(&status) {
            Next::Wait(step) => Some((key, step)),
            Next::Conflicts => {
                self.hold(&entry, Reason::Conflicts, poll);
                None
            }
            Next::Update if entry.stacked() => self.rebase(github, &entry, poll),
            Next::Update => self.update(github, &entry, poll),
            Next::Merge if entry.stacked() => self.merge_stack(github, &entry, poll),
            Next::Merge => self.merge(github, &entry, poll),
        }
    }

    /// Merge the default branch into a single pull request, pinned to its head
    /// commit.
    fn update(
        &mut self,
        github: &mut GitHub,
        entry: &Entry<'_>,
        poll: &mut Poll,
    ) -> Option<(u64, Step)> {
        let (key, signature, pr) = (entry.key(), entry.signature(), entry.top());
        // GitHub can report the old head commit for a moment after an update.
        if self.updated.as_ref() == Some(&signature) {
            return Some((key, Step::Updating));
        }

        let sent = github.send("PUT", &format!("pulls/{}/update-branch", pr.number), &[(
            "expected_head_sha",
            &pr.sha,
        )]);
        match sent.map(|(status, body)| classify_update(status, &body)) {
            Ok(Sent::Done) => {
                let message = format!("Bringing {} up to date before merging it.", entry.name());
                self.report(format!("{key}:{signature}:update"), message, false, poll);
                self.updated = Some(signature);
                poll.changed = true;
                Some((key, Step::Updating))
            }
            Ok(Sent::Moved) => Some((key, Step::Updating)),
            Ok(Sent::Conflicts) => {
                self.hold(entry, Reason::Conflicts, poll);
                None
            }
            Ok(Sent::Refused(reason)) => {
                self.hold(entry, Reason::Blocked(reason), poll);
                None
            }
            Err(error) => {
                let message = format!("Merge queue: updating {} failed: {error}", entry.name());
                self.report(message.clone(), message, false, poll);
                Some((key, Step::Updating))
            }
        }
    }

    /// Poll an in-flight stack rebase started by an earlier tick.
    ///
    /// While the thread is still running, returns `Some(key)` so the driver
    /// keeps the entry at the front without starting another action.
    /// Once the thread finishes, records the outcome and returns `None`.
    fn collect(&mut self, prs: &[PullRequest], poll: &mut Poll) -> Option<u64> {
        let running = self.rebasing.take()?;
        if !running.handle.is_finished() {
            let key = running.key;
            self.rebasing = Some(running);
            self.front = Some(key);
            poll.changed = true;
            return Some(key);
        }

        let outcome = running
            .handle
            .join()
            .unwrap_or_else(|_| Ok(Rebased::Refused("rebasing the stack panicked".to_owned())));
        let entries = queue::entries(prs);
        // An entry that left the queue meanwhile has nothing left to act on.
        let entry = entries.iter().find(|entry| entry.key() == running.key)?;

        match outcome {
            Ok(Rebased::Pushed) => {
                let message = format!(
                    "Rebased {} onto {} before merging it.",
                    entry.name(),
                    running.base
                );
                self.report(
                    format!("{}:{}:rebase", running.key, running.signature),
                    message,
                    false,
                    poll,
                );
                self.updated = Some(running.signature);
                poll.changed = true;
            }
            Ok(Rebased::Moved) => {}
            Ok(Rebased::Conflicts) => self.hold(entry, Reason::Conflicts, poll),
            Ok(Rebased::Refused(reason)) => self.hold(entry, Reason::Blocked(reason), poll),
            Err(error) => {
                let message = format!("Merge queue: rebasing {} failed: {error}", entry.name());
                self.report(message.clone(), message, false, poll);
            }
        }
        None
    }

    /// Spawn a background thread to rebase `entry` onto the default branch and
    /// force-push it; [`collect`] harvests the result on a later poll.
    ///
    /// [`collect`]: Self::collect
    fn rebase(
        &mut self,
        github: &mut GitHub,
        entry: &Entry<'_>,
        poll: &mut Poll,
    ) -> Option<(u64, Step)> {
        let (key, signature) = (entry.key(), entry.signature());
        // GitHub can report the old head commits for a moment after a push.
        if self.updated.as_ref() == Some(&signature) {
            return Some((key, Step::Updating));
        }

        let Some(remote) = github.remote() else {
            let reason =
                "no git remote here points at the repository, so rebase the stack on GitHub";
            self.hold(entry, Reason::Blocked(reason.to_owned()), poll);
            return None;
        };
        let Ok(base) = github.base() else {
            return Some((key, Step::Checking));
        };
        let branches: Vec<(String, String)> = entry
            .members
            .iter()
            .filter_map(|pr| Some((pr.stack.as_ref()?.branch.clone(), pr.sha.clone())))
            .collect();
        let below = entry.members[0]
            .stack
            .as_ref()
            .and_then(|place| place.below.clone());

        let dir = github.dir.clone();
        let onto = base.clone();
        let handle = thread::spawn(move || {
            git::rebase_stack(&dir, &remote, &onto, &branches, below.as_deref())
                .map_err(|error| error.to_string())
        });

        self.rebasing = Some(Rebasing {
            key,
            signature,
            base,
            handle,
        });
        poll.changed = true;
        Some((key, Step::Updating))
    }

    /// Merge a single pull request, pinned to its head commit.
    fn merge(
        &mut self,
        github: &mut GitHub,
        entry: &Entry<'_>,
        poll: &mut Poll,
    ) -> Option<(u64, Step)> {
        let (key, pr) = (entry.key(), entry.top());
        let method = github.merge_method();

        let sent = github.send("PUT", &format!("pulls/{}/merge", pr.number), &[
            ("sha", &pr.sha),
            ("merge_method", &method),
        ]);
        match sent.map(|(status, body)| classify_merge(status, &body)) {
            Ok(Sent::Done) => Some(self.merged(entry, poll)),
            Ok(Sent::Moved) => Some((key, Step::Checking)),
            Ok(Sent::Conflicts) => {
                self.hold(entry, Reason::Conflicts, poll);
                None
            }
            Ok(Sent::Refused(reason)) => {
                self.hold(entry, Reason::Blocked(reason), poll);
                None
            }
            Err(error) => {
                let message = format!("Merge queue: merging {} failed: {error}", entry.name());
                self.report(message.clone(), message, false, poll);
                Some((key, Step::Checking))
            }
        }
    }

    /// Ask GitHub to merge a stack up to its top pull request, pinned to that
    /// pull request's head commit.
    fn merge_stack(
        &mut self,
        github: &mut GitHub,
        entry: &Entry<'_>,
        poll: &mut Poll,
    ) -> Option<(u64, Step)> {
        let top = entry.top();
        let method = github.merge_method();

        let sent = github.send("PUT", &format!("pulls/{}/merge-async", top.number), &[
            ("sha", &top.sha),
            ("merge_method", &method),
            ("merge_action", "direct_merge"),
        ]);
        match sent {
            Ok((status, body)) => {
                self.answer(entry, classify_async(status, &body), Instant::now(), poll)
            }
            Err(error) => {
                let message = format!("Merge queue: merging {} failed: {error}", entry.name());
                self.report(message.clone(), message, false, poll);
                Some((entry.key(), Step::Checking))
            }
        }
    }

    /// Poll the status of an in-flight `merge-async` request.
    fn follow(
        &mut self,
        github: &mut GitHub,
        entry: &Entry<'_>,
        merging: Merging,
        poll: &mut Poll,
    ) -> Option<(u64, Step)> {
        let key = entry.key();
        if merging.since.elapsed() >= MERGE_PATIENCE {
            // Requesting the merge again picks up the same request at GitHub.
            let message = format!(
                "Merge queue: GitHub hasn't finished merging {}, asking again.",
                entry.name()
            );
            self.report(
                format!("{key}:{}:patience", merging.signature),
                message,
                false,
                poll,
            );
            return Some((key, Step::Checking));
        }

        let path = format!("pulls/{}/merge-async/{}", merging.top, merging.request);
        match github.send("GET", &path, &[]) {
            Ok((status, body)) => {
                self.answer(entry, classify_async(status, &body), merging.since, poll)
            }
            Err(error) => {
                let message = format!(
                    "Merge queue: following the merge of {} failed: {error}",
                    entry.name()
                );
                self.report(message.clone(), message, false, poll);
                self.merging = Some(merging);
                poll.changed = true;
                Some((key, Step::Merging))
            }
        }
    }

    /// Act on GitHub's answer about a stack merge first requested at `since`.
    fn answer(
        &mut self,
        entry: &Entry<'_>,
        answer: Async,
        since: Instant,
        poll: &mut Poll,
    ) -> Option<(u64, Step)> {
        let key = entry.key();
        match answer {
            Async::Done => Some(self.merged(entry, poll)),
            Async::Pending(request) => {
                self.merging = Some(Merging {
                    signature: entry.signature(),
                    top: entry.top().number,
                    request,
                    since,
                });
                poll.changed = true;
                Some((key, Step::Merging))
            }
            Async::Moved => Some((key, Step::Checking)),
            Async::Refused(reason) => {
                self.hold(entry, Reason::Blocked(reason), poll);
                None
            }
        }
    }

    /// Record that `entry` merged.
    fn merged(&mut self, entry: &Entry<'_>, poll: &mut Poll) -> (u64, Step) {
        let key = entry.key();
        self.merged.extend(entry.members.iter().filter_map(|pr| {
            Some(Merged {
                number: pr.number,
                branch: pr.remote_branch.clone()?,
                stack: pr.stack.as_ref().map(|place| (place.stack, place.position)),
            })
        }));
        let message = format!("Merged {}: {}", entry.name(), entry.top().title);
        self.report(
            format!("{key}:{}:merged", entry.signature()),
            message,
            true,
            poll,
        );

        self.front = None;
        self.merging = None;
        poll.changed = true;
        (key, Step::Merging)
    }

    /// Delete the remote branches of pull requests merged during this poll.
    ///
    /// Skipped when the repository has `delete_branch_on_merge` enabled; see
    /// [`deletable`] for branches that are retained.
    fn delete_merged(&mut self, github: &mut GitHub, prs: &[PullRequest], poll: &mut Poll) {
        let merged = mem::take(&mut self.merged);
        if merged.is_empty() || github.settings().is_some_and(|s| s.delete_branch_on_merge) {
            return;
        }
        let default = github.default_branch.clone().unwrap_or_default();

        for (number, branch) in deletable(&merged, prs, &default) {
            let key = format!("{number}:{branch}:deleted");
            let failed = match github.send("DELETE", &format!("git/refs/heads/{branch}"), &[]) {
                Ok((SUCCESS_FIRST..=SUCCESS_LAST, _)) => {
                    let message = format!("Deleted {branch}, the branch of #{number}.");
                    self.report(key, message, false, poll);
                    continue;
                }
                // Already gone, deleted by hand or by GitHub.
                Ok((422, body)) if api_message(&body).contains("Reference does not exist") => {
                    continue;
                }
                Ok((status, body)) => format!("HTTP {status}: {}", api_message(&body)),
                Err(error) => error.to_string(),
            };
            let message = format!("Merged #{number}, but deleting {branch} failed: {failed}");
            self.report(key, message, false, poll);
        }
    }

    /// Stop trying to merge `entry` until one of its head commits changes, and
    /// move on to the next entry.
    fn hold(&mut self, entry: &Entry<'_>, reason: Reason, poll: &mut Poll) {
        let (key, signature) = (entry.key(), entry.signature());
        let message = match &reason {
            Reason::Conflicts => format!(
                "{} conflicts with its base: {}",
                entry.name(),
                entry.top().title
            ),
            Reason::Blocked(why) => format!("{} left the merge queue, {why}", entry.name()),
        };

        // Reuse the `"conflicts"` key from `report_failing` so holding an entry
        // and observing its conflicted state emit only a single notification.
        let kind = match &reason {
            Reason::Conflicts => "conflicts",
            Reason::Blocked(_) => "hold",
        };
        self.report(format!("{key}:{signature}:{kind}"), message, true, poll);
        self.holds.insert(key, Hold {
            sha: signature,
            reason,
        });
        self.front = None;
        self.merging = None;
        poll.changed = true;
    }

    /// Add an event to `poll` unless one with `key` was reported before.
    fn report(&mut self, key: String, message: String, notify: bool, poll: &mut Poll) {
        if self.reported.insert(key) {
            poll.events.push(Event { message, notify });
        }
    }

    /// Take the repository's queue lock unless this process holds it already.
    ///
    /// Drives without a lock when there is no cache directory to put it in.
    fn acquire(&mut self, dir: &Utf8Path) -> bool {
        if self.lock.is_some() {
            return true;
        }
        match cache::file(dir, "queue") {
            Some(file) => self.acquire_at(&file.with_extension("lock")),
            None => true,
        }
    }

    fn acquire_at(&mut self, path: &Utf8Path) -> bool {
        if let Some(parent) = path.parent() {
            // A failure surfaces as the open below failing.
            let _missing = fs::create_dir_all(parent);
        }
        let Ok(file) = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
        else {
            // Without a lock file nothing can coordinate, so drive rather than
            // leave the queue standing.
            return true;
        };

        match file.try_lock() {
            Ok(()) => {
                self.lock = Some(file);
                true
            }
            Err(_) => false,
        }
    }
}

/// Show `step` on every member of the entry keyed `key`.
fn mark_front(prs: &mut [PullRequest], key: u64, step: Step) {
    let members = queue::entries(prs)
        .into_iter()
        .find(|entry| entry.key() == key)
        .map_or_default(|entry| entry.numbers());

    for pr in prs.iter_mut().filter(|pr| members.contains(&pr.number)) {
        pr.queue = Some(QueueState::Front(step));
    }
}

impl GitHub {
    /// Fetch the readiness [`Status`] for the front queue `entry`.
    ///
    /// Each member's `behind` count is measured against its immediate base (the
    /// default branch for the bottom member, and the preceding member's head
    /// commit for stacked members above it).
    fn status(&mut self, entry: &Entry<'_>, quiet: bool) -> Result<Status, Error> {
        let repo = self.repo(true)?;
        let base = self.base()?;

        let mut previous = base;
        let compares: Vec<String> = entry
            .members
            .iter()
            .map(|pr| {
                let path = format!("repos/{repo}/compare/{previous}...{}", pr.sha);
                previous.clone_from(&pr.sha);
                path
            })
            .collect();
        self.prefetch(compares.clone());

        let mut behind = 0;
        for path in &compares {
            behind += parse_json::<Compare>(&self.fetch(path, true, slim::<Compare>)?)?.behind_by;
        }

        // The poll read every queued pull request's mergeability already.
        let mut mergeable = Some(true);
        for pr in &entry.members {
            match pr.mergeable {
                Some(false) => mergeable = Some(false),
                None if mergeable == Some(true) => mergeable = None,
                _ => {}
            }
        }

        let checks = entry
            .members
            .iter()
            .fold(Checks::default(), |sum, pr| Checks {
                passed: sum.passed + pr.checks.passed,
                pending: sum.pending + pr.checks.pending,
                failed: sum.failed + pr.checks.failed,
            });

        Ok(Status {
            checks,
            behind,
            mergeable,
            quiet,
        })
    }

    fn base(&self) -> Result<String, Error> {
        self.default_branch
            .clone()
            .ok_or_else(|| Error::NotCached("the default branch".to_owned()))
    }

    fn remote(&mut self) -> Option<String> {
        let repo = self.repo(true).ok()?;
        git::remote_for(&self.dir, &repo)
    }

    /// Return the configured merge method (`dash.queueMergeMethod`), or the
    /// first method allowed by the repository in `[squash, merge, rebase]`
    /// order.
    fn merge_method(&mut self) -> String {
        if let Some(method) = git::config(&self.dir, MERGE_METHOD_KEY) {
            return method;
        }

        choose_method(self.settings().as_ref()).to_owned()
    }

    fn settings(&mut self) -> Option<Settings> {
        let repo = self.repo(true).ok()?;
        let body = self
            .fetch(&format!("repos/{repo}"), true, slim::<Settings>)
            .ok()?;
        parse_json(&body).ok()
    }

    /// Send a `method` request with string `fields` to `path` under the
    /// repository, returning the HTTP status and body whatever the status.
    fn send(
        &mut self,
        method: &str,
        path: &str,
        fields: &[(&str, &str)],
    ) -> Result<(u16, String), Error> {
        let repo = self.repo(true)?;

        let mut command = self.gh();
        command
            .args(["api", "--include", "--method", method])
            .arg(format!("repos/{repo}/{path}"));
        for (name, value) in fields {
            command.arg("-f").arg(format!("{name}={value}"));
        }
        api(&mut command).map(|reply| (reply.status, reply.body))
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Compare {
    behind_by: u64,
}

/// Subset of repository settings used to select the merge method and branch
/// cleanup behavior.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Settings {
    #[serde(default)]
    allow_squash_merge: bool,
    #[serde(default)]
    allow_merge_commit: bool,
    #[serde(default)]
    allow_rebase_merge: bool,

    #[serde(default)]
    delete_branch_on_merge: bool,
}

/// Return `(pr_number, branch_name)` for each branch in `merged` that is safe
/// to delete.
///
/// Retains the repository's `default` branch and any branch that another open
/// pull request in `prs` still targets (either as its base branch or as the
/// preceding member in the same stack).
fn deletable(merged: &[Merged], prs: &[PullRequest], default: &str) -> Vec<(u64, String)> {
    let numbers: HashSet<u64> = merged.iter().map(|merged| merged.number).collect();
    let others: Vec<&PullRequest> = prs
        .iter()
        .filter(|pr| !numbers.contains(&pr.number))
        .collect();

    let builds_on = |merged: &Merged, pr: &PullRequest| {
        let above = match (merged.stack, &pr.stack) {
            (Some((stack, position)), Some(place)) => {
                place.stack == stack && place.position == position + 1
            }
            _ => false,
        };
        above || (pr.stack.is_none() && pr.base.as_deref() == Some(merged.branch.as_str()))
    };

    merged
        .iter()
        .filter(|merged| merged.branch != default)
        .filter(|merged| !others.iter().any(|pr| builds_on(merged, pr)))
        .map(|merged| (merged.number, merged.branch.clone()))
        .collect()
}

/// Select the first allowed merge method in `[squash, merge, rebase]` order,
/// defaulting to `"squash"` when `methods` is `None` or reports no allowed
/// methods.
fn choose_method(methods: Option<&Settings>) -> &'static str {
    match methods {
        Some(methods) if !methods.allow_squash_merge && methods.allow_merge_commit => "merge",
        Some(methods)
            if !methods.allow_squash_merge
                && !methods.allow_merge_commit
                && methods.allow_rebase_merge =>
        {
            "rebase"
        }
        _ => "squash",
    }
}

/// Parsed outcome of a `PUT pulls/{n}/update-branch` or `PUT pulls/{n}/merge`
/// request.
#[derive(Debug, PartialEq, Eq)]
enum Sent {
    Done,

    /// Head or base branch changed since the poll; retry on the next poll.
    Moved,

    Conflicts,

    /// Permanently refused with this reason (formatted to follow `"left the
    /// merge queue, "`).
    Refused(String),
}

/// Classify the response to `PUT pulls/{n}/update-branch`.
fn classify_update(status: u16, body: &str) -> Sent {
    let message = api_message(body);
    let lower = message.to_lowercase();

    match status {
        SUCCESS_FIRST..=SUCCESS_LAST => Sent::Done,
        422 if lower.contains("expected head sha") => Sent::Moved,
        422 if lower.contains("conflict") => Sent::Conflicts,
        _ => Sent::Refused(format!("updating it failed: HTTP {status}: {message}")),
    }
}

/// Classify the response to `PUT pulls/{n}/merge`.
fn classify_merge(status: u16, body: &str) -> Sent {
    let message = api_message(body);

    match status {
        SUCCESS_FIRST..=SUCCESS_LAST => Sent::Done,
        409 => Sent::Moved,
        405 if message.contains("Base branch was modified") => Sent::Moved,
        _ => Sent::Refused(format!("merging it failed: HTTP {status}: {message}")),
    }
}

/// Parsed outcome of a `PUT` or `GET` on `pulls/{n}/merge-async`.
#[derive(Debug, PartialEq, Eq)]
enum Async {
    /// Merged, or handed off to GitHub's native merge queue.
    Done,

    /// Still merging in the background under this request UUID.
    Pending(String),

    /// Head or base branch changed since the poll; retry on the next poll.
    Moved,

    /// Permanently refused with this reason (formatted to follow `"left the
    /// merge queue, "`).
    Refused(String),
}

#[derive(Deserialize)]
struct AsyncReply {
    status: String,
    #[serde(default)]
    details: AsyncDetails,
}

#[derive(Default, Deserialize)]
struct AsyncDetails {
    uuid: Option<String>,
    message: Option<String>,
}

/// Classify the response to `PUT pulls/{n}/merge-async` or `GET
/// pulls/{n}/merge-async/{uuid}`.
fn classify_async(status: u16, body: &str) -> Async {
    if !(SUCCESS_FIRST..=SUCCESS_LAST).contains(&status) {
        return match classify_merge(status, body) {
            Sent::Refused(reason) => Async::Refused(reason),
            // Non-2xx responses never produce `Done` or `Conflicts` from
            // `classify_merge`; treat `Moved` as retryable on the next poll.
            Sent::Moved | Sent::Done | Sent::Conflicts => Async::Moved,
        };
    }

    let Ok(reply) = serde_json::from_str::<AsyncReply>(body) else {
        return Async::Refused(format!(
            "merging it failed: unexpected answer {}",
            api_message(body)
        ));
    };
    match reply.status.as_str() {
        "merged" | "enqueued" => Async::Done,
        "pending" => match reply.details.uuid {
            Some(request) => Async::Pending(request),
            None => {
                Async::Refused("merging it failed: GitHub named no request to follow".to_owned())
            }
        },
        "failed" => Async::Refused(format!(
            "merging it failed: {}",
            reply
                .details
                .message
                .as_deref()
                .unwrap_or("no reason given")
        )),
        other => Async::Refused(format!("merging it failed: GitHub answered {other:?}")),
    }
}

#[cfg(test)]
#[path = "merge_tests.rs"]
mod tests;
