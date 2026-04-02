use crossterm::style::Stylize as _;
use jp_workspace::ConversationHandle;
use tracing::warn;

use crate::{
    cmd::{ConversationLoadRequest, Output, conversation_id::PositionalIds},
    ctx::Ctx,
};

#[derive(Debug, clap::Args)]
pub(crate) struct Use {
    #[command(flatten)]
    target: PositionalIds<false, false>,
}

impl Use {
    #[expect(clippy::needless_pass_by_value, clippy::unused_self)]
    pub(crate) fn run(self, ctx: &mut Ctx, handle: ConversationHandle) -> Output {
        let id = handle.id();

        let active_id = ctx
            .session
            .as_ref()
            .and_then(|s| ctx.workspace.session_active_conversation(s));

        let id_fmt = id.to_string().bold().yellow();
        if active_id == Some(id) {
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
        ctx.printer.println(format!(
            "Switched active conversation from {from} to {}",
            id.to_string().bold().yellow()
        ));
        Ok(())
    }

    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        ConversationLoadRequest::explicit_or_previous(&self.target.ids)
    }
}
