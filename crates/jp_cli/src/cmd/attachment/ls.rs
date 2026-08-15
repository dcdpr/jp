use jp_term::table::{DetailItem, Details};

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

        let mut items = vec![];
        for uri in uris {
            items.push(DetailItem::plain(uri.to_url()?.to_string()));
        }

        print_details(&ctx.printer, title.as_deref(), Details::Items(items));
        Ok(())
    }
}
