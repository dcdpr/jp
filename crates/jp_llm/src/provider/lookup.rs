//! Asking a provider about a model without letting the answer hold up a
//! request.
//!
//! What a provider reports about a model (context window, output limit,
//! reasoning support) fills in details a request can go ahead without.
//! A lookup that fails, or takes longer than [`MODEL_LOOKUP_TIMEOUT`], is
//! logged and treated as reporting nothing, and the provider falls back to its
//! built-in table, or to empty details for a model the table does not know.

use std::{fmt::Display, time::Duration};

use jp_config::model::id::ModelIdConfig;
use tracing::warn;

use crate::{error::Result, model::ModelDetails};

/// How long a model lookup may take before the request goes ahead without it.
pub(crate) const MODEL_LOOKUP_TIMEOUT: Duration = Duration::from_secs(3);

/// Why a lookup still running at [`MODEL_LOOKUP_TIMEOUT`] reported nothing.
pub(crate) const TIMED_OUT: &str = "no answer within the lookup timeout";

/// Run `lookup`, yielding `None` when it fails or is still running at
/// [`MODEL_LOOKUP_TIMEOUT`].
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

/// Find `model` in a provider's `listing`.
///
/// `None` when the listing fails, is still running at [`MODEL_LOOKUP_TIMEOUT`],
/// or does not list the model.
pub(crate) async fn find_listed(
    model: &ModelIdConfig,
    listing: impl Future<Output = Result<Vec<ModelDetails>>>,
) -> Option<ModelDetails> {
    advisory_lookup(model, listing)
        .await?
        .into_iter()
        .find(|listed| &listed.id == model)
}

/// Log a lookup that reported nothing about `model`, and why.
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
