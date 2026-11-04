//! Open pull requests, their checks, and their review threads.
//!
//! Pull requests and checks come from the REST API as conditional requests:
//! each response's `ETag` goes back as `If-None-Match`, and a resource that has
//! not changed answers `304 Not Modified`, which GitHub does not count against
//! the rate limit.
//! Polling costs nothing until something changes; a running CI build costs a
//! point per poll while its checks move.
//!
//! Review threads come from GraphQL, because the REST API leaves out pending
//! review comments, and those are what `just pr-review` and `just pr-triage`
//! write.
//! GraphQL has no conditional requests and charges for every thread's comment
//! list, so each pull request's threads are read again only once they are older
//! than [`LOCAL_REVIEWS_INTERVAL`] (its branch has a local worktree) or
//! [`REMOTE_REVIEWS_INTERVAL`] (it has none), and one query covers every pull
//! request that is due.
//! Refreshing reads them all.
//!
//! Every call runs with `JP_GITHUB_TOKEN` when that is set, since pending
//! comments are visible only to the account that wrote them.
//!
//! The cached responses, their `ETag`s, the review counts, and the repository
//! name are kept on disk between runs, one file per repository under
//! `$XDG_CACHE_HOME/jp/dash/` (`~/.cache/jp/dash/` when that is unset).
//! A restart shows the last known state straight away, and its first reads are
//! conditional requests too, so a restart costs nothing when nothing changed.

