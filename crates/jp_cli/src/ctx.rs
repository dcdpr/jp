use std::io::{self, IsTerminal as _};

use jp_config::Config;
use jp_mcp::config::McpServer;
use jp_task::TaskHandler;
use jp_workspace::Workspace;

use crate::{Globals, Result};

/// Context for the CLI application
pub struct Ctx {
    /// The workspace.
    pub workspace: Workspace,

    /// Merged file/CLI configuration.
    pub config: Config,

    /// Global CLI arguments.
    pub term: Term,

    /// MCP client for interacting with MCP servers.
    pub mcp_client: jp_mcp::Client,

    pub task_handler: jp_task::TaskHandler,
}

pub struct Term {
    /// Global CLI arguments.
    pub args: Globals,

    /// Whether or not stdout is connected to a TTY.
    ///
    /// If you pipe (|) or redirect (>) the output, stdout is connected to a
    /// pipe or a regular file, respectively. These are not managed by the TTY
    /// subsystem.
    #[expect(dead_code)]
    pub is_tty: bool,
}

impl Ctx {
    /// Create a new context with the given workspace
    pub fn new(workspace: Workspace, args: Globals, config: Config) -> Self {
        Self {
            workspace,
            config,
            term: Term {
                args,
                is_tty: io::stdout().is_terminal(),
            },
            mcp_client: jp_mcp::Client::default(),
            task_handler: TaskHandler::default(),
        }
    }

    pub async fn configure_active_mcp_servers(&mut self) -> Result<()> {
        let conversation = self.workspace.get_active_conversation();

        let servers = conversation
            .context
            .mcp_server_ids
            .clone()
            .iter()
            .filter_map(|id| self.workspace.get_mcp_server(id).cloned())
            .collect::<Vec<McpServer>>();

        self.mcp_client.handle_servers(&servers).await?;

        Ok(())
    }
}

impl Drop for Ctx {
    fn drop(&mut self) {
        if let Err(err) = self.workspace.persist() {
            eprintln!("Failed to persist workspace: {err}");
        }
    }
}
