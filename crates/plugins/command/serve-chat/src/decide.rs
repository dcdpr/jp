//! Pre-turn message classification via a System One decide API.
//!
//! [`SystemOne`] sends a single `choice` question about recent chat messages to
//! `decide.url` and returns the chosen [`Action`] and its probability.
//! The bridge applies the action when its probability meets `decide.threshold`,
//! and falls back to [`Action::Answer`] otherwise.
//!
//! Command actions (`new_conversation`, `compact`) are offered only when the
//! message is eligible to issue commands (direct chats, or group messages that
//! start with a mention of the bot).

use std::{future::Future, pin::Pin, time::Duration};

use serde_json::{Map, Value, json};

use crate::options::DecideOptions;

/// Question key in the System One request and response.
const QUESTION: &str = "action";

/// HTTP request timeout for a decide API call.
const TIMEOUT: Duration = Duration::from_secs(10);

/// Maximum characters of an error response body included in log messages.
const MAX_ERROR_BODY: usize = 300;

/// Action the bridge can take on an incoming message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    /// Ignore the message because it is addressed to other participants.
    Ignore,

    /// Run a turn for the message, or deliver it to the active turn as a reply.
    Answer,

    /// Start the chat's next turn in a new conversation.
    NewConversation,

    /// Compact the chat's conversation.
    Compact,
}

impl Action {
    /// Actions offered for ordinary group messages.
    const MESSAGE: &[Self] = &[Self::Ignore, Self::Answer];

    /// Actions offered when the message can issue a command.
    const COMMAND: &[Self] = &[
        Self::Ignore,
        Self::Answer,
        Self::NewConversation,
        Self::Compact,
    ];

    /// Return the candidate actions for a message.
    pub(crate) fn offered(commands: bool) -> &'static [Self] {
        if commands {
            Self::COMMAND
        } else {
            Self::MESSAGE
        }
    }

    /// Wire key for this action in the decide API request and response.
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Ignore => "ignore",
            Self::Answer => "answer",
            Self::NewConversation => "new_conversation",
            Self::Compact => "compact",
        }
    }

    /// Criterion description sent to the decide model.
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
}

/// Action selected by the decide model and its probability.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Verdict {
    pub action: Action,
    pub probability: f64,
}

pub(crate) type Decision = Pin<Box<dyn Future<Output = Result<Verdict, String>> + Send>>;

/// Classifier that selects an [`Action`] for the latest message in a chat.
pub(crate) trait Decide: Send + Sync {
    /// Choose one of `actions` for the chat transcript in `state`.
    fn decide(&self, state: String, actions: &'static [Action]) -> Decision;
}

/// HTTP client for a System One decide endpoint.
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
    fn decide(&self, state: String, actions: &'static [Action]) -> Decision {
        let mut request =
            self.client
                .post(&self.options.url)
                .json(&request_body(&self.options, &state, actions));
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
            verdict(&body, actions)
        })
    }
}

/// Build the System One JSON request body for `state` and `actions`.
pub(crate) fn request_body(options: &DecideOptions, state: &str, actions: &[Action]) -> Value {
    let criteria: Map<String, Value> = actions
        .iter()
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

/// Parse the chosen [`Action`] and its probability from a System One response.
pub(crate) fn verdict(body: &Value, actions: &[Action]) -> Result<Verdict, String> {
    let answer = body
        .pointer(&format!("/answers/{QUESTION}"))
        .ok_or_else(|| format!("the decide API's answer has no `answers.{QUESTION}`"))?;
    let choice = answer
        .get("choice")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("the decide API's answer has no `answers.{QUESTION}.choice`"))?;
    let action = actions
        .iter()
        .copied()
        .find(|action| action.key() == choice)
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
