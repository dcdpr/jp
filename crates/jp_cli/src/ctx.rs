use std::io::{self, IsTerminal as _};

use jp_config::{Config, PartialConfig};
use jp_mcp::{config::McpServer, server::embedded::EmbeddedServer};
use jp_task::TaskHandler;
use jp_workspace::Workspace;

use crate::{Globals, Result};

/// Context for the CLI application
pub(crate) struct Ctx {
    /// The workspace.
    pub(crate) workspace: Workspace,

    /// Merged file/CLI configuration.
    pub(crate) config: Config,

    /// Global CLI arguments.
    pub(crate) term: Term,

    /// MCP client for interacting with MCP servers.
    pub(crate) mcp_client: jp_mcp::Client,

    pub(crate) task_handler: jp_task::TaskHandler,
}

pub(crate) struct Term {
    /// Global CLI arguments.
    pub(crate) args: Globals,

    /// Whether or not stdout is connected to a TTY.
    ///
    /// If you pipe (|) or redirect (>) the output, stdout is connected to a
    /// pipe or a regular file, respectively. These are not managed by the TTY
    /// subsystem.
    pub(crate) is_tty: bool,
}

impl Ctx {
    /// Create a new context with the given workspace
    pub(crate) fn new(workspace: Workspace, args: Globals, config: Config) -> Self {
        let tools = workspace
            .mcp_tools()
            .cloned()
            .map(|v| (v.id.clone(), v))
            .collect();

        let server = EmbeddedServer::new(tools, workspace.root.clone());

        Self {
            workspace,
            config,
            term: Term {
                args,
                is_tty: io::stdout().is_terminal(),
            },
            mcp_client: jp_mcp::Client::default().with_embedded_server(server),
            task_handler: TaskHandler::default(),
        }
    }

    /// Activate and deactivate MCP servers based on the active conversation
    /// context.
    pub(crate) async fn configure_active_mcp_servers(&mut self) -> Result<()> {
        let servers = self
            .config
            .conversation
            .mcp_servers
            .clone()
            .iter()
            .filter_map(|id| self.workspace.get_mcp_server(id).cloned())
            .collect::<Vec<McpServer>>();

        self.mcp_client.handle_servers(&servers).await?;

        Ok(())
    }
}

/// A trait for converting any type into a partial [`Config`].
pub(crate) trait IntoPartialConfig {
    fn apply_cli_config(
        &self,
        workspace: Option<&Workspace>,
        partial: PartialConfig,
    ) -> std::result::Result<PartialConfig, Box<dyn std::error::Error + Send + Sync>>;

    #[expect(unused_variables)]
    fn apply_conversation_config(
        &self,
        workspace: Option<&Workspace>,
        partial: PartialConfig,
    ) -> std::result::Result<PartialConfig, Box<dyn std::error::Error + Send + Sync>> {
        Ok(partial)
    }
}

impl Drop for Ctx {
    fn drop(&mut self) {
        if let Err(err) = self.workspace.persist() {
            eprintln!("Failed to persist workspace: {err}");
        }
    }
}
