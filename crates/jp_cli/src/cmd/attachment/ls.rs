use comfy_table::{Cell, Row};

use crate::{cmd::Success, ctx::Ctx, Output};

#[derive(Debug, clap::Args)]
pub(crate) struct Ls {}

impl Ls {
    #[expect(clippy::unused_self, clippy::unnecessary_wraps)]
    pub(crate) fn run(self, ctx: &mut Ctx) -> Output {
        let uris = &ctx.config.conversation.attachments;

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
