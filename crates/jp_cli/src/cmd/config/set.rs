use std::fs;

use jp_config::{
    PartialAppConfig,
    assignment::{AssignKeyValue as _, KvAssignment},
};
use jp_conversation::event::conversation_config;

use super::TargetWithConversation;
use crate::{Output, ctx::Ctx};

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

            let events = ctx.workspace.get_events(&id);
            let mut config = conversation_config(events);
            config.assign(assignment)?;
            ctx.workspace.add_event(id, config);

            return Ok(format!(
                "Set configuration value for {} in conversation {id:?}",
                self.key
            )
            .into());
        }

        let Some(mut config) = self.target.target.config_file(ctx)? else {
            unreachable!("target is either a path, or a conversation")
        };

        config.edit_content(|partial: &mut PartialAppConfig| {
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
