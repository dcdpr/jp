//! Inbound messages on their way to a turn: decided, then given context.
//!
//! [`Intake`] wraps the [`Inbox`].
//! For the front message it spawns the decide call and, when the message is to
//! be answered, the context lookups, which run while the inbox is still blocked
//! on that message so later messages can't overtake it.
//! [`Intake::decided`] then hands the bridge the message, with its context
//! appended, and the [`Action`] to take.

use std::{sync::Arc, time::Duration};

use chrono::NaiveDate;
use serde_json::json;
use tokio::{sync::mpsc, time::Instant};

use crate::{
    bridge::{Event, millis},
    decide::{Action, Decide, Decision, Verdict},
    host::{Host, Level, fields},
    inbox::{Inbox, Incoming, Next},
    inject::{Injected, Injector, render},
    transport::Chat,
};

/// What the decision task for the front message reports.
#[derive(Debug)]
pub(crate) struct Decided {
    result: Result<Verdict, String>,
    took: Duration,

    /// The context looked up for the message, when it is to be answered.
    injected: Option<Injected>,
}

#[derive(Default)]
pub(crate) struct Intake {
    inbox: Inbox,

    /// Minimum probability required to act on a [`Verdict`].
    threshold: f64,

    injector: Option<Arc<Injector>>,
}

impl Intake {
    /// Classify messages with `decider`, acting on a verdict whose probability
    /// is at least `threshold`.
    pub(crate) fn decide_with(&mut self, decider: Option<Arc<dyn Decide>>, threshold: f64) {
        self.inbox.decide_with(decider);
        self.threshold = threshold;
    }

    /// Look up context for messages to be answered with `injector`.
    pub(crate) fn inject_with(&mut self, injector: Option<Arc<Injector>>) {
        self.injector = injector;
    }

    /// Record `id` as seen and return `true` if this is the first sighting.
    pub(crate) fn first_sighting(&mut self, id: &str) -> bool {
        self.inbox.first_sighting(id)
    }

    /// Append `line` to the recent transcript for `chat`.
    pub(crate) fn remember(&mut self, chat: &Chat, line: &str) {
        self.inbox.remember(chat, line);
    }

    /// Enqueue a message from `person` in `chat`.
    pub(crate) fn push(&mut self, chat: Chat, person: String, text: String) {
        self.inbox.push(chat, person, text);
    }

    /// The next message ready to route, or `None` once the inbox is empty or
    /// waiting on a decision.
    ///
    /// A message that needs deciding spawns its decision task, which reports
    /// [`Event::Decided`] on `events`.
    pub(crate) fn next(&mut self, events: &mpsc::UnboundedSender<Event>) -> Option<Incoming> {
        loop {
            match self.inbox.next()? {
                Next::Take(incoming) => return Some(incoming),
                Next::Decide(decision, text) => self.spawn(decision, text, events),
            }
        }
    }

    /// Spawn the task awaiting `decision` for the message `text`, which looks
    /// up its context when it is to be answered.
    fn spawn(&self, decision: Decision, text: String, events: &mpsc::UnboundedSender<Event>) {
        let events = events.clone();
        let injector = self.injector.clone();
        let threshold = self.threshold;

        tokio::spawn(async move {
            let asked = Instant::now();
            let result = decision.await;
            let took = asked.elapsed();
            let injected = match injector {
                Some(injector) => look_up(&injector, &text, result.as_ref().ok(), threshold).await,
                None => None,
            };
            drop(events.send(Event::Decided(Decided {
                result,
                took,
                injected,
            })));
        });
    }

    /// Take the message `decided` was for, log the decision, and return the
    /// message with its context appended and the action to take.
    ///
    /// `today` dates the appended context.
    pub(crate) fn decided<H: Host>(
        &mut self,
        decided: Decided,
        host: &H,
        today: NaiveDate,
    ) -> Option<(Incoming, Action)> {
        let incoming = self.inbox.decided()?;
        log_decision(host, &decided.result, &incoming.chat, decided.took);
        let action = action(decided.result.as_ref().ok(), self.threshold);
        let incoming = match (action, decided.injected) {
            (Action::Answer, Some(injected)) => self.with_context(incoming, &injected, host, today),
            _ => incoming,
        };

        Some((incoming, action))
    }

    /// Log the lookup results in `injected` and append any rendered sections to
    /// `incoming.text`.
    fn with_context<H: Host>(
        &self,
        mut incoming: Incoming,
        injected: &Injected,
        host: &H,
        today: NaiveDate,
    ) -> Incoming {
        let chat = incoming.chat.to_string();
        for (lookup, error) in &injected.failures {
            host.log_with(
                Level::WARN,
                "Context lookup failed.",
                fields([
                    ("chat", chat.as_str().into()),
                    ("lookup", lookup.title.as_str().into()),
                    ("error", error.as_str().into()),
                ]),
            );
        }

        let titles: Vec<&str> = injected
            .sections
            .iter()
            .map(|section| section.lookup.title.as_str())
            .collect();
        host.log_with(
            Level::INFO,
            "Context looked up.",
            fields([
                ("chat", chat.as_str().into()),
                ("lookups", json!(titles)),
                ("failed", injected.failures.len().into()),
                ("took_ms", millis(injected.took)),
            ]),
        );

        if let Some(injector) = &self.injector
            && !injected.sections.is_empty()
        {
            incoming.text = render(
                &incoming.text,
                today,
                &injected.sections,
                injector.max_chars(),
            );
        }

        incoming
    }
}

/// Log the decide API's answer for a message in `chat`, and how long it took.
fn log_decision<H: Host>(host: &H, result: &Result<Verdict, String>, chat: &Chat, took: Duration) {
    match result {
        Ok(verdict) => {
            let mut data = fields([
                ("chat", chat.to_string().into()),
                ("action", verdict.action.key().into()),
                ("probability", verdict.probability.into()),
                ("took_ms", millis(took)),
            ]);
            if let Some(pick) = &verdict.pack {
                data.insert("pack".to_owned(), pick.name.clone().into());
                data.insert("pack_probability".to_owned(), pick.probability.into());
            }
            host.log_with(Level::INFO, "Message classified.", data);
        }
        Err(error) => host.log_with(
            Level::WARN,
            "Decide failed, answering.",
            fields([
                ("chat", chat.to_string().into()),
                ("error", error.as_str().into()),
                ("took_ms", millis(took)),
            ]),
        ),
    }
}

/// Resolve the action to take from the decider's `verdict`.
///
/// Falls back to [`Action::Answer`] when the request failed or the verdict's
/// probability is below `threshold`.
fn action(verdict: Option<&Verdict>, threshold: f64) -> Action {
    verdict
        .filter(|verdict| verdict.probability >= threshold)
        .map_or(Action::Answer, |verdict| verdict.action)
}

/// Run context lookups for `text` when `verdict` resolves to
/// [`Action::Answer`], returning `None` if the message is not answered or
/// matches no lookups.
async fn look_up(
    injector: &Injector,
    text: &str,
    verdict: Option<&Verdict>,
    threshold: f64,
) -> Option<Injected> {
    if action(verdict, threshold) != Action::Answer {
        return None;
    }
    let lookups = injector.lookups(text, verdict.and_then(|verdict| verdict.pack.as_ref()));
    if lookups.is_empty() {
        return None;
    }

    Some(injector.inject(lookups).await)
}
