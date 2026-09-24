//! Comparing what a provider's two billing routes recorded.
//!
//! The recorded suites prove each route can run a scenario; these compare the
//! two runs of one scenario against each other, so switching billing does not
//! change the conversation.
//!
//! Two comparisons, failing for different reasons: [`request_parity`] reads the
//! cassettes and compares what JP sent, [`outcome_parity`] reads the
//! conversation snapshots and compares what JP recorded.
//! Neither compares the model's wording, which differs between runs.

use std::{collections::BTreeSet, path::PathBuf};

use jp_config::{ConfigEnum as _, model::id::ProviderId};
use saphyr::{LoadableYamlNode as _, Yaml};
use serde_json::{Map, Value};

use crate::{
    provider::{number_ids, provider_test_support},
    test::{ProviderTestMode, fixture_dir},
};

/// Every provider that records a second route, and where each route's fixtures
/// live.
///
/// Asked of the providers, so implementing or dropping a subscription route
/// changes what is compared without touching this file.
fn routed_providers() -> Vec<(ProviderId, String, String)> {
    ProviderId::variants()
        .into_iter()
        .filter(|id| provider_test_support(*id).subscription().is_some())
        .map(|id| {
            (
                id,
                fixture_dir(id, ProviderTestMode::Api),
                fixture_dir(id, ProviderTestMode::Subscription),
            )
        })
        .collect()
}

/// The request bodies one route recorded for one scenario.
///
/// A cassette holds one entry per request the scenario made, in order, so a
/// turn that called a tool contributes the follow-up request too.
fn recorded_requests(dir: &str, scenario: &str) -> Option<Vec<Value>> {
    let path: PathBuf = jp_test::fixtures_dir()
        .join(dir)
        .join(format!("{scenario}.yml"));

    let raw = std::fs::read_to_string(path).ok()?;

    let bodies = Yaml::load_from_str(&raw)
        .ok()?
        .iter()
        .filter_map(|entry| {
            entry
                .as_mapping_get("when")?
                .as_mapping_get("json_body_str")?
                .as_str()
                .and_then(|body| serde_json::from_str(body).ok())
        })
        .collect();

    Some(bodies)
}

/// The scenarios both routes recorded.
///
/// A scenario only one route has is skipped: recording is per-suite, so a
/// half-recorded pair means the fixtures are mid-update.
fn shared_scenarios(api_dir: &str, subscription_dir: &str) -> Vec<String> {
    let names = |dir: &str| -> BTreeSet<String> {
        let Ok(entries) = std::fs::read_dir(jp_test::fixtures_dir().join(dir)) else {
            return BTreeSet::new();
        };

        entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                name.strip_suffix(".yml").map(str::to_owned)
            })
            .collect()
    };

    names(api_dir)
        .intersection(&names(subscription_dir))
        .cloned()
        .collect()
}

/// Both routes must have been sent the same request, once the wire dialect is
/// projected away.
#[test]
fn request_parity() {
    let mut differences = vec![];

    for (id, api_dir, subscription_dir) in routed_providers() {
        let support = provider_test_support(id);

        for scenario in &shared_scenarios(&api_dir, &subscription_dir) {
            let (Some(api), Some(subscription)) = (
                recorded_requests(&api_dir, scenario),
                recorded_requests(&subscription_dir, scenario),
            ) else {
                continue;
            };

            if api.len() != subscription.len() {
                differences.push(format!(
                    "{id} `{scenario}`: {} requests through the API route, {} through the \
                     subscription route",
                    api.len(),
                    subscription.len()
                ));
                continue;
            }

            for (index, (api, subscription)) in api.iter().zip(&subscription).enumerate() {
                let api = support.project_request(api);
                let subscription = support.project_request(subscription);

                if api != subscription {
                    differences.push(format!(
                        "{id} `{scenario}` request {index}:\n  api:          {api}\n  \
                         subscription: {subscription}"
                    ));
                }
            }
        }
    }

    assert!(
        differences.is_empty(),
        "{} request(s) differ between billing routes:\n\n{}",
        differences.len(),
        differences.join("\n\n")
    );
}

