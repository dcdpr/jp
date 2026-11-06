//! Bounded, non-fatal model metadata lookups.
//!
//! [`advisory_lookup`] and [`find_listed`] bound provider model queries to
//! [`MODEL_LOOKUP_TIMEOUT`] and log failures at `warn` instead of propagating
//! them, so callers can fall back to built-in catalog entries or empty
//! [`ModelDetails`].

use std::{fmt::Display, time::Duration};

use jp_config::model::id::ModelIdConfig;
use tracing::warn;

use crate::{error::Result, model::ModelDetails};

/// Maximum duration to wait for a provider model lookup.
pub(crate) const MODEL_LOOKUP_TIMEOUT: Duration = Duration::from_secs(3);

/// Warning reason logged when a model lookup exceeds [`MODEL_LOOKUP_TIMEOUT`].
pub(crate) const TIMED_OUT: &str = "no answer within the lookup timeout";

/// Await `lookup` up to [`MODEL_LOOKUP_TIMEOUT`], logging and returning `None`
/// on error or timeout.
pub(crate) async fn advisory_lookup<T, E: Display>(
    model: &ModelIdConfig,
    lookup: impl Future<Output = std::result::Result<T, E>>,
) -> Option<T> {
    match tokio::time::timeout(MODEL_LOOKUP_TIMEOUT, lookup).await {
        Ok(Ok(found)) => Some(found),
        Ok(Err(error)) => {
            skipped(model, &error);
            None
        }
        Err(_) => {
            skipped(model, &TIMED_OUT);
            None
        }
    }
}

/// Find `model` in a provider's model `listing`.
///
/// Returns `None` if `listing` fails, exceeds [`MODEL_LOOKUP_TIMEOUT`], or does
/// not contain `model`.
pub(crate) async fn find_listed(
    model: &ModelIdConfig,
    listing: impl Future<Output = Result<Vec<ModelDetails>>>,
) -> Option<ModelDetails> {
    advisory_lookup(model, listing)
        .await?
        .into_iter()
        .find(|listed| &listed.id == model)
}

/// Log a warning when a model lookup for `model` fails or times out.
pub(crate) fn skipped(model: &ModelIdConfig, reason: &dyn Display) {
    warn!(
        %model,
        %reason,
        "Model lookup reported nothing; continuing with built-in details and defaults."
    );
}

#[cfg(test)]
#[path = "lookup_tests.rs"]
mod tests;
