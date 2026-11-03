use clap::{Arg, ArgAction, ArgMatches, Command, value_parser};
use jp_config::{Config as _, PartialAppConfig, conversation::attachment::AttachmentConfig};
use jp_workspace::Workspace;

use crate::{
    IntoPartialAppConfig,
    cli::{CommandArgs, many},
    cmd::Output,
    ctx::Ctx,
    parser::AttachmentUrlOrPath,
};

/// `jp attachment rm`.
#[derive(Debug)]
pub(crate) struct Rm {
    attachments: Vec<AttachmentUrlOrPath>,
}

impl CommandArgs for Rm {
    fn augment(cmd: Command) -> Command {
        cmd.arg(
            Arg::new("attachments")
                .action(ArgAction::Append)
                .num_args(1..)
                .value_name("ATTACHMENTS")
                .value_parser(value_parser!(AttachmentUrlOrPath)),
        )
    }

    fn from_matches(matches: &ArgMatches) -> Result<Self, clap::Error> {
        Ok(Self {
            attachments: many(matches, "attachments"),
        })
    }
}

impl Rm {
    #[expect(clippy::unused_self, clippy::unnecessary_wraps)]
    pub(crate) fn run(self, _: &mut Ctx) -> Output {
        // See `apply_cli_config` for implementation.

        Ok(())
    }
}

impl IntoPartialAppConfig for Rm {
    fn apply_cli_config(
        &self,
        workspace: Option<&Workspace>,
        mut partial: PartialAppConfig,
        _: Option<&PartialAppConfig>,
    ) -> std::result::Result<PartialAppConfig, Box<dyn std::error::Error + Send + Sync>> {
        let mut attachments = vec![];

        let to_remove_attachments = self
            .attachments
            .iter()
            .map(|v| v.parse(workspace.map(Workspace::root)))
            .collect::<Result<Vec<_>, _>>()?;

        for attachment in partial.conversation.attachments {
            let url = AttachmentConfig::from_partial(attachment.clone(), vec![])?.to_url()?;
            if !to_remove_attachments.contains(&url) {
                attachments.push(attachment);
            }
        }

        partial.conversation.attachments = attachments.into();
        Ok(partial)
    }
}
