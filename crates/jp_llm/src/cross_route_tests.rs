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
use serde_json::Value;

use crate::{
    provider::provider_test_support,
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

            if api != subscription {
                differences.push(format!(
                    "{id} `{scenario}`:\n  api:          {api:?}\n  subscription: {subscription:?}"
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

/// The shape of the conversation one route recorded: which events happened, in
/// what order, and which tools were called with which arguments.
///
/// Response prose and reasoning text are dropped, since two runs word them
/// differently.
fn recorded_conversation(dir: &str, scenario: &str) -> Option<Vec<String>> {
    let path = jp_test::fixtures_dir()
        .join(dir)
        .join(format!("{scenario}__conversation_stream.snap"));
    let raw = std::fs::read_to_string(path).ok()?;

    let shape = raw
        .lines()
        // An insta snapshot opens with a `---` delimited header naming the
        // source; only the body describes the conversation.
        .skip_while(|line| *line != "---")
        .skip(1)
        .filter_map(structural_line)
        .collect();

    Some(shape)
}

/// One structural fact from a snapshot line, or `None` for prose.
///
/// Event kinds and tool names are JP's own vocabulary, so both routes owe the
/// same sequence of them.
fn structural_line(line: &str) -> Option<String> {
    let trimmed = line.trim();

    for marker in [
        "TurnStart",
        "ChatRequest",
        "ToolCallRequest",
        "ToolCallResponse",
        "InquiryRequest",
        "InquiryResponse",
    ] {
        if trimmed.starts_with(marker) {
            return Some(marker.to_owned());
        }
    }

    // JP derives these from the schema it sent.
    for field in ["name:", "id:", "type:"] {
        if let Some(value) = trimmed.strip_prefix(field) {
            return Some(format!("{field}{}", value.trim()));
        }
    }

    // The variant is a decode decision; its content is the model's.
    if trimmed.starts_with("Reasoning") || trimmed.starts_with("Message") {
        return Some(trimmed.split_whitespace().next()?.to_owned());
    }

    None
}
