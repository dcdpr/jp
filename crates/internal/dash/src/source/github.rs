//! GitHub pull request, CI check, and review-thread polling.
//!
//! Pull requests and commit checks are fetched via the GitHub REST API using
//! conditional requests (`If-None-Match` with the cached `ETag`).
//! Unchanged resources return `304 Not Modified`, which does not count against
//! the GitHub rate limit.
//!
//! Review threads are fetched via GraphQL because the REST API omits pending
//! review comments created by `just pr-review` and `just pr-triage`.
//! Because GraphQL does not support conditional requests, review threads are
//! refreshed per pull request only after [`LOCAL_REVIEWS_INTERVAL`] (when
//! checked out in a local worktree) or [`REMOTE_REVIEWS_INTERVAL`] (when not
//! checked out), batching all due pull requests into a single query.
//! Explicit user refreshes (`r`) query all open pull requests.
//!
//! When `JP_GITHUB_TOKEN` is set in the environment, it is passed to `gh` as
//! `GH_TOKEN` so pending review comments authored by that account are visible.
//!
//! Cached REST bodies, `ETag`s, review counts, and the `owner/name` repository
//! identifier are persisted per repository under `$XDG_CACHE_HOME/jp/dash/`
//! (`~/.cache/jp/dash/` when unset), allowing the dashboard to render cached PR
//! state immediately on startup.
//!
//! Pull requests carrying the queue label (`dash.queueLabel` in git config,
//! defaulting to [`DEFAULT_QUEUE_LABEL`]) are placed in the merge queue by
//! [`queue::assign`], and [`merge`] advances the front entry on each poll.
//! Checks required by the default branch's rulesets count as pending until they
//! report.
//! Open GitHub stacks populate each member's [`StackPlace`]; if the stacks
//! endpoint returns `404`, subsequent polls skip it.
//!
//! [`queue::assign`]: crate::queue::assign

