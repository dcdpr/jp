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

use std::{
    collections::{HashMap, HashSet},
    env,
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use super::{Error, cache, failure, git, output, stdout};
use crate::model::{Checks, PullRequest};

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
        let state = file.as_deref().map(cache::load).unwrap_or_default();

        Self {
            dir,
            file,
            state,
            dirty: false,
            requested: HashSet::new(),
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

    /// Poll open pull requests and persist any updated cache state.
    ///
    /// When `asked` is `true`, forces a refresh of all review threads.
    pub fn pull_requests(&mut self, asked: bool) -> Result<Vec<PullRequest>, Error> {
        let read = self.read(Mode::Online { asked });
        self.persist();
        read
    }

    fn read(&mut self, mode: Mode) -> Result<Vec<PullRequest>, Error> {
        let online = matches!(mode, Mode::Online { .. });
        let repo = self.repo(online)?;
        let branches: HashSet<String> = git::worktrees(&self.dir)?
            .into_iter()
            .filter_map(|worktree| worktree.branch)
            .collect();
        let mut local = HashSet::new();

        self.requested.clear();
        let pulls: Vec<RestPull> = parse_json(&self.get(
            &format!("repos/{repo}/pulls?state=open&per_page=100"),
            online,
        )?)?;

        let mut prs = vec![];
        for pull in pulls {
            let head = local_branch(&pull);
            if branches.contains(&head) {
                local.insert(pull.number);
            }

            if let Some(pr) = self.pull_request(&repo, pull, head, online)? {
                prs.push(pr);
            }
        }
        let before = self.state.rest.len();
        self.state
            .rest
            .retain(|path, _| self.requested.contains(path));
        self.dirty |= self.state.rest.len() != before;
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

        Ok(prs)
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
        online: bool,
    ) -> Result<Option<PullRequest>, Error> {
        let sha = &pull.head.sha;
        let runs = self.get(
            &format!("repos/{repo}/commits/{sha}/check-runs?per_page=100"),
            online,
        );
        let status = self.get(
            &format!("repos/{repo}/commits/{sha}/status?per_page=100"),
            online,
        );
        let (runs, status) = match (runs, status) {
            (Ok(runs), Ok(status)) => (runs, status),
            (Err(Error::NotCached(_)), _) | (_, Err(Error::NotCached(_))) => return Ok(None),
            (Err(error), _) | (_, Err(error)) => return Err(error),
        };

        Ok(Some(PullRequest {
            number: pull.number,
            title: pull.title,
            url: pull.html_url,
            head,
            draft: pull.draft,
            checks: checks(contexts(&runs, &status)?),
            threads: 0,
            answered: 0,
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
        self.requested.insert(path.to_owned());

        if !online {
            return self
                .state
                .rest
                .get(path)
                .map(|cached| cached.body.clone())
                .ok_or_else(|| Error::NotCached(path.to_owned()));
        }

        let mut command = self.gh();
        command.args(["api", "--include", path]);
        if let Some(cached) = self.state.rest.get(path) {
            command
                .arg("--header")
                .arg(format!("If-None-Match: {}", cached.etag));
        }

        // `gh` exits 1 on every status outside 2xx, a `304` included, so the
        // status line decides what happened.
        let out = output("gh", &mut command)?;
        let raw = String::from_utf8_lossy(&out.stdout);
        let Some(response) = parse_response(&raw) else {
            return Err(failure("gh", &out.stderr));
        };

        match response.status {
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
                let body = response.body.to_owned();
                if let Some(etag) = response.etag {
                    self.state.rest.insert(path.to_owned(), Cached {
                        etag: etag.to_owned(),
                        body: body.clone(),
                    });
                    self.dirty = true;
                }
                Ok(body)
            }
            status => Err(Error::Failed {
                program: "gh",
                message: format!("HTTP {status}: {}", api_message(response.body)),
            }),
        }
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

/// Aggregate check outcomes by check name, keeping only the latest attempt when
/// a check has been re-run.
///
/// A queued check run with `started_at: None` is treated as newer than any
/// previously started attempt.
fn checks(contexts: Vec<Context>) -> Checks {
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

    latest
        .into_values()
        .fold(Checks::default(), |mut checks, (_, outcome)| {
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
