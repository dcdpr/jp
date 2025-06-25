use jp_config::PartialConfig;
use jp_mcp::config::McpServerId;
use jp_workspace::Workspace;

use crate::{
    ctx::{Ctx, IntoPartialConfig},
    error::{Error, Result},
    Output,
};

#[derive(Debug, clap::Args)]
#[command(arg_required_else_help(true))]
pub(crate) struct Attach {
    /// Names of MCP servers to attach
    names: Vec<String>,

    /// ID of the conversation to attach the servers to. Defaults to the active
    /// conversation.
    #[arg(long = "id")]
    conversation_id: Option<String>,
}

impl Attach {
    #[expect(clippy::unnecessary_wraps)]
    pub(crate) fn run(self, _ctx: &mut Ctx) -> Output {
        // Attaching servers is handled in `IntoPartialConfig` below.

        Ok(format!("Attached MCP servers: {}", self.names.join(", ")).into())
    }
}

impl IntoPartialConfig for Attach {
    fn apply_cli_config(
        &self,
        workspace: Option<&Workspace>,
        mut partial: PartialConfig,
    ) -> std::result::Result<PartialConfig, Box<dyn std::error::Error + Send + Sync>> {
        let server_ids = self
            .names
            .iter()
            .map(McpServerId::new)
            .filter_map(|id| {
                workspace.as_ref().map(|ws| {
                    ws.get_mcp_server(&id)
                        .map(|_| id.clone())
                        .ok_or(Error::NotFound("MCP server", id.to_string()))
                })
            })
            .collect::<Result<Vec<_>>>()?;

        for id in server_ids {
            partial
                .conversation
                .mcp_servers
                .get_or_insert_default()
                .push(id.clone());
        }

        Ok(partial)
    }
}
