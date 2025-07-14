use std::fs;

use jp_config::{
    assignment::{AssignKeyValue as _, KvAssignment},
    PartialConfig,
};

use super::TargetWithConversation;
use crate::{ctx::Ctx, Error, Output};

#[derive(Debug, clap::Args)]
pub(crate) struct Set {
    /// The key to set.
    key: String,

    /// The value to set.
    value: String,

    #[command(flatten)]
    target: TargetWithConversation,

    /// Whether to parse the value as a JSON string.
    #[arg(long)]
    raw: bool,

    /// Whether to merge the value with any existing value.
    #[arg(long)]
    merge: bool,
}

impl Set {
    pub(crate) fn run(self, ctx: &mut Ctx) -> Output {
        let assignment: KvAssignment = format!(
            "{}{}{}={}",
            &self.key,
            if self.raw { ":" } else { "" },
            if self.merge { "+" } else { "" },
            self.value
        )
        .parse()?;

        if let Some(ref target) = self.target.conversation {
            let id = match target {
                Some(id) => id.parse()?,
                None => ctx.workspace.active_conversation_id(),
            };

            ctx.workspace
                .get_conversation_mut(&id)
                .ok_or(Error::NotFound("Conversation", id.to_string()))?
                .config_mut()
                .assign(assignment)?;

            return Ok(format!(
                "Set configuration value for {} in conversation {id:?}",
                self.key
            )
            .into());
        }

        let Some(mut config) = self.target.target.config_file(ctx)? else {
            unreachable!("target is either a path, or a conversation")
        };

        config.edit_content(|partial: &mut PartialConfig| {
            partial.assign(assignment)?;
            Ok(())
        })?;

        if let Some(parent) = config.path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&config.path, config.content)?;

        Ok(format!(
            "Set configuration value for {} in {}",
            self.key,
            config.path.display()
        )
        .into())
    }
}
