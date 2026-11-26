use clap::{Arg, ArgAction, ArgMatches, Command, value_parser};
use jp_config::{
    PartialAppConfig, conversation::attachment::AttachmentConfig,
    types::vec::vec_to_mergeable_partial,
};
use jp_workspace::ConversationHandle;
use url::Url;

use super::{attachment_identity, lock_target, record_overrides};
use crate::{
    cli::{CommandArgs, many},
    cmd::{ConversationLoadRequest, Output, conversation_id::FlagIds, target::TargetGrammar},
    ctx::Ctx,
    parser::AttachmentUrlOrPath,
};

/// `jp attachment rm`.
#[derive(Debug)]
pub(crate) struct Rm {
    pub(super) attachments: Vec<AttachmentUrlOrPath>,
    pub(super) target: FlagIds<true, false>,
}

impl CommandArgs for Rm {
    fn augment(cmd: Command) -> Command {
        let cmd = cmd.arg(
            Arg::new("attachments")
                .action(ArgAction::Append)
                .num_args(1..)
                .required(true)
                .value_name("ATTACHMENTS")
                .value_parser(value_parser!(AttachmentUrlOrPath))
                .help("One or more attachments to remove from the conversation")
                .long_help(
                    "One or more attachments to remove from the conversation, written as `jp \
                     conversation show` lists them.\n\nAn attachment a config file declared is \
                     removed the same way: the conversation keeps its own copy of the \
                     configuration it was created with.\n\nRemoving an attachment the \
                     conversation doesn't have is not an error, but it is reported.",
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

impl Rm {
    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        ConversationLoadRequest::explicit_or_session(&self.target)
    }

    pub(crate) async fn run(self, ctx: &mut Ctx, handles: Vec<ConversationHandle>) -> Output {
        // Match by canonical URL so equivalent spellings remove the attachment,
        // while keeping the original argument for error reporting. File paths
        // are converted to URLs without checking whether the file exists on
        // disk.
        let to_remove = self
            .attachments
            .iter()
            .map(|uri| {
                let url = uri.parse_unchecked(Some(ctx.workspace.root()))?;
                Ok((attachment_identity(&url), url))
            })
            .collect::<crate::error::Result<Vec<_>>>()?;

        let grammar = TargetGrammar::from_args(&self.target, false);
        let lock = lock_target(ctx, handles, grammar).await?;
        let id = lock.id();

        let attachments = lock
            .events()
            .config()
            .map_err(jp_conversation::Error::from)?
            .conversation
            .attachments;

        let Split {
            kept,
            removed,
            missing,
        } = split(attachments, &to_remove)?;

        for url in missing {
            ctx.printer
                .eprintln(format!("Not attached to conversation {id}: {url}"));
        }

        if removed.is_empty() {
            return Ok(());
        }

        // Config deltas cannot remove individual list items, so replace the
        // list with the remaining attachments. Because a conversation's attachment
        // list is already pinned at creation, replacing it does not change how
        // file-layer configs are inherited.
        let mut overrides = PartialAppConfig::empty();
        overrides.conversation.attachments = vec_to_mergeable_partial(&kept);

        record_overrides(ctx, lock, overrides)?;

        for url in removed {
            ctx.printer
                .println(format!("Removed {url} from conversation {id}"));
        }

        Ok(())
    }
}

struct Split<'a> {
    kept: Vec<AttachmentConfig>,
    /// Stored URL of each removed attachment.
    removed: Vec<Url>,
    /// Arguments (as written) that matched no stored attachment.
    missing: Vec<&'a Url>,
}

/// Partition `attachments` against `to_remove` (`(canonical_url, written_url)`
/// pairs).
fn split(
    attachments: Vec<AttachmentConfig>,
    to_remove: &[(Url, Url)],
) -> crate::error::Result<Split<'_>> {
    let mut kept = vec![];
    let mut removed = vec![];
    let mut matched = vec![];
    for attachment in attachments {
        let url = attachment.to_url()?;
        let identity = attachment_identity(&url);
        if to_remove.iter().any(|(target, _)| *target == identity) {
            matched.push(identity);
            removed.push(url);
        } else {
            kept.push(attachment);
        }
    }

    let missing = to_remove
        .iter()
        .filter(|(identity, _)| !matched.contains(identity))
        .map(|(_, url)| url)
        .collect();

    Ok(Split {
        kept,
        removed,
        missing,
    })
}
