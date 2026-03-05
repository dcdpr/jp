use std::{str::FromStr as _, time::Duration};

use chrono::{DateTime, Utc};
use jp_conversation::ConversationId;

use crate::{cmd::Output, ctx::Ctx};

#[derive(Debug, clap::Args)]
pub(crate) struct Fork {
    /// Conversation ID to fork.
    ///
    /// Defaults to the active conversation if not specified.
    id: Option<ConversationId>,

    #[arg(short, long, default_value = "false")]
    activate: bool,

    /// Ignore all conversation events *before* the specified timestamp.
    ///
    /// Timestamp can be relative (5days, 2mins, etc) or absolute. Can be used
    /// in combination with `--until`.
    #[arg(long, value_parser = parse_duration)]
    from: Option<DateTime<Utc>>,

    /// Ignore all conversation events *after* the specified timestamp.
    ///
    /// Timestamp can be relative (5days, 2mins, etc) or absolute. Can be used
    /// in combination with `--until`.
    #[arg(long, value_parser = parse_duration)]
    until: Option<DateTime<Utc>>,

    /// Fork the last N turns of the conversation. Defaults to 1.
    #[arg(long, short = 'l')]
    last: Option<Option<usize>>,
}

fn parse_duration(s: &str) -> Result<DateTime<Utc>, String> {
    humantime::Duration::from_str(s)
        .map(|d| Utc::now() - Duration::from(d))
        .map_err(|e| e.to_string())
        .or_else(|_| {
            humantime::parse_rfc3339_weak(s)
                .map(Into::into)
                .map_err(|e| e.to_string())
        })
}

impl Fork {
    pub(crate) fn run(self, ctx: &mut Ctx) -> Output {
        let now = ctx.now();

        let original_id = self
            .id
            .unwrap_or_else(|| ctx.workspace.active_conversation_id());

        let mut new_conversation = ctx.workspace.try_get_conversation(&original_id)?.clone();
        new_conversation.last_activated_at = now;
        new_conversation.expires_at = None;

        let mut new_events = ctx
            .workspace
            .try_get_events(&original_id)?
            .clone()
            .with_created_at(now);

        new_events.retain(|event| {
            self.from.is_none_or(|from| event.timestamp >= from)
                && self.until.is_none_or(|until| event.timestamp <= until)
        });

        if let Some(last) = self.last {
            let n = last.unwrap_or(1);
            let turn_count = new_events
                .iter()
                .filter(|e| e.event.is_turn_start())
                .count();

            if turn_count > n {
                let skip = turn_count - n;
                let mut turns_seen = 0;
                let mut keeping = false;

                new_events.retain(|event| {
                    if event.is_turn_start() {
                        turns_seen += 1;
                        if turns_seen > skip {
                            keeping = true;
                        }
                    }
                    keeping
                });
            }
        }

        new_events.sanitize();

        let new_id = ConversationId::try_from(ctx.now())?;
        ctx.workspace.create_conversation_with_id(
            new_id,
            new_conversation,
            new_events.base_config(),
        );

        ctx.workspace
            .try_get_events_mut(&new_id)?
            .extend(new_events);

        if self.activate {
            ctx.workspace.set_active_conversation_id(new_id, now)?;
        }

        ctx.printer.println("Conversation forked.");
        Ok(())
    }
}

#[cfg(test)]
#[path = "fork_tests.rs"]
mod tests;
