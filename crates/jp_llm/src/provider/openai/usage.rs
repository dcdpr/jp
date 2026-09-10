//! Reading a `ChatGPT` subscription's usage and spending its reset credits.
//!
//! A plan carries a small number of reset credits; redeeming one reopens a
//! spent usage window immediately.
//! Codex surfaces them as "usage limit resets available".
//!
//! These endpoints are siblings of the responses host, not children, so they
//! take their own base URL.

use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, warn};

/// Where the account endpoints live, relative to the `ChatGPT` backend root.
const ACCOUNT_PATH: &str = "wham";

/// A reset credit that has not been spent.
#[derive(Debug, Clone, Deserialize)]
pub struct ResetCredit {
    /// The credit's own id, named when redeeming a specific one.
    pub id: Option<String>,
}

/// What the account reports about its reset credits.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ResetCredits {
    /// The credits available to redeem.
    #[serde(default)]
    pub credits: Vec<ResetCredit>,
}

/// What redeeming a credit reported back.
#[derive(Debug, Clone, Deserialize)]
struct ConsumeResponse {
    /// Whether the window actually reopened.
    #[serde(default)]
    success: Option<bool>,
}

/// The reset credits the account has left.
///
/// Errors are reported as `None` rather than propagated: a credit that cannot
/// be counted is one that will not be spent, which leaves the caller exactly
/// where it already was.
pub async fn reset_credits(http: &Client, base_url: &str) -> Option<ResetCredits> {
    let url = format!("{}/{ACCOUNT_PATH}/rate-limit-reset-credits", root(base_url));

    match http.get(&url).send().await {
        Ok(response) if response.status().is_success() => match response.json().await {
            Ok(credits) => Some(credits),
            Err(error) => {
                debug!(%error, "Could not read the account's reset credits.");
                None
            }
        },
        Ok(response) => {
            debug!(status = %response.status(), "Reset credits are unavailable.");
            None
        }
        Err(error) => {
            debug!(%error, "Could not reach the reset-credit endpoint.");
            None
        }
    }
}

/// Spend one reset credit, reopening the spent usage window.
///
/// `redeem_request_id` makes the redemption idempotent: retrying with the same
/// id spends the same credit once, so a retried request cannot burn two.
///
/// Returns whether a window actually reopened.
/// A refusal is reported as `false` rather than an error for the same reason as
/// [`reset_credits`]: the caller's fallback is the chain it was already about
/// to walk.
pub async fn consume_reset_credit(
    http: &Client,
    base_url: &str,
    redeem_request_id: &str,
    credit_id: Option<&str>,
) -> bool {
    let url = format!(
        "{}/{ACCOUNT_PATH}/rate-limit-reset-credits/consume",
        root(base_url)
    );

    let mut body = serde_json::json!({ "redeem_request_id": redeem_request_id });
    if let Some(credit_id) = credit_id {
        body["credit_id"] = credit_id.into();
    }

    let response = match http.post(&url).json(&body).send().await {
        Ok(response) => response,
        Err(error) => {
            warn!(%error, "Could not reach the reset-credit endpoint.");
            return false;
        }
    };

    if !response.status().is_success() {
        debug!(status = %response.status(), "Redeeming a reset credit was refused.");
        return false;
    }

    match response.json::<ConsumeResponse>().await {
        // A body that parses but reports nothing is taken at its word: the
        // request succeeded, so the window is open.
        Ok(consumed) => consumed.success.unwrap_or(true),
        Err(error) => {
            debug!(%error, "Could not read the redemption result.");
            false
        }
    }
}

/// The backend root the account endpoints hang off.
///
/// Derived from the responses base URL by dropping its trailing path segment,
/// so pointing the provider at a recording server moves both together.
fn root(base_url: &str) -> &str {
    // Trimmed before the segment is dropped, so a configured URL written with a
    // trailing slash reaches the same root as one without.
    let base_url = base_url.trim_end_matches('/');
    base_url.strip_suffix("/codex").unwrap_or(base_url)
}

#[cfg(test)]
#[path = "usage_tests.rs"]
mod tests;
