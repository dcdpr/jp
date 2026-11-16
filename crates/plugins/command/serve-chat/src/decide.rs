//! Pre-turn message classification via a System One decide API.
//!
//! [`SystemOne`] sends a single `choice` question about recent chat messages to
//! `decide.url` and returns the chosen [`Action`] and its probability.
//! The bridge applies the action when its probability meets `decide.threshold`,
//! and falls back to [`Action::Answer`] otherwise.
//!
//! Every message is offered all actions, including the commands
//! (`new_conversation`, `compact`); the decide model determines whether a
//! message requests one.
//!
//! With `inject.packs` configured, the same request asks a second `choice`
//! question, [`PackQuestion`]: which context pack, if any, the assistant needs
//! for the message.

use std::{future::Future, pin::Pin, time::Duration};

use serde_json::{Map, Value, json};

use crate::options::{DecideOptions, InjectOptions, NO_PACK};

/// Question key in the System One request and response.
const QUESTION: &str = "action";

/// Key of the pack question in the System One request and response.
const PACK_QUESTION: &str = "pack";

/// Criterion description for [`NO_PACK`].
const NO_PACK_DESCRIPTION: &str = "The assistant needs none of the other options to answer the \
                                   last message, or the message asks nothing of it.";

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
    /// The actions offered for every message.
    pub(crate) const ALL: &[Self] = &[
        Self::Ignore,
        Self::Answer,
        Self::NewConversation,
        Self::Compact,
    ];

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
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Verdict {
    pub action: Action,
    pub probability: f64,

    /// Context pack selected for the message, if a pack question was asked and
    /// a pack other than [`NO_PACK`] was chosen.
    pub pack: Option<Pick>,
}

/// Context pack selected by the decide model and its probability.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Pick {
    pub name: String,
    pub probability: f64,
}

/// System One `choice` question for selecting a context pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PackQuestion {
    instructions: String,

    /// Configured `(name, when)` pairs in deterministic order.
    packs: Vec<(String, String)>,
}

impl PackQuestion {
    /// Build the pack question from `inject`, or return `None` if no packs are
    /// configured.
    pub(crate) fn new(inject: &InjectOptions) -> Option<Self> {
        if inject.packs.is_empty() {
            return None;
        }

        Some(Self {
            instructions: inject.instructions.clone(),
            packs: inject
                .packs
                .iter()
                .map(|(name, pack)| (name.clone(), pack.when.clone()))
                .collect(),
        })
    }

    /// Serialize this question as a System One `choice` definition.
    fn request(&self) -> Value {
        let mut criteria = Map::new();
        criteria.insert(NO_PACK.to_owned(), json!(NO_PACK_DESCRIPTION));
        for (name, when) in &self.packs {
            criteria.insert(name.clone(), json!(when));
        }

        json!({ "type": "choice", "instructions": self.instructions, "criteria": criteria })
    }

    /// Extract the selected pack from `body`, returning `None` if the model
    /// chose [`NO_PACK`] or if the pack answer is missing, unknown, or
    /// malformed.
    ///
    /// Pack selection is best-effort: an invalid pack answer skips the pack
    /// lookup without failing the action verdict.
    fn pick(&self, body: &Value) -> Option<Pick> {
        let answer = body.pointer(&format!("/answers/{PACK_QUESTION}"))?;
        let choice = answer.get("choice").and_then(Value::as_str)?;
        let (name, _) = self.packs.iter().find(|(name, _)| name == choice)?;
        let probability = answer
            .get("probabilities")
            .and_then(|probabilities| probabilities.get(choice))
            .and_then(Value::as_f64)
            .filter(|probability| (0.0..=1.0).contains(probability))?;

        Some(Pick {
            name: name.clone(),
            probability,
        })
    }
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
    packs: Option<PackQuestion>,
}

impl SystemOne {
    pub(crate) fn new(
        options: DecideOptions,
        token: Option<String>,
        packs: Option<PackQuestion>,
    ) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .build()
            .map_err(|error| format!("failed to set up the decide API client: {error}"))?;

        Ok(Self {
            client,
            options,
            token,
            packs,
        })
    }
}

impl Decide for SystemOne {
    fn decide(&self, state: String, actions: &'static [Action]) -> Decision {
        let body = request_body(&self.options, &state, actions, self.packs.as_ref());
        let mut request = self.client.post(&self.options.url).json(&body);
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        let packs = self.packs.clone();

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
            verdict(&body, actions, packs.as_ref())
        })
    }
}

/// Build the System One JSON request body for `state`, `actions`, and the
/// optional `packs` question.
pub(crate) fn request_body(
    options: &DecideOptions,
    state: &str,
    actions: &[Action],
    packs: Option<&PackQuestion>,
) -> Value {
    let criteria: Map<String, Value> = actions
        .iter()
        .map(|action| (action.key().to_owned(), json!(action.description())))
        .collect();
    let mut questions = Map::new();
    questions.insert(
        QUESTION.to_owned(),
        json!({ "type": "choice", "instructions": options.instructions, "criteria": criteria }),
    );
    if let Some(packs) = packs {
        questions.insert(PACK_QUESTION.to_owned(), packs.request());
    }

    json!({
        "model": options.model,
        "state": state,
        "questions": questions,
    })
}

/// Parse the chosen [`Action`], its probability, and the optional pack [`Pick`]
/// from a System One response.
pub(crate) fn verdict(
    body: &Value,
    actions: &[Action],
    packs: Option<&PackQuestion>,
) -> Result<Verdict, String> {
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
        pack: packs.and_then(|packs| packs.pick(body)),
    })
}

#[cfg(test)]
#[path = "decide_tests.rs"]
mod tests;
