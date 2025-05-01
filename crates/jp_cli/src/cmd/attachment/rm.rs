use super::unregister_attachment;
use crate::{ctx::Ctx, Output};

#[derive(Debug, clap::Args)]
pub struct Args {
    attachments: Vec<String>,
}

impl Args {
    pub fn run(self, ctx: &mut Ctx) -> Output {
        let context = &mut ctx.workspace.get_active_conversation_mut().context;

        for uri in &self.attachments {
            unregister_attachment(uri, context)?;
        }

        Ok(().into())
    }
}
