use jp_config::PartialConfig;
use jp_mcp::config::McpServerId;
use jp_workspace::Workspace;

use crate::{
    ctx::{Ctx, IntoPartialConfig},
    Output,
};

#[derive(Debug, clap::Args)]
#[command(arg_required_else_help(true))]
pub(crate) struct Detach {
    /// Names of MCP servers to detach
    names: Vec<String>,

    /// ID of the conversation to detach the servers from. Defaults to the
    /// active conversation.
    #[arg(long = "id")]
    conversation_id: Option<String>,
}

impl Detach {
    #[expect(clippy::unnecessary_wraps)]
    pub(crate) fn run(self, _ctx: &mut Ctx) -> Output {
        Ok(format!("Detached MCP servers: {}", self.names.join(", ")).into())
    }
}

impl IntoPartialConfig for Detach {
    fn apply_cli_config(
        &self,
        _workspace: Option<&Workspace>,
        mut partial: PartialConfig,
    ) -> std::result::Result<PartialConfig, Box<dyn std::error::Error + Send + Sync>> {
        for id in self.names.iter().map(McpServerId::new) {
            partial
                .conversation
                .mcp_servers
                .get_or_insert_default()
                .retain(|v| *v != id);
        }

        Ok(partial)
    }
}
