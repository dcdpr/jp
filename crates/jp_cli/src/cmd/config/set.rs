use std::fs;

use clap::{ArgMatches, Command};
use jp_config::PartialAppConfig;
use jp_conversation::stream::ApplyDelta;
use jp_workspace::ConversationHandle;

use super::Target;
use crate::{
    cli::CommandArgs,
    cmd::{
        ConversationLoadRequest, Output,
        conversation_id::FlagIds,
        lock::{LockOutcome, LockRequest, acquire_lock},
    },
    config_pipeline,
    ctx::Ctx,
};

/// `jp config set`.
///
/// Writes to the conversations `--id` names, or else to one configuration file:
/// the workspace's, when no target flag is given.
#[derive(Debug)]
pub(crate) struct Set {
    file_target: Target,
    conversation: FlagIds<false, true>,
}

impl CommandArgs for Set {
    fn augment(cmd: Command) -> Command {
        let cmd = Target::augment(
            cmd,
            "FileTarget",
            [
                "Write to the workspace's user-specific configuration file",
                "Write to the global user-specific configuration file",
                "Write to the current-working-directory configuration file",
            ],
            Some("id"),
        );

        FlagIds::<false, true>::augment(cmd)
    }

    fn from_matches(matches: &ArgMatches) -> Result<Self, clap::Error> {
        Ok(Self {
            file_target: Target::from_matches(matches),
            conversation: FlagIds::from_matches(matches)?,
        })
    }
}

impl Set {
    pub(crate) async fn run(self, ctx: &mut Ctx, handles: Vec<ConversationHandle>) -> Output {
        let base = ctx.config().to_partial();
        let config_delta = config_pipeline::build_partial_from_cfg_args(
            &ctx.term.args.config,
            &base,
            Some(&ctx.workspace),
            ctx.fs_backend.as_deref(),
        )?;

        if handles.is_empty() {
            self.set_in_file(ctx, &config_delta)
        } else {
            Self::set_in_conversations(ctx, handles, config_delta).await
        }
    }

    async fn set_in_conversations(
        ctx: &mut Ctx,
        handles: Vec<ConversationHandle>,
        mut config_delta: PartialAppConfig,
    ) -> Output {
        config_delta.resolve_model_aliases(&ctx.config().providers.llm.aliases);

        for handle in handles {
            let lock = match acquire_lock(LockRequest::from_ctx(handle, ctx)).await? {
                LockOutcome::Acquired(lock) => lock,
                LockOutcome::NewConversation => unreachable!("new conversation not allowed"),
                LockOutcome::ForkConversation(_) => unreachable!("fork not allowed"),
            };

            let id = lock.id();
            let current = lock
                .events()
                .config_partial()
                .map_err(jp_conversation::Error::from)?;
            let record = config_pipeline::override_to_record(&current, config_delta.clone())?;

            if let Some((delta, unsets)) = record {
                let apply = ApplyDelta::with_unsets(ctx.now(), delta, unsets);
                lock.into_mut()
                    .update_events_and_flush(|events| events.add_config_delta(apply))?;
            }

            ctx.printer
                .println(format!("Set configuration in conversation {id}"));
        }
        Ok(())
    }

    fn set_in_file(self, ctx: &mut Ctx, config_delta: &PartialAppConfig) -> Output {
        let Some(mut config) = self.file_target.config_file(ctx)? else {
            return Err("No configuration file found for the given target.".into());
        };

        config.merge_delta(&config_delta)?;

        if let Some(parent) = config.path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&config.path, &config.content)?;

        ctx.printer
            .println(format!("Set configuration in {}", config.path));
        Ok(())
    }

    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        ConversationLoadRequest::explicit_or_none(&self.conversation)
    }
}

#[cfg(test)]
#[path = "set_tests.rs"]
mod tests;