use std::{
    collections::{HashMap, HashSet},
    env, fs, io,
    process::{self, Command},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use super::{Error, failure, git, output, stdout};
use crate::model::{Checks, PullRequest};

/// How old a pull request's review threads get before they are read again, when
/// its branch has a local worktree.
const LOCAL_REVIEWS_INTERVAL: Duration = Duration::from_mins(3);

/// The same, for a pull request with no local worktree: nobody is working on it
/// here, and there are more of them.
const REMOTE_REVIEWS_INTERVAL: Duration = Duration::from_mins(15);

/// Review threads counted per pull request.
/// GraphQL charges a request per thread, so this bounds the query's cost.
const THREADS: u32 = 30;

/// Pull request number to its review threads and how many have a reply.
type Counts = HashMap<u64, (u32, u32)>;

/// Pull request number to its last read review threads.
type Reviews = HashMap<u64, Review>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Review {
    threads: u32,
    answered: u32,

    /// When they were read, in seconds since the Unix epoch.
    read_at: u64,
}

/// Reads the pull requests of the GitHub repository a directory belongs to.
pub struct GitHub {
    dir: Utf8PathBuf,

    /// Where `state` is kept between runs.
    /// `None` when there is no cache directory, or the repository's git
    /// directory could not be found.
    file: Option<Utf8PathBuf>,

    state: State,

    /// Whether `state` changed since it was last written to `file`.
    dirty: bool,

    /// The REST paths this read has requested.
    /// Cached responses for any other path (the checks of a head commit that
    /// has since moved on) are dropped at the end of the read.
    requested: HashSet<String>,
}

/// What one read leaves for the next, in memory and on disk.
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct State {
    /// `owner/name`, looked up on the first successful read.
    repo: Option<String>,

    /// The last `200` response for each REST path, replayed on a `304`.
    rest: HashMap<String, Cached>,

    /// From the last review read of each pull request.
    reviews: Reviews,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Cached {
    etag: String,
    body: String,
}

/// Whether a read may use the network.
#[derive(Clone, Copy)]
enum Mode {
    /// Answer from the network; `asked` is whether the user asked for the read.
    Online { asked: bool },

    /// Answer from the cache alone.
    Offline,
}

impl GitHub {
    /// Read the repository containing `dir`, starting from what the last run
    /// left in the cache.
    #[must_use]
    pub fn open(dir: Utf8PathBuf) -> Self {
        let file = cache_file(&dir);
        let state = file.as_deref().map(load_state).unwrap_or_default();

        Self {
            dir,
            file,
            state,
            dirty: false,
            requested: HashSet::new(),
        }
    }

    /// The pull requests as the cache last saw them, without touching the
    /// network.
    /// `None` when the cache has nothing to show.
    ///
    /// A pull request whose checks are not cached (its branch got a worktree
    /// since the last run) is left out until the first network read.
    pub fn cached(&mut self) -> Option<Vec<PullRequest>> {
        self.read(Mode::Offline).ok()
    }

    /// List the open pull requests.
    ///
    /// `asked` is whether the user asked for this read, which also refreshes
    /// review threads.
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
            if branches.contains(&pull.head.r#ref) {
                local.insert(pull.number);
            }

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
                (Err(Error::NotCached(_)), _) | (_, Err(Error::NotCached(_))) => continue,
                (Err(error), _) | (_, Err(error)) => return Err(error),
            };

            prs.push(PullRequest {
                number: pull.number,
                title: pull.title,
                url: pull.html_url,
                head: pull.head.r#ref,
                draft: pull.draft,
                checks: checks(contexts(&runs, &status)?),
                threads: 0,
                answered: 0,
            });
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

    /// Read the review threads of the pull requests `numbers`, which is not
    /// empty.
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

    /// Write the state to the cache file when it changed.
    fn persist(&mut self) {
        let Some(file) = &self.file else {
            return;
        };

        // A cache that cannot be written costs the next start its head start
        // and nothing else; the read it follows succeeded or failed on its own,
        // so the failure is not reported.
        if self.dirty && store_state(file, &self.state).is_ok() {
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

    /// `GET` a REST path, as a conditional request when it was read before.
    ///
    /// Without `online`, answer from the cache alone.
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
            304 => self
                .state
                .rest
                .get(path)
                .map(|cached| cached.body.clone())
                .ok_or_else(|| Error::Failed {
                    program: "gh",
                    message: format!("{path}: 304 for a response never fetched"),
                }),
            200..=299 => {
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

/// A `gh` command running in `dir`, with `JP_GITHUB_TOKEN` when that is set.
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

/// Check pull request `number` out in the worktree at `worktree`, the way `gh
/// pr checkout` does: a pull request from a fork gets that fork as its branch's
/// upstream.
///
/// Neither `gh` nor `git` may ask anything: the dashboard owns the terminal,
/// and a prompt would draw over it and wait forever.
pub fn checkout(worktree: &Utf8Path, number: u64) -> Result<(), Error> {
    let mut command = gh(worktree);
    command
        .args(["pr", "checkout", &number.to_string()])
        .env("GH_PROMPT_DISABLED", "1");
    stdout("gh", git::noninteractive(&mut command, worktree)).map(drop)
}

/// The pull requests whose review threads should be read now: every one when
/// `asked`, otherwise those never read and those read longer ago than their
/// interval.
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

/// The cache file for the repository containing `dir`.
fn cache_file(dir: &Utf8Path) -> Option<Utf8PathBuf> {
    let home = cache_home(env::var("XDG_CACHE_HOME").ok(), env::var("HOME").ok())?;
    let repository = git::common_dir(dir).ok()?;
    Some(cache_path(&home, &repository))
}

/// The XDG cache directory: `$XDG_CACHE_HOME`, or `$HOME/.cache` when that is
/// unset or not absolute, as the XDG spec asks.
fn cache_home(xdg: Option<String>, home: Option<String>) -> Option<Utf8PathBuf> {
    xdg.map(Utf8PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            let home = home.filter(|home| !home.is_empty())?;
            Some(Utf8PathBuf::from(home).join(".cache"))
        })
}

/// The cache file under `home` for the repository whose common git directory is
/// `repository`.
/// Every worktree of a repository shares that directory, so they share the
/// file.
fn cache_path(home: &Utf8Path, repository: &Utf8Path) -> Utf8PathBuf {
    let name: String = repository
        .as_str()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();

    home.join("jp")
        .join("dash")
        .join(format!("{}.json", name.trim_matches('-')))
}

/// Read a cache file.
///
/// A missing, unreadable, or outdated file is the same as no cache: the next
/// read starts from nothing and writes a fresh one.
fn load_state(file: &Utf8Path) -> State {
    fs::read_to_string(file)
        .ok()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

fn store_state(file: &Utf8Path, state: &State) -> io::Result<()> {
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string(state).map_err(io::Error::other)?;

    // Written aside and renamed into place, so another `dash` starting at the
    // same moment never reads half a file.
    let partial = file.with_extension(format!("{}.tmp", process::id()));
    fs::write(&partial, json)?;
    fs::rename(&partial, file)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// A REST response as `gh api --include` prints it: the status line, the
/// headers, a blank line, and the body.
#[derive(Debug, PartialEq, Eq)]
struct Response<'a> {
    status: u16,
    etag: Option<&'a str>,
    body: &'a str,
}

/// Parse `gh api --include` output.
/// `None` when it does not start with a status line, which means `gh` failed
/// before getting a response.
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

/// The `message` of a REST error body, or the start of the body itself.
fn api_message(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| Some(value.get("message")?.as_str()?.to_owned()))
        .unwrap_or_else(|| body.chars().take(80).collect())
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
}

#[derive(Deserialize)]
struct Head {
    r#ref: String,
    sha: String,
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

/// One check on a commit: a check run from GitHub Actions or an app, or a
/// commit status posted by an outside service.
/// States are upper case.
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

/// Combine a commit's `check-runs` and `status` responses.
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

/// A query for the review threads of each pull request in `numbers`, one alias
/// per pull request.
///
/// A thread with a second comment has been answered, so two comments per thread
/// are enough to tell.
fn reviews_query(numbers: &[u64]) -> String {
    let mut query = String::from(
        "query($owner: String!, $name: String!) {\n  repository(owner: $owner, name: $name) {\n",
    );
    for number in numbers {
        query.push_str(&format!(
            "    pr{number}: pullRequest(number: {number}) {{ reviewThreads(first: {THREADS}) {{ \
             nodes {{ comments(first: 2) {{ nodes {{ id }} }} }} }} }}\n"
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
    /// Keyed by the `pr<number>` alias.
    /// `null` for a pull request that no longer exists.
    repository: HashMap<String, Option<ReviewedPullRequest>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewedPullRequest {
    review_threads: Nodes<Thread>,
}

#[derive(Deserialize)]
struct Nodes<T> {
    nodes: Vec<T>,
}

#[derive(Deserialize)]
struct Thread {
    comments: Nodes<serde_json::Value>,
}

/// Parse the response to [`reviews_query`] into each pull request's threads and
/// answered threads.
fn parse_reviews(json: &str) -> Result<Counts, Error> {
    let response: ReviewsResponse = parse_json(json)?;

    let reviews = response
        .data
        .repository
        .into_iter()
        .filter_map(|(alias, pr)| {
            let number = alias.strip_prefix("pr")?.parse().ok()?;
            let threads = pr?.review_threads.nodes;
            let answered = threads
                .iter()
                .filter(|thread| thread.comments.nodes.len() > 1)
                .count();
            Some((number, (count(threads.len()), count(answered))))
        })
        .collect();

    Ok(reviews)
}

/// Tally a commit's checks.
///
/// A re-run check is reported next to its earlier attempts; only the latest
/// attempt counts.
/// One that has not started yet has no `started_at` and is the latest by
/// definition.
fn checks(contexts: Vec<Context>) -> Checks {
    #[derive(Clone, Copy)]
    enum Outcome {
        Passed,
        Pending,
        Failed,
    }

    let mut latest: HashMap<String, (Option<String>, Outcome)> = HashMap::new();

    for context in contexts {
        let (name, started_at, outcome) = match context {
            Context::CheckRun {
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
            Context::StatusContext { context, state } => {
                let outcome = match state.as_str() {
                    "SUCCESS" => Outcome::Passed,
                    "PENDING" | "EXPECTED" => Outcome::Pending,
                    _ => Outcome::Failed,
                };
                (context, None, outcome)
            }
        };

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
