use clap::{Arg, ArgMatches, Command, value_parser};

use super::register_attachment;
use crate::{
    cli::{CommandArgs, required},
    cmd::Output,
    ctx::Ctx,
    parser::AttachmentUrlOrPath,
};

/// `jp attachment print`.
#[derive(Debug)]
pub(crate) struct Print {
    /// The attachment URL to preview.
    attachment: AttachmentUrlOrPath,
}

impl CommandArgs for Print {
    fn augment(cmd: Command) -> Command {
        cmd.arg_required_else_help(true).arg(
            Arg::new("attachment")
                .required(true)
                .value_name("ATTACHMENT")
                .value_parser(value_parser!(AttachmentUrlOrPath))
                .help("The attachment URL to preview"),
        )
    }

    fn from_matches(matches: &ArgMatches) -> Result<Self, clap::Error> {
        Ok(Self {
            attachment: required(matches, "attachment")?,
        })
    }
}

impl Print {
    pub(crate) async fn run(self, ctx: &mut Ctx) -> Output {
        let uri = self.attachment.parse(Some(ctx.workspace.root()))?;
        let attachments = register_attachment(ctx, uri).await?;

        for (idx, attachment) in attachments.iter().enumerate() {
            if idx > 0 {
                ctx.printer.println("");
            }

            match attachment.as_text() {
                Some(text) => ctx.printer.println(text),
                None => {
                    ctx.printer.eprintln(format!(
                        "Attachment '{}' is binary and cannot be previewed.",
                        attachment.source
                    ));
                }
            }
        }

        Ok(())
    }
}
