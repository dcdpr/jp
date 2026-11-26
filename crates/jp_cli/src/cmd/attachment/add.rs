use clap::{Arg, ArgAction, ArgMatches, Command, value_parser};
use jp_config::PartialAppConfig;
use jp_workspace::ConversationHandle;
use url::Url;

use super::{attachment_identity, canonical_attachment, lock_target, record_overrides};
use crate::{
    cli::{CommandArgs, many},
    cmd::{ConversationLoadRequest, Output, conversation_id::FlagIds, target::TargetGrammar},
    ctx::Ctx,
    parser::AttachmentUrlOrPath,
};

/// `jp attachment add`, and its hidden shorthand `jp aa`.
#[derive(Debug)]
pub(crate) struct Add {
    pub(super) attachments: Vec<AttachmentUrlOrPath>,
    pub(super) target: FlagIds<true, false>,
}

impl CommandArgs for Add {
    fn augment(cmd: Command) -> Command {
        let cmd = cmd.arg_required_else_help(true).arg(
            Arg::new("attachments")
                .action(ArgAction::Append)
                .num_args(1..)
                .required(true)
                .value_name("ATTACHMENTS")
                .value_parser(value_parser!(AttachmentUrlOrPath))
                .help("One or more attachments to add to the conversation")
                .long_help(
                    "One or more attachments to add to the conversation.\n\nIf the attachment is \
                     pointing to a file, then a file attachment is added, otherwise the \
                     attachment type can be added as a prefix.\n\nFor example, to add a `summary` \
                     attachment, use `summary://<path>`.\n\nAdding an attachment the conversation \
                     already has is not an error, but it is reported.",
                ),
        );

        FlagIds::<true, false>::augment(cmd)
    }

    fn from_matches(matches: &ArgMatches) -> Result<Self, clap::Error> {
        Ok(Self {
            attachments: many(matches, "attachments"),
            target: FlagIds::from_matches(matches)?,
        })
    }
}

impl Add {
    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        ConversationLoadRequest::explicit_or_session(&self.target)
    }

    pub(crate) async fn run(self, ctx: &mut Ctx, handles: Vec<ConversationHandle>) -> Output {
        // Pair each URL with its canonical form to validate it and check
        // whether it is already attached.
        let urls = self
            .attachments
            .iter()
            .map(|uri| {
                let url = uri.parse(Some(ctx.workspace.root()))?;
                Ok((canonical_attachment(&url)?, url))
            })
            .collect::<crate::error::Result<Vec<_>>>()?;

        let grammar = TargetGrammar::from_args(&self.target, false);
        let lock = lock_target(ctx, handles, grammar).await?;
        let id = lock.id();

        let mut seen = lock
            .events()
            .config()
            .map_err(jp_conversation::Error::from)?
            .conversation
            .attachments
            .iter()
            .map(|attachment| attachment.to_url().map(|url| attachment_identity(&url)))
            .collect::<Result<Vec<_>, _>>()?;

        let mut added: Vec<Url> = vec![];
        for (canonical, url) in urls {
            if seen.contains(&canonical) {
                ctx.printer
                    .eprintln(format!("Already attached to conversation {id}: {url}"));
                continue;
            }

            seen.push(canonical);
            added.push(url);
        }

        if added.is_empty() {
            return Ok(());
        }

        // `MergeableVec` defaults to the append strategy, so only the new
        // attachments need to be included in the delta.
        let mut overrides = PartialAppConfig::empty();
        for url in &added {
            overrides.conversation.attachments.push(url.clone().into());
        }

        record_overrides(ctx, lock, overrides)?;

        for url in added {
            ctx.printer
                .println(format!("Added {url} to conversation {id}"));
        }

        Ok(())
    }
}
