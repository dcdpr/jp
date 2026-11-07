use std::{future, time::Duration};

use jp_config::model::id::{ModelIdConfig, ProviderId};

use super::*;
use crate::error::Error;

fn id(name: &str) -> ModelIdConfig {
    (ProviderId::Test, name).try_into().unwrap()
}

fn with_window(name: &str, window: u32) -> ModelDetails {
    ModelDetails {
        context_window: Some(window),
        ..ModelDetails::empty(id(name))
    }
}

#[tokio::test]
async fn an_answered_lookup_is_used() {
    let found = advisory_lookup(&id("m"), async { Ok::<_, Error>(7) }).await;

    assert_eq!(found, Some(7));
}

#[tokio::test]
async fn a_failed_lookup_reports_nothing() {
    let failed = async { Err::<u32, _>(Error::InvalidResponse("API route not found".into())) };

    assert_eq!(advisory_lookup(&id("m"), failed).await, None);
}

#[tokio::test(start_paused = true)]
async fn a_lookup_still_running_at_the_timeout_reports_nothing() {
    let stalled = future::pending::<Result<u32>>();

    assert_eq!(advisory_lookup(&id("m"), stalled).await, None);
}

#[tokio::test(start_paused = true)]
async fn a_lookup_answering_before_the_timeout_is_used() {
    let slow = async {
        let just_inside = MODEL_LOOKUP_TIMEOUT.checked_sub(Duration::from_millis(1));
        tokio::time::sleep(just_inside.unwrap()).await;
        Ok::<_, Error>(7)
    };

    assert_eq!(advisory_lookup(&id("m"), slow).await, Some(7));
}

#[tokio::test]
async fn find_listed_returns_the_listed_model() {
    let listing = async { Ok(vec![with_window("a", 8), with_window("b", 16)]) };

    let found = find_listed(&id("b"), listing).await;

    assert_eq!(found, Some(with_window("b", 16)));
}

#[tokio::test]
async fn find_listed_reports_nothing_for_a_model_the_listing_omits() {
    let listing = async { Ok(vec![with_window("a", 8)]) };

    assert_eq!(find_listed(&id("b"), listing).await, None);
}

#[tokio::test]
async fn find_listed_reports_nothing_when_the_listing_fails() {
    let listing = async { Err(Error::InvalidResponse("HTTP 404".into())) };

    assert_eq!(find_listed(&id("b"), listing).await, None);
}
