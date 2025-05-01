use comfy_table::{Cell, Row};

use crate::{cmd::Success, ctx::Ctx, error::Error, Output};

#[derive(Debug, clap::Args)]
pub struct Args {}

impl Args {
    #[expect(clippy::unused_self)]
    pub fn run(self, ctx: &mut Ctx) -> Output {
        let context = &mut ctx.workspace.get_active_conversation_mut().context;

        let mut uris = vec![];
        for handler in context.attachment_handlers.values() {
            uris.extend(
                handler
                    .list()
                    .map_err(|e| Error::Attachment(e.to_string()))?,
            );
        }

        if uris.is_empty() {
            return Ok("No attachments in current context.".into());
        }

        let title = Some("Attachments".to_owned());

        let mut rows = vec![];
        for uri in uris {
            let mut row = Row::new();
            row.add_cell(Cell::new(uri));
            rows.push(row);
        }

        Ok(Success::Details { title, rows })
    }
}
