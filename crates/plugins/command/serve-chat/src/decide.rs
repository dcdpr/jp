//! Asking a model whether a message is meant for the assistant, before a turn
//! is spent on it.
//!
//! [`Jev`] calls the decide API (`decide.url`, Jev's `systemone` endpoint by
//! default): one yes-or-no question about a piece of text, answered with the
//! probability of "yes" (a `noul` answer).
//! The bridge compares that probability with `decide.threshold`.

use std::{future::Future, pin::Pin, time::Duration};

use serde_json::{Map, Value, json};

use crate::options::DecideOptions;

/// The name the question is asked and answered under.
const QUESTION: &str = "respond";

/// How long a decision may take; a message waits for it, and every message
/// after it in the same bridge does too.
const TIMEOUT: Duration = Duration::from_secs(10);

/// How much of an error response is quoted in the log.
const MAX_ERROR_BODY: usize = 300;

/// A decision in flight: the probability that the assistant should answer.
pub(crate) type Decision = Pin<Box<dyn Future<Output = Result<f64, String>> + Send>>;

/// Decides whether the assistant should answer the last message in a chat.
pub(crate) trait Decide: Send + Sync {
    /// The probability that the assistant should answer the last message in
    /// `state`, a chat's recent messages.
    fn decide(&self, state: String) -> Decision;
}

/// The decide API, over HTTPS with a bearer token.
pub(crate) struct Jev {
    client: reqwest::Client,
    options: DecideOptions,
    token: String,
}

impl Jev {
    pub(crate) fn new(options: DecideOptions, token: String) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .build()
            .map_err(|error| format!("failed to set up the decide API client: {error}"))?;

        Ok(Self {
            client,
            options,
            token,
        })
    }
}

impl Decide for Jev {
    fn decide(&self, state: String) -> Decision {
        let request = self
            .client
            .post(&self.options.url)
            .bearer_auth(&self.token)
            .json(&request_body(&self.options, &state));

        Box::pin(async move {
            let response = request
                .send()
                .await
                .map_err(|error| format!("the decide API didn't answer: {error}"))?;
            let status = response.status();
            let body = response
                .text()
                .await
                .map_err(|error| format!("the decide API's answer couldn't be read: {error}"))?;
            if !status.is_success() {
                let quoted: String = body.chars().take(MAX_ERROR_BODY).collect();
                return Err(format!("the decide API answered {status}: {quoted}"));
            }

            let body: Value = serde_json::from_str(&body)
                .map_err(|error| format!("the decide API's answer isn't JSON: {error}"))?;
            probability(&body)
        })
    }
}

/// The request asking `options.instructions` about `state`.
pub(crate) fn request_body(options: &DecideOptions, state: &str) -> Value {
    let mut questions = Map::new();
    questions.insert(
        QUESTION.to_owned(),
        json!({ "type": "noul", "instructions": options.instructions }),
    );

    json!({
        "model": options.model,
        "state": state,
        "questions": questions,
    })
}

/// The probability of "yes" in a decide API answer.
pub(crate) fn probability(body: &Value) -> Result<f64, String> {
    body.pointer(&format!("/answers/{QUESTION}/noul"))
        .and_then(Value::as_f64)
        .filter(|probability| (0.0..=1.0).contains(probability))
        .ok_or_else(|| {
            format!("the decide API's answer has no probability at `answers.{QUESTION}.noul`")
        })
}

#[cfg(test)]
#[path = "decide_tests.rs"]
mod tests;
