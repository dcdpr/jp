use super::register_attachment;
use crate::{ctx::Ctx, Output};

#[derive(Debug, clap::Args)]
#[command(arg_required_else_help(true))]
pub struct Args {
    /// One or more attachments to add to the context.
    ///
    /// If the attachment is pointing to a file, then a file attachment is
    /// added, otherwise the attachment type can be added as a prefix.
    ///
    /// For example, to add a `summary` attachment, use `summary://<path>`.
    attachments: Vec<String>,
}

impl Args {
    pub fn run(self, ctx: &mut Ctx) -> Output {
        let context = &mut ctx.workspace.get_active_conversation_mut().context;

        for uri in &self.attachments {
            register_attachment(uri, context)?;
        }

        Ok(().into())
    }
}
