use std::str::FromStr as _;

use jp_conversation::ConversationId;
use jp_mcp::config::McpServerId;

use crate::{
    ctx::Ctx,
    error::{Error, Result},
    Output,
};

#[derive(Debug, clap::Args)]
#[command(arg_required_else_help(true))]
pub struct Args {
    /// Names of MCP servers to attach
    pub names: Vec<String>,

    /// ID of the conversation to attach the servers to. Defaults to the active
    /// conversation.
    #[arg(long = "id")]
    pub conversation_id: Option<String>,
}

impl Args {
    pub fn run(self, ctx: &mut Ctx) -> Output {
        let conversation_id = self.conversation_id.map_or_else(
            || Ok(ctx.workspace.active_conversation_id()),
            |v| ConversationId::from_str(&v).map_err(Error::from),
        )?;

        let server_ids = self
            .names
            .iter()
            .map(McpServerId::new)
            .map(|id| {
                ctx.workspace
                    .get_mcp_server(&id)
                    .map(|_| id.clone())
                    .ok_or(Error::NotFound("MCP server", id.to_string()))
            })
            .collect::<Result<Vec<_>>>()?;

        let conversation = ctx
            .workspace
            .get_conversation_mut(&conversation_id)
            .ok_or(Error::NotFound("conversation", conversation_id.to_string()))?;

        for id in server_ids {
            conversation.context.mcp_server_ids.insert(id.clone());
        }

        Ok(format!("Attached MCP servers: {}", self.names.join(", ")).into())
    }
}