use std::{
    collections::{HashMap, HashSet},
    env,
    process::Command,
    sync::mpsc::{self, Sender},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

mod merge;

pub use self::merge::{Poll, QueueControl};
use super::{Error, cache, failure, git, output, parallel, stdout};
use crate::{
    model::{Checks, PullRequest, StackPlace},
    queue::{self, Entry, Hold},
};

/// Default merge-queue label when `dash.queueLabel` is unset in git config.
pub const DEFAULT_QUEUE_LABEL: &str = "merge-when-green";

const QUEUE_LABEL_KEY: &str = "dash.queueLabel";

/// Maximum concurrent REST API requests per poll.
const PARALLEL: usize = 8;

/// Review-thread refresh interval for pull requests with a local worktree.
const LOCAL_REVIEWS_INTERVAL: Duration = Duration::from_mins(3);

/// Review-thread refresh interval for pull requests without a local worktree.
const REMOTE_REVIEWS_INTERVAL: Duration = Duration::from_mins(15);

/// Maximum review threads fetched per pull request in [`reviews_query`] to
/// bound GraphQL query cost.
/// Threads beyond this limit are counted in `totalCount` but not inspected for
/// replies.
const THREADS: u32 = 30;

const NOT_MODIFIED: u16 = 304;
const SUCCESS_FIRST: u16 = 200;
const SUCCESS_LAST: u16 = 299;

/// Maximum characters of a raw REST error body included when it has no JSON
/// `message` field.
const ERROR_BODY_CHARS: usize = 80;

/// Map from PR number to `(total_threads, answered_threads)`.
type Counts = HashMap<u64, (u32, u32)>;

/// Map from PR number to cached review-thread counts.
type Reviews = HashMap<u64, Review>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Review {
    threads: u32,
    answered: u32,

    /// Unix timestamp (in seconds) when the review threads were last fetched.
    read_at: u64,
}

/// Client for polling open pull requests, checks, and review threads for the
/// repository containing `dir`.
pub struct GitHub {
    dir: Utf8PathBuf,

    /// The label that puts a pull request in the merge queue.
    label: String,

    /// On-disk cache path, or `None` if the cache directory or git common
    /// directory could not be resolved.
    file: Option<Utf8PathBuf>,

    state: State,

    /// Whether `state` has been modified since it was last written to `file`.
    dirty: bool,

    /// REST endpoint paths requested during the current poll.
    /// Cached entries for paths not in this set (such as checks for superseded
    /// head commits) are pruned at the end of the poll.
    requested: HashSet<String>,

    /// Default branch of the repository, taken from the most recent non-empty
    /// pull request list.
    default_branch: Option<String>,

    /// Prefetched REST responses for the current poll, keyed by endpoint path
    /// and consumed by [`fetch`].
    ///
    /// [`fetch`]: Self::fetch
    prefetched: HashMap<String, Result<Reply, Error>>,

    /// Whether to query the repository's stacks endpoint (`false` after a `404`
    /// response).
    stacks: bool,

    driver: merge::Driver,
}

/// Cached GitHub state persisted across polls and dashboard runs.
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct State {
    /// Repository `"owner/name"`, resolved lazily on the first online poll.
    repo: Option<String>,

    /// Cached HTTP `200` body and `ETag` per REST endpoint path, reused on
    /// `304`.
    rest: HashMap<String, Cached>,

    /// Cached review-thread counts per open pull request.
    reviews: Reviews,

    /// Persisted holds on merge-queue entries, keyed by [`Entry::key`], so
    /// restarting `dash` does not retry held entries.
    #[serde(default)]
    holds: HashMap<u64, Hold>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Cached {
    etag: String,
    body: String,
}

/// Whether a poll may make network calls.
#[derive(Clone, Copy)]
enum Mode {
    /// Fetch from GitHub; `asked` is `true` on an explicit user refresh.
    Online { asked: bool },

    /// Read from the in-memory cache only.
    Offline,
}

impl GitHub {
    /// Create a [`GitHub`] reader for `dir`, loading any persisted cache from
    /// disk.
    #[must_use]
    pub fn open(dir: Utf8PathBuf) -> Self {
        let file = cache::file(&dir, "github");
        let state: State = file.as_deref().map(cache::load).unwrap_or_default();
        let driver = merge::Driver::new(state.holds.clone());

        Self {
            label: queue_label(&dir),
            dir,
            file,
            state,
            dirty: false,
            requested: HashSet::new(),
            default_branch: None,
            prefetched: HashMap::new(),
            stacks: true,
            driver,
        }
    }

    /// Build the pull request list from cached responses without making network
    /// requests.
    ///
    /// Returns `None` if the repository name or pull request list is not
    /// cached.
    /// Pull requests without cached check responses are omitted until the first
    /// network poll.
    pub fn cached(&mut self) -> Option<Vec<PullRequest>> {
        self.read(Mode::Offline).ok()
    }

    /// Poll open pull requests, move the merge queue forward unless `control`
    /// pauses it, and persist any updated cache state.
    ///
    /// When `asked` is `true`, forces a refresh of all review threads.
    pub fn pull_requests(&mut self, asked: bool, control: &QueueControl) -> Poll {
        self.requested.clear();
        self.driver.forget(&control.take_cleared());

        let mut poll = Poll::default();
        match self.read(Mode::Online { asked }) {
            Ok(mut prs) => {
                let mut driver = std::mem::take(&mut self.driver);
                poll.role = driver.drive(self, control.paused(), &mut prs, &mut poll);
                self.driver = driver;
                poll.prs = Ok(prs);
            }
            Err(error) => poll.prs = Err(error),
        }

        // Discard any prefetched responses left unconsumed by an early error.
        self.prefetched.clear();
        self.keep_holds();
        self.prune();
        self.persist();
        poll
    }

    /// Sync the driver's holds into `self.state` and mark the cache dirty if
    /// they changed.
    fn keep_holds(&mut self) {
        if self.state.holds != self.driver.holds {
            self.state.holds.clone_from(&self.driver.holds);
            self.dirty = true;
        }
    }

    /// Drop cached REST responses no request asked for during this poll.
    fn prune(&mut self) {
        let before = self.state.rest.len();
        self.state
            .rest
            .retain(|path, _| self.requested.contains(path));
        self.dirty |= self.state.rest.len() != before;
    }

    fn read(&mut self, mode: Mode) -> Result<Vec<PullRequest>, Error> {
        let online = matches!(mode, Mode::Online { .. });
        let repo = self.repo(online)?;
        let branches: HashSet<String> = git::worktrees(&self.dir)?
            .into_iter()
            .filter_map(|worktree| worktree.branch)
            .collect();
        let mut local = HashSet::new();

        let pulls: Vec<RestPull> = parse_json(&self.get(
            &format!("repos/{repo}/pulls?state=open&per_page=100"),
            online,
        )?)?;

        if let Some(branch) = pulls
            .iter()
            .map(|pull| &pull.base.repo.default_branch)
            .find(|branch| !branch.is_empty())
        {
            self.default_branch = Some(branch.clone());
        }

        if online {
            let mut paths: Vec<String> = pulls
                .iter()
                .flat_map(|pull| {
                    [
                        runs_path(&repo, &pull.head.sha),
                        status_path(&repo, &pull.head.sha),
                    ]
                })
                .collect();
            if let Some(branch) = &self.default_branch {
                paths.push(rules_path(&repo, branch));
            }
            if self.stacks {
                paths.push(stacks_path(&repo));
            }
            self.prefetch(paths);
        }
        let required = self.required(&repo, online);
        let mut places = self.places(&repo, online);

        let mut prs = vec![];
        for pull in pulls {
            let head = local_branch(&pull);
            if branches.contains(&head) {
                local.insert(pull.number);
            }

            let place = places.remove(&pull.number);
            if let Some(pr) = self.pull_request(&repo, pull, head, place, &required, online)? {
                prs.push(pr);
            }
        }
        prs.sort_by_key(|pr| pr.number);

        if let Mode::Online { asked } = mode {
            let open: HashSet<u64> = prs.iter().map(|pr| pr.number).collect();
            let before = self.state.reviews.len();
            self.state.reviews.retain(|number, _| open.contains(number));
            self.dirty |= self.state.reviews.len() != before;

            let now = now();
            let due = due_reviews(&prs, &local, &self.state.reviews, now, asked);
            if !due.is_empty() {
                self.read_reviews(&repo, &due, now)?;
            }
        }

        for pr in &mut prs {
            if let Some(review) = self.state.reviews.get(&pr.number) {
                pr.threads = review.threads;
                pr.answered = review.answered;
            }
        }
        self.read_mergeable(&repo, &mut prs, online);
        queue::assign(&mut prs, self.driver.front, &self.driver.holds);

        Ok(prs)
    }

    /// Populate `mergeable` for every pull request in a queue entry so
    /// conflicts are visible at any queue position.
    ///
    /// If the detail request fails, `mergeable` remains `None`.
    fn read_mergeable(&mut self, repo: &str, prs: &mut [PullRequest], online: bool) {
        let queued: Vec<u64> = queue::entries(prs)
            .iter()
            .flat_map(Entry::numbers)
            .collect();
        let paths: Vec<String> = queued
            .iter()
            .map(|number| detail_path(repo, *number))
            .collect();
        if online {
            self.prefetch(paths.clone());
        }

        for (number, path) in queued.iter().zip(&paths) {
            let mergeable = self
                .fetch(path, online, slim::<Detail>)
                .and_then(|body| parse_json::<Detail>(&body))
                .ok()
                .and_then(|detail| detail.mergeable);
            if let Some(pr) = prs.iter_mut().find(|pr| pr.number == *number) {
                pr.mergeable = mergeable;
            }
        }
    }

    /// Return each stacked pull request's `(StackPlace, stack_base_branch)`
    /// keyed by PR number.
    ///
    /// Returns an empty map when stacks are disabled or the request fails;
    /// stacked pull requests then retain their individual base branches and are
    /// skipped by the queue as `OtherBase`.
    fn places(&mut self, repo: &str, online: bool) -> HashMap<u64, (StackPlace, String)> {
        if !self.stacks {
            return HashMap::new();
        }
        match self.get(&stacks_path(repo), online) {
            Ok(body) => parse_stacks(&body).unwrap_or_default(),
            Err(Error::Failed { message, .. }) if message.starts_with("HTTP 404") => {
                self.stacks = false;
                HashMap::new()
            }
            Err(_) => HashMap::new(),
        }
    }

    /// Return the status-check contexts required by the default branch's
    /// rulesets, or an empty list if rulesets are unavailable or not cached.
    fn required(&mut self, repo: &str, online: bool) -> Vec<String> {
        let Some(branch) = self.default_branch.clone() else {
            return vec![];
        };
        self.get(&rules_path(repo, &branch), online)
            .and_then(|body| parse_required(&body))
            .unwrap_or_default()
    }

    /// Fetch check runs and commit statuses for `pull`'s head commit and build
    /// a [`PullRequest`] (with review thread counts left at zero).
    ///
    /// Returns `Ok(None)` in offline mode when the commit's checks are not
    /// cached.
    fn pull_request(
        &mut self,
        repo: &str,
        pull: RestPull,
        head: String,
        place: Option<(StackPlace, String)>,
        required: &[String],
        online: bool,
    ) -> Result<Option<PullRequest>, Error> {
        let runs = self.get(&runs_path(repo, &pull.head.sha), online);
        let status = self.get(&status_path(repo, &pull.head.sha), online);
        let (runs, status) = match (runs, status) {
            (Ok(runs), Ok(status)) => (runs, status),
            (Err(Error::NotCached(_)), _) | (_, Err(Error::NotCached(_))) => return Ok(None),
            (Err(error), _) | (_, Err(error)) => return Err(error),
        };

        let labeled = pull.labels.iter().any(|label| label.name == self.label);
        let remote_branch = pull
            .head
            .repo
            .as_ref()
            .filter(|repo| repo.full_name == pull.base.repo.full_name)
            .map(|_| pull.head.r#ref.clone());
        let (stack, target) = match place {
            Some((place, base)) => (
                Some(StackPlace {
                    branch: pull.head.r#ref.clone(),
                    ..place
                }),
                base,
            ),
            None => (None, pull.base.r#ref),
        };
        let base =
            Some(target).filter(|base| !base.is_empty() && *base != pull.base.repo.default_branch);

        Ok(Some(PullRequest {
            number: pull.number,
            title: pull.title,
            url: pull.html_url,
            head,
            sha: pull.head.sha,
            draft: pull.draft,
            checks: checks(contexts(&runs, &status)?, required),
            threads: 0,
            answered: 0,
            labeled,
            base,
            stack,
            remote_branch,
            mergeable: None,
            queue: None,
        }))
    }

    /// Fetch and update review-thread counts for `numbers` (must be non-empty).
    fn read_reviews(&mut self, repo: &str, numbers: &[u64], now: u64) -> Result<(), Error> {
        let counts = parse_reviews(&self.graphql(repo, &reviews_query(numbers))?)?;

        for number in numbers {
            match counts.get(number) {
                Some(&(threads, answered)) => {
                    self.state.reviews.insert(*number, Review {
                        threads,
                        answered,
                        read_at: now,
                    });
                }
                // Closed since the pull request list was read.
                None => {
                    self.state.reviews.remove(number);
                }
            }
        }
        self.dirty = true;
        Ok(())
    }

    /// Write `state` to disk if modified since the last save.
    fn persist(&mut self) {
        let Some(file) = &self.file else {
            return;
        };

        // Ignore cache write errors; a failed write only prevents the next run
        // from starting warm.
        if self.dirty && cache::store(file, &self.state).is_ok() {
            self.dirty = false;
        }
    }

    fn repo(&mut self, online: bool) -> Result<String, Error> {
        if let Some(repo) = &self.state.repo {
            return Ok(repo.clone());
        }
        if !online {
            return Err(Error::NotCached("the repository name".to_owned()));
        }

        let repo = stdout(
            "gh",
            self.gh().args([
                "repo",
                "view",
                "--json",
                "nameWithOwner",
                "--jq",
                ".nameWithOwner",
            ]),
        )?
        .trim()
        .to_owned();
        self.state.repo = Some(repo.clone());
        self.dirty = true;
        Ok(repo)
    }

    /// Fetch `path` from the GitHub REST API using `If-None-Match` when cached,
    /// or read from the cache alone when `online` is `false`.
    fn get(&mut self, path: &str, online: bool) -> Result<String, Error> {
        self.fetch(path, online, |body| Ok(body.to_owned()))
    }

    /// Fetch `path` from the GitHub REST API (or read from the cache when
    /// `online` is `false`), transforming `200` bodies with `keep` before
    /// caching and returning them.
    fn fetch(
        &mut self,
        path: &str,
        online: bool,
        keep: fn(&str) -> Result<String, Error>,
    ) -> Result<String, Error> {
        self.requested.insert(path.to_owned());

        if !online {
            return self
                .state
                .rest
                .get(path)
                .map(|cached| cached.body.clone())
                .ok_or_else(|| Error::NotCached(path.to_owned()));
        }

        let reply = match self.prefetched.remove(path) {
            Some(reply) => reply,
            None => call(&self.dir, path, self.etag(path)),
        }?;

        match reply.status {
            NOT_MODIFIED => self
                .state
                .rest
                .get(path)
                .map(|cached| cached.body.clone())
                .ok_or_else(|| Error::Failed {
                    program: "gh",
                    message: format!("{path}: 304 for a response never fetched"),
                }),
            SUCCESS_FIRST..=SUCCESS_LAST => {
                let body = keep(&reply.body)?;
                if let Some(etag) = reply.etag {
                    self.state.rest.insert(path.to_owned(), Cached {
                        etag,
                        body: body.clone(),
                    });
                    self.dirty = true;
                }
                Ok(body)
            }
            status => Err(Error::Failed {
                program: "gh",
                message: format!("HTTP {status}: {}", api_message(&reply.body)),
            }),
        }
    }

    /// The `ETag` of the cached response for `path`, if any.
    fn etag(&self, path: &str) -> Option<&str> {
        self.state.rest.get(path).map(|cached| cached.etag.as_str())
    }

    /// Fetch `paths` concurrently (up to [`PARALLEL`] at a time) and store the
    /// replies in `self.prefetched` for subsequent [`fetch`] calls.
    ///
    /// [`fetch`]: Self::fetch
    fn prefetch(&mut self, paths: Vec<String>) {
        let mut seen = HashSet::new();
        let jobs: Vec<(String, Option<String>)> = paths
            .into_iter()
            .filter(|path| !self.prefetched.contains_key(path) && seen.insert(path.clone()))
            .map(|path| {
                let etag = self.etag(&path).map(str::to_owned);
                (path, etag)
            })
            .collect();

        let dir = &self.dir;
        let replies = parallel(&jobs, PARALLEL, |(path, etag)| {
            call(dir, path, etag.as_deref())
        });
        self.prefetched
            .extend(jobs.into_iter().map(|(path, _)| path).zip(replies));
    }

    fn graphql(&self, repo: &str, query: &str) -> Result<String, Error> {
        let (owner, name) = repo.split_once('/').unwrap_or((repo, ""));
        stdout(
            "gh",
            self.gh()
                .args(["api", "graphql", "-f"])
                .arg(format!("query={query}"))
                .args([
                    "-F",
                    &format!("owner={owner}"),
                    "-F",
                    &format!("name={name}"),
                ]),
        )
    }

    fn gh(&self) -> Command {
        gh(&self.dir)
    }
}

/// A REST response as `gh api --include` printed it.
#[derive(Debug)]
struct Reply {
    status: u16,
    etag: Option<String>,
    body: String,
}

/// Request `path` from the GitHub REST API, sending `etag` as `If-None-Match`
/// when given.
fn call(dir: &Utf8Path, path: &str, etag: Option<&str>) -> Result<Reply, Error> {
    let mut command = gh(dir);
    command.args(["api", "--include", path]);
    if let Some(etag) = etag {
        command
            .arg("--header")
            .arg(format!("If-None-Match: {etag}"));
    }

    // `gh` exits 1 on every status outside 2xx, a `304` included, so the status
    // line decides what happened.
    let out = output("gh", &mut command)?;
    let raw = String::from_utf8_lossy(&out.stdout);
    let Some(response) = parse_response(&raw) else {
        return Err(failure("gh", &out.stderr));
    };

    Ok(Reply {
        status: response.status,
        etag: response.etag.map(str::to_owned),
        body: response.body.to_owned(),
    })
}

fn runs_path(repo: &str, sha: &str) -> String {
    format!("repos/{repo}/commits/{sha}/check-runs?per_page=100")
}

fn status_path(repo: &str, sha: &str) -> String {
    format!("repos/{repo}/commits/{sha}/status?per_page=100")
}

fn rules_path(repo: &str, branch: &str) -> String {
    format!("repos/{repo}/rules/branches/{branch}")
}

fn stacks_path(repo: &str) -> String {
    format!("repos/{repo}/stacks?per_page=100")
}

fn detail_path(repo: &str, number: u64) -> String {
    format!("repos/{repo}/pulls/{number}")
}

/// Subset of a single pull request REST response used by the merge queue.
#[derive(Debug, Serialize, Deserialize)]
struct Detail {
    /// `None` while GitHub is still computing mergeability.
    mergeable: Option<bool>,
}

/// Strip a JSON response body down to the fields of `T` before caching (for
/// example, discarding the file diffs in a compare response).
fn slim<T: Serialize + DeserializeOwned>(body: &str) -> Result<String, Error> {
    serde_json::to_string(&parse_json::<T>(body)?).map_err(|source| Error::Parse {
        program: "gh",
        source,
    })
}

#[derive(Deserialize)]
struct RestStack {
    number: u64,
    #[serde(default)]
    open: bool,
    base: StackBase,
    pull_requests: Vec<StackMember>,
}

#[derive(Deserialize)]
struct StackBase {
    r#ref: String,
}

#[derive(Deserialize)]
struct StackMember {
    number: u64,
    head: StackHead,
}

#[derive(Deserialize)]
struct StackHead {
    sha: String,
}

/// Parse the stacks list into each member's place in its open stack, and the
/// branch that stack merges into, by pull request number.
///
/// [`StackPlace::branch`] is left empty for the caller to fill in.
fn parse_stacks(json: &str) -> Result<HashMap<u64, (StackPlace, String)>, Error> {
    let stacks: Vec<RestStack> = parse_json(json)?;

    let mut places = HashMap::new();
    for stack in stacks.into_iter().filter(|stack| stack.open) {
        let size = stack.pull_requests.len();
        for (index, member) in stack.pull_requests.iter().enumerate() {
            let below = index
                .checked_sub(1)
                .map(|below| stack.pull_requests[below].head.sha.clone());
            places.insert(
                member.number,
                (
                    StackPlace {
                        stack: stack.number,
                        position: index + 1,
                        size,
                        branch: String::new(),
                        below,
                    },
                    stack.base.r#ref.clone(),
                ),
            );
        }
    }
    Ok(places)
}

/// Build a `gh` [`Command`] rooted at `dir`, forwarding `JP_GITHUB_TOKEN` as
/// `GH_TOKEN` if set.
fn gh(dir: &Utf8Path) -> Command {
    let mut command = Command::new("gh");
    command.current_dir(dir);

    if let Ok(token) = env::var("JP_GITHUB_TOKEN")
        && !token.is_empty()
    {
        command.env("GH_TOKEN", token);
    }
    command
}

/// Check out pull request `number` in `worktree` via `gh pr checkout` with all
/// interactive prompts disabled.
pub fn checkout(worktree: &Utf8Path, number: u64) -> Result<(), Error> {
    let mut command = gh(worktree);
    command
        .args(["pr", "checkout", &number.to_string()])
        .env("GH_PROMPT_DISABLED", "1");
    stdout("gh", git::noninteractive(&mut command, worktree)).map(drop)
}

/// Return the merge-queue label for the repository containing `dir`
/// (`dash.queueLabel` in git config, or [`DEFAULT_QUEUE_LABEL`]).
#[must_use]
pub fn queue_label(dir: &Utf8Path) -> String {
    git::config(dir, QUEUE_LABEL_KEY).unwrap_or_else(|| DEFAULT_QUEUE_LABEL.to_owned())
}

/// Add (`add: true`) or remove (`add: false`) the merge-queue label on pull
/// request `number`, creating the label in the repository first if needed.
pub fn set_queued(dir: &Utf8Path, number: u64, add: bool) -> Result<(), Error> {
    let label = queue_label(dir);
    let edit = || {
        let mut command = gh(dir);
        command
            .args(["pr", "edit", &number.to_string()])
            .arg(if add { "--add-label" } else { "--remove-label" })
            .arg(&label)
            .env("GH_PROMPT_DISABLED", "1");
        stdout("gh", git::noninteractive(&mut command, dir)).map(drop)
    };

    match edit() {
        Err(_) if add => {
            // `gh pr edit --add-label` fails if the label does not exist in the
            // repository yet; create it (ignoring errors if it already exists)
            // and retry so the retry surfaces any underlying error.
            let mut create = gh(dir);
            create
                .args([
                    "label",
                    "create",
                    &label,
                    "--color",
                    "0e8a16",
                    "--description",
                ])
                .arg("Merged by dash once its checks pass")
                .env("GH_PROMPT_DISABLED", "1");
            let _exists = stdout("gh", git::noninteractive(&mut create, dir));
            edit()
        }
        result => result,
    }
}

/// Spawn a background worker that applies `(pr_number, add)` queue-label edits
/// sequentially and invokes `done` with each outcome.
pub fn label_editor<D>(dir: Utf8PathBuf, done: D) -> Sender<(u64, bool)>
where
    D: Fn(u64, bool, Result<(), String>) + Send + 'static,
{
    editor(
        move |number, add| set_queued(&dir, number, add).map_err(|error| error.to_string()),
        done,
    )
}

fn editor<E, D>(edit: E, done: D) -> Sender<(u64, bool)>
where
    E: Fn(u64, bool) -> Result<(), String> + Send + 'static,
    D: Fn(u64, bool, Result<(), String>) + Send + 'static,
{
    let (send, edits) = mpsc::channel();

    // Process label edits on a single thread so rapid toggles (`m` twice)
    // execute in order instead of racing on GitHub.
    thread::spawn(move || {
        for (number, add) in edits {
            done(number, add, edit(number, add));
        }
    });

    send
}

/// Return PR numbers whose review threads need refreshing: all PRs when `asked`
/// is true, or those with missing or expired review cache entries.
fn due_reviews(
    prs: &[PullRequest],
    local: &HashSet<u64>,
    reviews: &Reviews,
    now: u64,
    asked: bool,
) -> Vec<u64> {
    prs.iter()
        .map(|pr| pr.number)
        .filter(|number| {
            asked
                || reviews.get(number).is_none_or(|review| {
                    let interval = if local.contains(number) {
                        LOCAL_REVIEWS_INTERVAL
                    } else {
                        REMOTE_REVIEWS_INTERVAL
                    };
                    now.saturating_sub(review.read_at) >= interval.as_secs()
                })
        })
        .collect()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Parsed output of `gh api --include` (status code, optional `ETag`, and
/// response body).
#[derive(Debug, PartialEq, Eq)]
struct Response<'a> {
    status: u16,
    etag: Option<&'a str>,
    body: &'a str,
}

/// Parse `gh api --include` output, returning `None` if stdout does not begin
/// with an HTTP status line.
fn parse_response(raw: &str) -> Option<Response<'_>> {
    let (head, body) = raw
        .split_once("\r\n\r\n")
        .or_else(|| raw.split_once("\n\n"))
        .unwrap_or((raw, ""));

    let mut lines = head.lines();
    let status = lines
        .next()?
        .strip_prefix("HTTP/")?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;

    let etag = lines.find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("etag")
            .then(|| value.trim())
    });

    Some(Response { status, etag, body })
}

/// Extract the `"message"` field from a JSON error body, or truncate the raw
/// body to [`ERROR_BODY_CHARS`].
fn api_message(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| Some(value.get("message")?.as_str()?.to_owned()))
        .unwrap_or_else(|| body.chars().take(ERROR_BODY_CHARS).collect())
}

fn parse_json<T: DeserializeOwned>(json: &str) -> Result<T, Error> {
    serde_json::from_str(json).map_err(|source| Error::Parse {
        program: "gh",
        source,
    })
}

#[derive(Deserialize)]
struct RestPull {
    number: u64,
    title: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    head: Head,
    base: Base,
    #[serde(default)]
    labels: Vec<Label>,
}

#[derive(Deserialize)]
struct Label {
    name: String,
}

#[derive(Deserialize)]
struct Head {
    r#ref: String,
    sha: String,

    /// `None` if the author's user account has been deleted.
    user: Option<User>,

    /// `None` if the source fork repository has been deleted.
    repo: Option<Repo>,
}

#[derive(Deserialize)]
struct Base {
    #[serde(default)]
    r#ref: String,
    repo: Repo,
}

#[derive(Deserialize)]
struct User {
    login: String,
}

#[derive(Deserialize)]
struct Repo {
    full_name: String,
    #[serde(default)]
    default_branch: String,
}

/// Determine the local branch name `gh pr checkout` uses for `pull`.
///
/// Returns the head ref unless `pull` comes from a fork whose head ref matches
/// the base repository's default branch, in which case `gh` prefixes it with
/// `<owner>/` to avoid colliding with the local default branch.
fn local_branch(pull: &RestPull) -> String {
    let head = &pull.head;
    let base = &pull.base.repo;
    let foreign = head
        .repo
        .as_ref()
        .is_none_or(|repo| repo.full_name != base.full_name);

    match &head.user {
        Some(user) if foreign && head.r#ref == base.default_branch => {
            format!("{}/{}", user.login, head.r#ref)
        }
        _ => head.r#ref.clone(),
    }
}

#[derive(Deserialize)]
struct CheckRuns {
    check_runs: Vec<CheckRun>,
}

#[derive(Deserialize)]
struct CheckRun {
    name: String,
    status: String,
    conclusion: Option<String>,
    started_at: Option<String>,
}

#[derive(Deserialize)]
struct CombinedStatus {
    statuses: Vec<Status>,
}

#[derive(Deserialize)]
struct Status {
    context: String,
    state: String,
}

/// A CI check on a commit: either a GitHub Actions / App check run or an
/// external commit status context (with uppercase status and conclusion
/// strings).
#[derive(Debug)]
enum Context {
    CheckRun {
        name: String,
        status: String,
        conclusion: Option<String>,
        started_at: Option<String>,
    },
    StatusContext {
        context: String,
        state: String,
    },
}

impl Context {
    /// Convert into `(name, started_at, outcome)`.
    /// `started_at` is `None` for commit statuses and queued check runs.
    fn into_attempt(self) -> (String, Option<String>, Outcome) {
        match self {
            Self::CheckRun {
                name,
                status,
                conclusion,
                started_at,
            } => {
                let outcome = if status != "COMPLETED" {
                    Outcome::Pending
                } else if matches!(
                    conclusion.as_deref(),
                    Some("SUCCESS" | "NEUTRAL" | "SKIPPED")
                ) {
                    Outcome::Passed
                } else {
                    Outcome::Failed
                };
                (name, started_at, outcome)
            }
            Self::StatusContext { context, state } => {
                let outcome = match state.as_str() {
                    "SUCCESS" => Outcome::Passed,
                    "PENDING" | "EXPECTED" => Outcome::Pending,
                    _ => Outcome::Failed,
                };
                (context, None, outcome)
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Outcome {
    Passed,
    Pending,
    Failed,
}

/// Parse and combine a commit's `check-runs` and `status` JSON payloads.
fn contexts(runs: &str, status: &str) -> Result<Vec<Context>, Error> {
    let runs: CheckRuns = parse_json(runs)?;
    let status: CombinedStatus = parse_json(status)?;

    let runs = runs.check_runs.into_iter().map(|run| Context::CheckRun {
        name: run.name,
        status: run.status.to_ascii_uppercase(),
        conclusion: run.conclusion.map(|c| c.to_ascii_uppercase()),
        started_at: run.started_at,
    });
    let statuses = status
        .statuses
        .into_iter()
        .map(|status| Context::StatusContext {
            context: status.context,
            state: status.state.to_ascii_uppercase(),
        });

    Ok(runs.chain(statuses).collect())
}

/// Build a GraphQL query fetching the review threads for each PR in `numbers`.
///
/// Requests up to 2 comments per thread, which is sufficient to determine
/// whether the thread has at least one reply.
fn reviews_query(numbers: &[u64]) -> String {
    let mut query = String::from(
        "query($owner: String!, $name: String!) {\n  repository(owner: $owner, name: $name) {\n",
    );
    for number in numbers {
        query.push_str(&format!(
            "    pr{number}: pullRequest(number: {number}) {{ reviewThreads(first: {THREADS}) {{ \
             totalCount nodes {{ comments(first: 2) {{ nodes {{ id }} }} }} }} }}\n"
        ));
    }
    query.push_str("  }\n}");
    query
}

#[derive(Deserialize)]
struct ReviewsResponse {
    data: ReviewsData,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewsData {
    /// Keyed by the `pr<number>` alias; `null` if the pull request no longer
    /// exists.
    repository: HashMap<String, Option<ReviewedPullRequest>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewedPullRequest {
    review_threads: Threads,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Threads {
    /// Total threads on the pull request, including any beyond [`THREADS`].
    total_count: u32,
    nodes: Vec<Thread>,
}

#[derive(Deserialize)]
struct Nodes<T> {
    nodes: Vec<T>,
}

#[derive(Deserialize)]
struct Thread {
    comments: Nodes<serde_json::Value>,
}

/// Parse the GraphQL response from [`reviews_query`] into total and answered
/// thread counts per PR.
fn parse_reviews(json: &str) -> Result<Counts, Error> {
    let response: ReviewsResponse = parse_json(json)?;

    let reviews = response
        .data
        .repository
        .into_iter()
        .filter_map(|(alias, pr)| {
            let number = alias.strip_prefix("pr")?.parse().ok()?;
            let threads = pr?.review_threads;
            let answered = threads
                .nodes
                .iter()
                .filter(|thread| thread.comments.nodes.len() > 1)
                .count();
            Some((number, (threads.total_count, count(answered))))
        })
        .collect();

    Ok(reviews)
}

#[derive(Deserialize)]
struct Rule {
    r#type: String,
    parameters: Option<RuleParameters>,
}

#[derive(Deserialize)]
struct RuleParameters {
    #[serde(default)]
    required_status_checks: Vec<RequiredCheck>,
}

#[derive(Deserialize)]
struct RequiredCheck {
    context: String,
}

/// Parse the `rules/branches/{branch}` response into the names of the checks it
/// requires.
fn parse_required(json: &str) -> Result<Vec<String>, Error> {
    let rules: Vec<Rule> = parse_json(json)?;

    Ok(rules
        .into_iter()
        .filter(|rule| rule.r#type == "required_status_checks")
        .filter_map(|rule| rule.parameters)
        .flat_map(|parameters| parameters.required_status_checks)
        .map(|check| check.context)
        .collect())
}

/// Aggregate check outcomes by check name, keeping only the latest attempt when
/// a check has been re-run, and counting each `required` check that hasn't
/// reported as pending.
///
/// A queued check run with `started_at: None` is treated as newer than any
/// previously started attempt.
fn checks(contexts: Vec<Context>, required: &[String]) -> Checks {
    let mut latest: HashMap<String, (Option<String>, Outcome)> = HashMap::new();

    for context in contexts {
        let (name, started_at, outcome) = context.into_attempt();
        let newer = latest
            .get(&name)
            .is_none_or(|(seen, _)| match (seen, &started_at) {
                (None, _) => false,
                (Some(_), None) => true,
                (Some(seen), Some(started)) => started > seen,
            });
        if newer {
            latest.insert(name, (started_at, outcome));
        }
    }

    let missing = required
        .iter()
        .filter(|name| !latest.contains_key(*name))
        .collect::<HashSet<_>>()
        .len();

    let unreported = Checks {
        pending: count(missing),
        ..Checks::default()
    };
    latest
        .into_values()
        .fold(unreported, |mut checks, (_, outcome)| {
            match outcome {
                Outcome::Passed => checks.passed += 1,
                Outcome::Pending => checks.pending += 1,
                Outcome::Failed => checks.failed += 1,
            }
            checks
        })
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

#[cfg(test)]
#[path = "github_tests.rs"]
mod tests;
