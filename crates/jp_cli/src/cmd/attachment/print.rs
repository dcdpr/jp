use super::register_attachment;
use crate::{cmd::Output, ctx::Ctx, parser::AttachmentUrlOrPath};

#[derive(Debug, clap::Args)]
#[command(arg_required_else_help(true))]
pub(crate) struct Print {
    /// The attachment URL to preview.
    attachment: AttachmentUrlOrPath,
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
