//! A commit's check runs and commit statuses, and the checks the default
//! branch's rulesets require, reduced to a pull request's [`Checks`].

use std::collections::{HashMap, HashSet};

use serde::Deserialize;

use super::{Error, count, parse_json};
use crate::model::Checks;

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
pub(super) enum Context {
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
pub(super) fn contexts(runs: &str, status: &str) -> Result<Vec<Context>, Error> {
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
pub(super) fn parse_required(json: &str) -> Result<Vec<String>, Error> {
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
pub(super) fn checks(contexts: Vec<Context>, required: &[String]) -> Checks {
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

#[cfg(test)]
#[path = "checks_tests.rs"]
mod tests;
