//! Pre-turn message classification and context injection.
//!
//! [`Intake`] wraps [`Inbox`], spawning the [`Decide`] call for the front
//! message and running any [`Injector`] context lookups when the message is
//! classified as [`Action::Answer`].
//! Lookups run before [`Inbox::decided`] is called so the inbox stays blocked
//! on the front message and later messages cannot overtake it.
//! [`Intake::decided`] logs the verdict and lookups, appends rendered context
//! to the message, and returns the message with the resolved [`Action`].

use std::{sync::Arc, time::Duration};

use chrono::NaiveDate;
use jp_plugin::log::{Level, fields};
use serde_json::json;
use tokio::{sync::mpsc, time::Instant};

use crate::{
    bridge::{Event, millis},
    decide::{Action, Decide, Decision, Verdict},
    host::Host,
    inbox::{Inbox, Incoming, Next},
    inject::{Injected, Injector, render},
    transport::Chat,
};

/// Output of the decision and context-lookup task for the front inbox message.
#[derive(Debug)]
pub(crate) struct Decided {
    result: Result<Verdict, String>,
    took: Duration,

    /// Context lookup results when the message resolves to [`Action::Answer`].
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

    /// Configure the [`Injector`] used to look up context for messages that
    /// will be answered.
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

    /// Pop the next message ready to route, or return `None` if the inbox is
    /// empty or awaiting a decision.
    ///
    /// If the front message requires classification, spawns a decision task
    /// that sends [`Event::Decided`] on `events`.
    pub(crate) fn next(&mut self, events: &mpsc::UnboundedSender<Event>) -> Option<Incoming> {
        loop {
            match self.inbox.next()? {
                Next::Take(incoming) => return Some(incoming),
                Next::Decide(decision, text) => self.spawn(decision, text, events),
            }
        }
    }

    /// Spawn a task that awaits `decision` for `text`, runs context lookups if
    /// the message will be answered, and sends [`Event::Decided`] on `events`.
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

    /// Complete the in-flight decision for the front inbox message, log the
    /// verdict and any lookups, and return the message (with rendered context
    /// appended for `today`) alongside the resolved [`Action`].
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
