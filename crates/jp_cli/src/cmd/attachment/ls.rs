use jp_term::table::{DetailItem, Details};

use crate::{cmd::Output, ctx::Ctx, output::print_details};

#[derive(Debug, clap::Args)]
pub(crate) struct Ls {}

impl Ls {
    #[expect(clippy::unused_self)]
    pub(crate) fn run(self, ctx: &mut Ctx) -> Output {
        let uris = &ctx.config().conversation.attachments;

        let mut items = vec![];
        for uri in uris {
            items.push(DetailItem::plain(uri.to_url()?.to_string()));
        }

        // The text views swap an empty listing for a sentence; JSON keeps the
        // array, so a script sees one shape whether or not anything was
        // attached.
        if items.is_empty() && !ctx.printer.format().is_json() {
            ctx.printer.println("No attachments in current context.");
            return Ok(());
        }

        print_details(&ctx.printer, Some("Attachments"), Details::Items(items));
        Ok(())
    }
}
