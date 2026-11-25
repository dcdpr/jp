use clap::{ArgMatches, Command};
use jp_term::table::{DetailItem, Details};

use crate::{
    cli::CommandArgs,
    cmd::{ConversationLoadRequest, Output, conversation_id::FlagIds},
    ctx::Ctx,
    output::print_details,
};

/// `jp attachment ls`.
#[derive(Debug)]
pub(crate) struct Ls {
    /// The conversation to list; defaults to the session's active conversation.
    pub(super) target: FlagIds<true, false>,
}

impl CommandArgs for Ls {
    fn augment(cmd: Command) -> Command {
        FlagIds::<true, false>::augment(cmd)
    }

    fn from_matches(matches: &ArgMatches) -> Result<Self, clap::Error> {
        Ok(Self {
            target: FlagIds::from_matches(matches)?,
        })
    }
}

impl Ls {
    /// The conversation's config is layered into the invocation's, which is
    /// what `run` lists.
    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        ConversationLoadRequest::explicit_or_session_with_config(&self.target)
    }

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
