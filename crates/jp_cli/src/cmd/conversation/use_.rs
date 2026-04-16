use crossterm::style::Stylize as _;
use jp_conversation::ConversationId;
use jp_workspace::ConversationHandle;
use tracing::warn;

use crate::{
    cmd::{ConversationLoadRequest, Output, conversation_id::PositionalIds},
    ctx::Ctx,
};

#[derive(Debug, clap::Args)]
pub(crate) struct Use {
    #[command(flatten)]
    target: PositionalIds<true, false>,
}

impl Use {
    #[expect(clippy::needless_pass_by_value, clippy::unused_self)]
    pub(crate) fn run(self, ctx: &mut Ctx, handle: ConversationHandle) -> Output {
        let id = handle.id();

        let active_id = ctx
            .session
            .as_ref()
            .and_then(|s| ctx.workspace.session_active_conversation(s));

        if active_id == Some(id) {
            let id_fmt = id.to_string().bold().yellow();
            ctx.printer
                .println(format!("Already active conversation: {id_fmt}"));
            return Ok(());
        }

        let Some(session) = &ctx.session else {
            Err((
                1,
                "No session identity available. Set $JP_SESSION or run in a terminal with \
                 automatic session detection."
                    .to_string(),
            ))?;
            unreachable!()
        };

        let now = ctx.now();
        if let Err(error) = ctx
            .workspace
            .activate_session_conversation(session, id, now)
        {
            warn!(%error, "Failed to write session mapping.");
        }

        let from = active_id.map_or_else(
            || "(none)".grey().to_string(),
            |id| id.to_string().bold().grey().to_string(),
        );
        let to = id.to_string().bold().yellow();
        let title_suffix = conversation_title(ctx, id)
            .map(|t| format!(": {}", t.yellow()))
            .unwrap_or_default();
        ctx.printer.println(format!(
            "Switched active conversation from {from} to {to}{title_suffix}"
        ));
        Ok(())
    }

    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        ConversationLoadRequest::explicit_or_previous(&self.target)
    }
}

fn conversation_title(ctx: &Ctx, id: ConversationId) -> Option<String> {
    let h = ctx.workspace.acquire_conversation(&id).ok()?;
    ctx.workspace.metadata(&h).ok()?.title.clone()
}
