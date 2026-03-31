use std::fs;

use jp_config::{PartialAppConfig, fs::load_partial};
use jp_workspace::ConversationHandle;

use crate::{
    cmd::{ConversationLoadRequest, Output, conversation_id::FlagIds},
    config_pipeline,
    ctx::Ctx,
};

/// File target for `config set`. Mutually exclusive with `--id` (conversation
/// targeting). When none of these flags are set and no `--id` is given, the
/// workspace config file is used as the default.
#[derive(Debug, Clone, Copy, Default, PartialEq, clap::Args)]
#[group(required = false, multiple = false)]
struct FileTarget {
    /// Write to the workspace's user-specific configuration file.
    #[arg(long, conflicts_with = "id")]
    user_workspace: bool,

    /// Write to the global user-specific configuration file.
    #[arg(long, conflicts_with = "id")]
    user_global: bool,

    /// Write to the current-working-directory configuration file.
    #[arg(long, conflicts_with = "id")]
    cwd: bool,
}

#[derive(Debug, clap::Args)]
pub(crate) struct Set {
    #[command(flatten)]
    file_target: FileTarget,

    #[command(flatten)]
    conversation: FlagIds<false, true>,
}

impl Set {
    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        ConversationLoadRequest::explicit_or_none(&self.conversation.ids)
    }

    pub(crate) fn run(self, ctx: &mut Ctx, handles: Vec<ConversationHandle>) -> Output {
        let base = ctx.config().to_partial();
        let config_delta = config_pipeline::build_partial_from_cfg_args(
            &ctx.term.args.config,
            &base,
            Some(&ctx.workspace),
        )?;

        if handles.is_empty() {
            self.set_in_file(ctx, config_delta)
        } else {
            Self::set_in_conversations(ctx, handles, config_delta)
        }
    }

    fn set_in_conversations(
        ctx: &mut Ctx,
        handles: Vec<ConversationHandle>,
        mut config_delta: PartialAppConfig,
    ) -> Output {
        config_delta.resolve_model_aliases(&ctx.config().providers.llm.aliases);

        let session = ctx.session.as_ref().map(|s| s.id.as_str().to_owned());

        for handle in handles {
            let id = handle.id();
            let conv = ctx
                .workspace
                .lock_conversation(handle, session.as_deref())?
                .ok_or(crate::error::Error::LockTimeout(id))?
                .into_mut();

            conv.update_events(|events| events.add_config_delta(config_delta.clone()));
            ctx.printer
                .println(format!("Set configuration in conversation {id}"));
        }
        Ok(())
    }

    fn set_in_file(self, ctx: &mut Ctx, config_delta: PartialAppConfig) -> Output {
        let target = super::Target {
            user_workspace: self.file_target.user_workspace,
            user_global: self.file_target.user_global,
            cwd: self.file_target.cwd,
        };

        let Some(mut config) = target.config_file(ctx)? else {
            return Err("No configuration file found for the given target.".into());
        };

        config.edit_content(|partial: &mut PartialAppConfig| {
            *partial = load_partial(partial.clone(), config_delta)?;
            Ok(())
        })?;

        if let Some(parent) = config.path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&config.path, &config.content)?;

        ctx.printer
            .println(format!("Set configuration in {}", config.path));
        Ok(())
    }
}

#[cfg(test)]
#[path = "set_tests.rs"]
mod tests;
