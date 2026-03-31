use std::{str::FromStr as _, time::Duration};

use chrono::{DateTime, Utc};
use jp_conversation::ConversationId;
use jp_workspace::ConversationHandle;

use crate::{
    cmd::{ConversationLoadRequest, Output, conversation_id::PositionalIds},
    ctx::Ctx,
};

#[derive(Debug, clap::Args)]
pub(crate) struct Fork {
    #[command(flatten)]
    target: PositionalIds<true, true>,

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
    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        ConversationLoadRequest::explicit_or_session(&self.target.ids)
    }

    pub(crate) fn run(self, ctx: &mut Ctx, handles: Vec<ConversationHandle>) -> Output {
        for source in &handles {
            self.fork_one(ctx, source)?;
        }
        ctx.printer.println("Conversation forked.");
        Ok(())
    }

    fn fork_one(&self, ctx: &mut Ctx, source: &ConversationHandle) -> Output {
        let now = ctx.now();
        let mut new_conversation = ctx.workspace.metadata(source)?.clone();
        new_conversation.last_activated_at = now;
        new_conversation.expires_at = None;

        let mut new_events = ctx.workspace.events(source)?.clone().with_created_at(now);

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

        let new_handle = ctx.workspace.acquire_conversation(&new_id)?;
        let conv = ctx
            .workspace
            .lock_conversation(new_handle, None)?
            .expect("newly created conversation should not be locked")
            .into_mut();
        conv.update_events(|events| events.extend(new_events));

        if self.activate
            && let Some(session) = &ctx.session
            && let Err(error) = ctx
                .workspace
                .activate_session_conversation(session, new_id, now)
        {
            tracing::warn!(%error, "Failed to write session mapping.");
        }

        Ok(())
    }
}

#[cfg(test)]
#[path = "fork_tests.rs"]
mod tests;
