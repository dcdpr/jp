use jp_term::table::DetailRow;

use crate::{
    cmd::Output,
    ctx::Ctx,
    output::{print_details, print_json},
};

#[derive(Debug, clap::Args)]
pub(crate) struct Ls {}

impl Ls {
    #[expect(clippy::unused_self)]
    pub(crate) fn run(self, ctx: &mut Ctx) -> Output {
        let uris = &ctx.config().conversation.attachments;

        if uris.is_empty() {
            // Machine-readable formats get an empty payload, not prose.
            if ctx.printer.format().is_json() {
                print_json(&ctx.printer, &serde_json::json!([]));
            } else {
                ctx.printer.println("No attachments in current context.");
            }
            return Ok(());
        }

        let title = Some("Attachments".to_owned());

        // The JSON payload is a plain array of attachment URLs.
        let mut rows = vec![];
        let mut urls = vec![];
        for uri in uris {
            let url = uri.to_url()?;
            urls.push(url.to_string());
            rows.push(DetailRow::bare(url));
        }

        print_details(
            &ctx.printer,
            title.as_deref(),
            rows,
            &serde_json::json!(urls),
        );
        Ok(())
    }
}
