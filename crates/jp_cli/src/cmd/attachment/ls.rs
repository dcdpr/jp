use clap::{ArgMatches, Command};
use jp_term::table::{DetailItem, Details};
use jp_workspace::ConversationHandle;

use super::attachment_url;
use crate::{
    cli::CommandArgs,
    cmd::{ConversationLoadRequest, Output, conversation_id::FlagIds, target::TargetGrammar},
    ctx::Ctx,
    error::Error,
    output::print_details,
};

/// `jp attachment ls`.
#[derive(Debug)]
pub(crate) struct Ls {
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
    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        ConversationLoadRequest::explicit_or_session(&self.target)
    }

    /// List the attachments stored on the conversation.
    ///
    /// `--cfg` overrides on the current invocation are not stored on the
    /// conversation and are not listed.
    pub(crate) fn run(self, ctx: &mut Ctx, handles: &[ConversationHandle]) -> Output {
        let Some(handle) = handles.first() else {
            return Err(Error::from(TargetGrammar::from_args(&self.target, false)).into());
        };

        let config = ctx
            .workspace
            .events(handle)?
            .config()
            .map_err(jp_conversation::Error::from)?;

        let mut items = vec![];
        for attachment in &config.conversation.attachments {
            items.push(DetailItem::plain(attachment_url(attachment)?.to_string()));
        }

        // Text output replaces an empty list with a message; JSON preserves
        // the empty array so consumers always receive the same schema.
        if items.is_empty() && !ctx.printer.format().is_json() {
            ctx.printer
                .println(format!("No attachments in conversation {}.", handle.id()));
            return Ok(());
        }

        print_details(&ctx.printer, Some("Attachments"), Details::Items(items));
        Ok(())
    }
}
