//! Reading a `ChatGPT` subscription's reset credits and spending one.
//!
//! A plan carries a small number of reset credits; redeeming one reopens a
//! spent usage window immediately.
//! Codex surfaces them as "usage limit resets available".
//!
//! These endpoints are siblings of the responses host, not children, so they
//! take their own base URL.

use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::{debug, warn};

/// Where the account endpoints live, relative to the `ChatGPT` backend root.
const ACCOUNT_PATH: &str = "wham";

/// What the account reports about its reset credits.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ResetCredits {
    /// How many credits can still be redeemed.
    ///
    /// The backend also lists credits already redeemed or mid-redemption, so
    /// the length of that list says nothing about what is left.
    #[serde(default)]
    pub available_count: u32,
}

/// What redeeming a credit reported back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ConsumeCode {
    /// A credit was spent and the spent windows reopened.
    Reset,

    /// No window was spent, so nothing was redeemed.
    NothingToReset,

    /// No credit was left to spend.
    NoCredit,

    /// This redemption id was already used, so the reset it asked for has
    /// happened.
    AlreadyRedeemed,

    /// A code this build does not know, read as no reset.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
struct ConsumeResponse {
    code: ConsumeCode,
}

/// What a redemption attempt is known to have done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Redemption {
    /// The backend confirmed a spent window reopened.
    Reopened,

    /// The backend confirmed nothing was redeemed.
    Refused,

    /// The request may have reached the backend, but no answer confirms either
    /// way: the connection dropped, the server failed, or the body could not be
    /// read.
    Unknown,
}

/// The reset credits the account has left.
///
/// Errors are reported as `None` rather than propagated: a credit that cannot
/// be counted is one that will not be spent, which leaves the caller exactly
/// where it already was.
pub async fn reset_credits(http: &Client, base_url: &str) -> Option<ResetCredits> {
    let url = format!("{}/{ACCOUNT_PATH}/rate-limit-reset-credits", root(base_url));

    let response = match http.get(&url).send().await {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            debug!(status = %response.status(), "Reset credits are unavailable.");
            return None;
        }
        Err(error) => {
            debug!(%error, "Could not reach the reset-credit endpoint.");
            return None;
        }
    };

    match response.text().await {
        Ok(body) => parse_credits(&body),
        Err(error) => {
            debug!(%error, "Could not read the account's reset credits.");
            None
        }
    }
}

/// Spend one reset credit, reopening the spent usage window.
///
/// `redeem_request_id` makes the redemption idempotent: retrying with the same
/// id spends the same credit once.
/// The backend picks which credit to spend.
///
/// An outcome no answer confirms is reported as [`Redemption::Unknown`] rather
/// than as a refusal: the credit may be spent and the window open.
pub async fn consume_reset_credit(
    http: &Client,
    base_url: &str,
    redeem_request_id: &str,
) -> Redemption {
    let url = format!(
        "{}/{ACCOUNT_PATH}/rate-limit-reset-credits/consume",
        root(base_url)
    );

    let response = match http
        .post(&url)
        .json(&consume_body(redeem_request_id))
        .send()
        .await
    {
        Ok(response) => response,

        // A connection that never opened carried no request.
        Err(error) if error.is_connect() => {
            warn!(%error, "Could not reach the reset-credit endpoint.");
            return Redemption::Refused;
        }
        Err(error) => {
            warn!(%error, "Redemption request failed after it was sent.");
            return Redemption::Unknown;
        }
    };

    let status = response.status();
    if status.is_server_error() {
        warn!(%status, "Reset-credit endpoint failed; the redemption may have happened.");
        return Redemption::Unknown;
    }
    if !status.is_success() {
        debug!(%status, "Redeeming a reset credit was refused.");
        return Redemption::Refused;
    }

    match response.text().await {
        Ok(body) => redemption_outcome(&body),
        Err(error) => {
            warn!(%error, "Could not read the redemption result.");
            Redemption::Unknown
        }
    }
}

/// Read the credit listing, or `None` when it cannot be parsed.
fn parse_credits(body: &str) -> Option<ResetCredits> {
    serde_json::from_str(body)
        .inspect_err(|error| debug!(%error, "Could not parse the account's reset credits."))
        .ok()
}

/// The body of a redemption request.
///
/// No `credit_id`: the backend spends one that is available, which spares JP
/// from telling an available credit apart from a redeemed one.
fn consume_body(redeem_request_id: &str) -> Value {
    json!({ "redeem_request_id": redeem_request_id })
}

/// What a redemption's `200` body reports.
///
/// The endpoint answers `200` for a redemption that did nothing, with a `code`
/// saying why, so the status alone proves nothing.
/// A body with a code this build does not know, or no code at all, confirms
/// nothing either way.
fn redemption_outcome(body: &str) -> Redemption {
    match serde_json::from_str::<ConsumeResponse>(body) {
        Ok(response) => {
            debug!(code = ?response.code, "Reset credit redemption answered.");
            match response.code {
                ConsumeCode::Reset | ConsumeCode::AlreadyRedeemed => Redemption::Reopened,
                ConsumeCode::NothingToReset | ConsumeCode::NoCredit => Redemption::Refused,
                ConsumeCode::Unknown => Redemption::Unknown,
            }
        }
        Err(error) => {
            debug!(%error, "Could not parse the redemption result.");
            Redemption::Unknown
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
