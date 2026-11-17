//! Review-thread counts per pull request, read through GraphQL.
//!
//! The REST API omits pending review comments created by `just pr-review` and
//! `just pr-triage`, so review threads are read through GraphQL.
//! GraphQL has no conditional requests, so a pull request's threads are read
//! again only after [`LOCAL_REVIEWS_INTERVAL`] (with a local worktree) or
//! [`REMOTE_REVIEWS_INTERVAL`] (without one), batching every pull request due
//! into one query.
//! An explicit refresh (`r`) reads them all.

use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use serde::{Deserialize, Serialize};

use super::{Error, count, parse_json};
use crate::model::PullRequest;

/// Review-thread refresh interval for pull requests with a local worktree.
const LOCAL_REVIEWS_INTERVAL: Duration = Duration::from_mins(3);

/// Review-thread refresh interval for pull requests without a local worktree.
const REMOTE_REVIEWS_INTERVAL: Duration = Duration::from_mins(15);

/// Maximum review threads fetched per pull request in [`reviews_query`] to
/// bound GraphQL query cost.
/// Threads beyond this limit are counted in `totalCount` but not inspected for
/// replies.
const THREADS: u32 = 30;

/// Map from PR number to `(total_threads, answered_threads)`.
pub(super) type Counts = HashMap<u64, (u32, u32)>;

/// Map from PR number to cached review-thread counts.
pub(super) type Reviews = HashMap<u64, Review>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Review {
    pub(super) threads: u32,
    pub(super) answered: u32,

    /// Unix timestamp (in seconds) when the review threads were last fetched.
    pub(super) read_at: u64,
}

/// Return PR numbers whose review threads need refreshing: all PRs when `asked`
/// is true, or those with missing or expired review cache entries.
pub(super) fn due_reviews(
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

/// Build a GraphQL query fetching the review threads for each PR in `numbers`.
///
/// Requests up to 2 comments per thread, which is sufficient to determine
/// whether the thread has at least one reply.
pub(super) fn reviews_query(numbers: &[u64]) -> String {
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
pub(super) fn parse_reviews(json: &str) -> Result<Counts, Error> {
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

#[cfg(test)]
#[path = "reviews_tests.rs"]
mod tests;
