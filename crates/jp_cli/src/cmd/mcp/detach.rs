use std::str::FromStr as _;

use jp_conversation::ConversationId;
use jp_mcp::config::McpServerId;

use crate::{ctx::Ctx, error::Error, Output};

#[derive(Debug, clap::Args)]
#[command(arg_required_else_help(true))]
pub struct Args {
    /// Names of MCP servers to detach
    pub names: Vec<String>,

    /// ID of the conversation to detach the servers from. Defaults to the
    /// active conversation.
    #[arg(long = "id")]
    pub conversation_id: Option<String>,
}

impl Args {
    pub fn run(self, ctx: &mut Ctx) -> Output {
        let id = self.conversation_id.map_or_else(
            || Ok(ctx.workspace.active_conversation_id()),
            |v| ConversationId::from_str(&v).map_err(Error::from),
        )?;

        let conversation = ctx
            .workspace
            .get_conversation_mut(&id)
            .ok_or(Error::NotFound("conversation", id.to_string()))?;

        for id in self.names.iter().map(McpServerId::new) {
            conversation.context.mcp_server_ids.remove(&id);
        }

        Ok(format!("Detached MCP servers: {}", self.names.join(", ")).into())
    }
}
