use jp_workspace::ConversationHandle;

use crate::{
    cmd::{
        ConversationLoadRequest, Output,
        conversation_id::PositionalIds,
        label::{self, LabelDirectives, resolve::Resolver},
        lock::{LockOutcome, LockRequest, acquire_lock},
    },
    ctx::Ctx,
    error::Error,
};

/// Set or remove labels on a single conversation.
///
/// Targets one conversation so `--label=:name` can resolve a
/// `conversation.labels` rule against that conversation's effective config.
/// `jp conversation edit --label` covers bulk literal labelling across several
/// conversations.
#[derive(Debug, clap::Args)]
pub(crate) struct Label {
    #[command(flatten)]
    target: PositionalIds<true, false>,

    #[command(flatten)]
    directives: LabelDirectives<true, true, true>,
}

impl Label {
    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        // Config loading is what makes `--label=:name` resolvable: the rule
        // comes from this conversation's effective config, not the workspace's.
        let mut request = ConversationLoadRequest::explicit_or_session(&self.target);
        request.config_conversation = Some(0);
        request
    }

    pub(crate) async fn run(self, ctx: &mut Ctx, handles: Vec<ConversationHandle>) -> Output {
        if self.directives.is_empty() {
            return Err(Error::Label(
                "no labels given; pass `--label KEY[=VALUE]`, `--label :NAME`, or `--no-label`"
                    .to_owned(),
            )
            .into());
        }

        // Only the first handle's config is layered, so a second target would
        // silently resolve its aliases against the wrong conversation. The clap
        // types reject multi-target keywords, but `+archived` slips through
        // their check, so refuse here rather than relabel the wrong thing.
        let mut handles = handles.into_iter();
        let handle = handles.next().ok_or(Error::NoConversationTarget)?;
        if handles.next().is_some() {
            return Err(Error::Label(
                "`jp conversation label` targets one conversation; use `jp conversation edit \
                 --label` to set literal labels on several at once"
                    .to_owned(),
            )
            .into());
        }

        let config = ctx.config();
        let resolver = Resolver::new(
            &config.conversation.labels,
            ctx.workspace.root(),
            ctx.term.is_tty,
            &ctx.printer,
        );
        let directives = label::expand_aliases(&self.directives, &resolver).await?;

        let lock = match acquire_lock(LockRequest::from_ctx(handle, ctx)).await? {
            LockOutcome::Acquired(lock) => lock,
            LockOutcome::NewConversation | LockOutcome::ForkConversation(_) => {
                unreachable!("neither is offered")
            }
        };

        lock.as_mut()
            .update_metadata(|m| label::apply(&mut m.labels, &directives));

        ctx.printer.println("Conversation updated.");
        Ok(())
    }
}
