use comfy_table::{Cell, Row};

use crate::{cmd::Output, ctx::Ctx, output::print_details};

#[derive(Debug, clap::Args)]
pub(crate) struct Ls {}

impl Ls {
    #[expect(clippy::unused_self)]
    pub(crate) fn run(self, ctx: &mut Ctx) -> Output {
        let uris = &ctx.config().conversation.attachments;

        if uris.is_empty() {
            ctx.printer.println("No attachments in current context.");
            return Ok(());
        }

        let title = Some("Attachments".to_owned());

        let mut rows = vec![];
        for uri in uris {
            let mut row = Row::new();
            row.add_cell(Cell::new(uri.to_url()?));
            rows.push(row);
        }

        print_details(&ctx.printer, title.as_deref(), rows);
        Ok(())
    }
}
