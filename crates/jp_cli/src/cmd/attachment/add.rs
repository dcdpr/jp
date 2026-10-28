use clap::{Arg, ArgAction, ArgMatches, Command, value_parser};
use jp_config::PartialAppConfig;
use jp_workspace::Workspace;

use super::validate_attachment;
use crate::{
    IntoPartialAppConfig,
    cli::{CommandArgs, many},
    cmd::Output,
    ctx::Ctx,
    parser::AttachmentUrlOrPath,
};

/// `jp attachment add`, and its hidden shorthand `jp aa`.
#[derive(Debug)]
pub(crate) struct Add {
    /// The attachments to add to the context.
    attachments: Vec<AttachmentUrlOrPath>,
}

impl CommandArgs for Add {
    fn augment(cmd: Command) -> Command {
        cmd.arg_required_else_help(true).arg(
            Arg::new("attachments")
                .action(ArgAction::Append)
                .num_args(1..)
                .value_name("ATTACHMENTS")
                .value_parser(value_parser!(AttachmentUrlOrPath))
                .help("One or more attachments to add to the context")
                .long_help(
                    "One or more attachments to add to the context.\n\nIf the attachment is \
                     pointing to a file, then a file attachment is added, otherwise the \
                     attachment type can be added as a prefix.\n\nFor example, to add a `summary` \
                     attachment, use `summary://<path>`.",
                ),
        )
    }

    fn from_matches(matches: &ArgMatches) -> Result<Self, clap::Error> {
        Ok(Self {
            attachments: many(matches, "attachments"),
        })
    }
}

impl Add {
    #[expect(clippy::unused_self, clippy::unnecessary_wraps)]
    pub(crate) fn run(self, _: &mut Ctx) -> Output {
        // See `apply_cli_config` for implementation.

        Ok(())
    }
}

impl IntoPartialAppConfig for Add {
    fn apply_cli_config(
        &self,
        workspace: Option<&Workspace>,
        mut partial: PartialAppConfig,
        _: Option<&PartialAppConfig>,
    ) -> std::result::Result<PartialAppConfig, Box<dyn std::error::Error + Send + Sync>> {
        for uri in &self.attachments {
            let uri = uri.parse(workspace.map(Workspace::root))?;
            validate_attachment(&uri)?;

            partial.conversation.attachments.push(uri.clone().into());
        }

        Ok(partial)
    }
}