/// Both routes must have produced the same conversation, once the model's
/// wording is projected away.
#[test]
fn outcome_parity() {
    let mut differences = vec![];

    for (id, api_dir, subscription_dir) in routed_providers() {
        for scenario in &shared_scenarios(&api_dir, &subscription_dir) {
            let (Some(api), Some(subscription)) = (
                recorded_conversation(&api_dir, scenario),
                recorded_conversation(&subscription_dir, scenario),
            ) else {
                continue;
            };

            // A snapshot this projection cannot read reduces to nothing on
            // both sides, and two nothings compare equal.
            if api.as_array().is_none_or(Vec::is_empty) {
                differences.push(format!(
                    "{id} `{scenario}`: the recorded conversation projected to no events"
                ));
                continue;
            }

            if api != subscription {
                differences.push(format!(
                    "{id} `{scenario}`:\n  api:          {api}\n  subscription: {subscription}"
                ));
            }
        }
    }

    assert!(
        differences.is_empty(),
        "{} conversation(s) differ between billing routes:\n\n{}",
        differences.len(),
        differences.join("\n\n")
    );
}

#[path = "cross_route/projection_tests.rs"]
mod projection;

#[path = "cross_route/outcome_tests.rs"]
mod outcome;

/// The shape of the conversation one route recorded, read from its snapshot.
///
/// See [`project_conversation`] for what is kept.
fn recorded_conversation(dir: &str, scenario: &str) -> Option<Value> {
    let path = jp_test::fixtures_dir()
        .join(dir)
        .join(format!("{scenario}__conversation_stream.snap"));
    let raw = std::fs::read_to_string(&path).ok()?;

    // A snapshot that exists but cannot be read is a broken comparison, not a
    // scenario to skip.
    let body = snapshot_body(&raw).unwrap_or_else(|| panic!("{}: no insta header", path.display()));
    let conversation =
        serde_json::from_str(body).unwrap_or_else(|error| panic!("{}: {error}", path.display()));

    Some(project_conversation(&conversation))
}

/// The body of an insta snapshot, after its `---` delimited header.
///
/// Lines may end in `\r\n`: a Windows checkout converts the fixtures unless
/// told otherwise.
fn snapshot_body(raw: &str) -> Option<&str> {
    let mut offset = 0;
    let mut delimiters = 0;

    for line in raw.split_inclusive('\n') {
        offset += line.len();

        if line.trim_end_matches(['\r', '\n']) == "---" {
            delimiters += 1;
            if delimiters == 2 {
                return Some(&raw[offset..]);
            }
        } else if delimiters == 0 {
            // The header has to open the file.
            return None;
        }
    }

    None
}

/// Reduce a recorded conversation to what two routes must agree on.
///
/// Kept: the order and kind of events, what the user asked, which tools were
/// called, what they returned, and which call each result answers.
///
/// Dropped, since two runs never share them:
///
/// - Timestamps and provider metadata.
/// - The model's wording, reasoning, structured answers, and chosen tool
///   arguments; only the kind of answer is kept.
/// - Config deltas, which the harness writes when it points a route at its own
///   model.
/// - Tool call ids, which the host mints; they are numbered instead, so a
///   result answering the wrong call still shows.
///
/// A run of answers of one kind is collapsed into one, since the model decides
/// how many items to split its answer into.
fn project_conversation(conversation: &Value) -> Value {
    let mut events: Vec<Value> = vec![];

    for event in conversation
        .get("events")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(projected) = project_event(event) else {
            continue;
        };

        if projected.get("type") == Some(&Value::from("chat_response"))
            && events.last() == Some(&projected)
        {
            continue;
        }

        events.push(projected);
    }

    let mut events = Value::Array(events);
    number_ids(&mut events, &["id"]);

    events
}

/// One event as [`project_conversation`] keeps it, or `None` when dropped.
fn project_event(event: &Value) -> Option<Value> {
    let kind = event.get("type")?.as_str()?;
    let mut kept = Map::new();
    kept.insert("type".to_owned(), Value::from(kind));

    let copy = |kept: &mut Map<String, Value>, keys: &[&str]| {
        for key in keys {
            if let Some(value) = event.get(*key) {
                kept.insert((*key).to_owned(), value.clone());
            }
        }
    };

    match kind {
        "config_delta" => return None,

        // Reasoning is optional output: whether the model summarizes its
        // thinking at all varies between runs.
        "chat_response" if event.get("reasoning").is_some() => return None,

        "chat_response" => {
            let variant = ["message", "data"]
                .into_iter()
                .find(|key| event.get(*key).is_some())
                .unwrap_or("unknown");
            kept.insert("variant".to_owned(), Value::from(variant));
        }

        "chat_request" => copy(&mut kept, &["content", "schema"]),
        "tool_call_request" => copy(&mut kept, &["id", "name"]),
        "tool_call_response" => copy(&mut kept, &["id", "content", "is_error"]),
        _ => copy(&mut kept, &["id"]),
    }

    Some(Value::Object(kept))
}
