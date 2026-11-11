//! Asking a model what to do with a message, before a turn is spent on it.
//!
//! [`SystemOne`] calls a decide API in the System One request format
//! (`decide.url`, Jev by default; local servers speak it too): one `choice`
//! question about the chat, answered with the [`Action`] the model picks and
//! the probability it gives that action.
//! The bridge takes the action only when that probability reaches
//! `decide.threshold`.

use std::{future::Future, pin::Pin, time::Duration};

use serde_json::{Map, Value, json};

use crate::options::DecideOptions;

/// The name the question is asked and answered under.
const QUESTION: &str = "action";

/// How long a decision may take; a message waits for it, and every message
/// after it in the same bridge does too.
const TIMEOUT: Duration = Duration::from_secs(10);

/// How much of an error response is quoted in the log.
const MAX_ERROR_BODY: usize = 300;

/// What the bridge can do with a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    /// Nothing: the message is for the other people in the chat.
    Ignore,

    /// Run a turn for it, or hand it to the running one.
    Answer,

    /// Start the chat's next turn in a new conversation.
    NewConversation,

    /// Compact the chat's conversation before its next turn.
    Compact,
}

impl Action {
    const ALL: [Self; 4] = [
        Self::Ignore,
        Self::Answer,
        Self::NewConversation,
        Self::Compact,
    ];

    /// The option's name in the question and the answer.
    fn key(self) -> &'static str {
        match self {
            Self::Ignore => "ignore",
            Self::Answer => "answer",
            Self::NewConversation => "new_conversation",
            Self::Compact => "compact",
        }
    }

    /// What the option means, as the model reads it.
    fn description(self) -> &'static str {
        match self {
            Self::Ignore => {
                "The last message is for the other people in the chat, such as thanks, small talk, \
                 news, or arrangements between them, and asks nothing of the assistant."
            }
            Self::Answer => {
                "The last message asks the assistant a question, asks it to do something, or \
                 corrects or follows up on its previous answer."
            }
            Self::NewConversation => {
                "The last message asks the assistant to start over in a new, empty conversation, \
                 and asks nothing else."
            }
            Self::Compact => {
                "The last message asks the assistant to compact, shorten, or clean up its current \
                 conversation while keeping it, and asks nothing else."
            }
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|action| action.key() == key)
    }
}

/// The model's answer: the action it picked, and the probability it gave it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Verdict {
    pub action: Action,
    pub probability: f64,
}

/// A decision in flight.
pub(crate) type Decision = Pin<Box<dyn Future<Output = Result<Verdict, String>> + Send>>;

/// Decides what the bridge does with the last message in a chat.
pub(crate) trait Decide: Send + Sync {
    /// The action for the last message in `state`, a chat's recent messages.
    fn decide(&self, state: String) -> Decision;
}

/// A decide API over HTTP, with a bearer token when it asks for one.
pub(crate) struct SystemOne {
    client: reqwest::Client,
    options: DecideOptions,
    token: Option<String>,
}

impl SystemOne {
    pub(crate) fn new(options: DecideOptions, token: Option<String>) -> Result<Self, String> {
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

impl Decide for SystemOne {
    fn decide(&self, state: String) -> Decision {
        let mut request = self
            .client
            .post(&self.options.url)
            .json(&request_body(&self.options, &state));
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }

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
            verdict(&body)
        })
    }
}

/// The request asking `options.instructions` about `state`, with every
/// [`Action`] as an option.
pub(crate) fn request_body(options: &DecideOptions, state: &str) -> Value {
    let criteria: Map<String, Value> = Action::ALL
        .into_iter()
        .map(|action| (action.key().to_owned(), json!(action.description())))
        .collect();
    let mut questions = Map::new();
    questions.insert(
        QUESTION.to_owned(),
        json!({ "type": "choice", "instructions": options.instructions, "criteria": criteria }),
    );

    json!({
        "model": options.model,
        "state": state,
        "questions": questions,
    })
}

/// The action a decide API answer picks, and the probability it gives it.
pub(crate) fn verdict(body: &Value) -> Result<Verdict, String> {
    let answer = body
        .pointer(&format!("/answers/{QUESTION}"))
        .ok_or_else(|| format!("the decide API's answer has no `answers.{QUESTION}`"))?;
    let choice = answer
        .get("choice")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("the decide API's answer has no `answers.{QUESTION}.choice`"))?;
    let action = Action::from_key(choice)
        .ok_or_else(|| format!("the decide API chose `{choice}`, which isn't an option"))?;
    let probability = answer
        .get("probabilities")
        .and_then(|probabilities| probabilities.get(choice))
        .and_then(Value::as_f64)
        .filter(|probability| (0.0..=1.0).contains(probability))
        .ok_or_else(|| format!("the decide API's answer has no probability for `{choice}`"))?;

    Ok(Verdict {
        action,
        probability,
    })
}

#[cfg(test)]
#[path = "decide_tests.rs"]
mod tests;
