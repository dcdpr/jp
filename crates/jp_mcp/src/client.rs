use std::{collections::HashMap, env, process::Stdio, time::Duration};

use rmcp::{
    model::{CallToolRequestParam, CallToolResult, Tool},
    service::{RoleClient, RunningService, ServiceExt},
    transport::TokioChildProcess,
};
use tokio::process::Command;
use tracing::trace;

use crate::{config::McpServer, error::Result, transport::Transport, Error};

/// Manages multiple MCP clients and delegates operations to them
#[derive(Default)]
pub struct Client {
    clients: HashMap<String, RunningService<RoleClient, ()>>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("clients", &self.clients.keys())
            .finish()
    }
}

impl Client {
    /// Create a new MCP manager and connect to all configured servers
    pub async fn new(servers: &[&McpServer]) -> Result<Self> {
        let mut clients = HashMap::new();

        for server in servers {
            let client = Self::create_client(server).await?;
            clients.insert(server.id.to_string(), client);
        }

        Ok(Self { clients })
    }

    /// Get all available tools from all connected MCP servers
    pub async fn list_tools(&self) -> Result<Vec<Tool>> {
        let mut tools = vec![];

        for client in self.clients.values() {
            tools.extend(client.peer().list_all_tools().await?);
        }

        Ok(tools)
    }

    /// Call a tool by name with given parameters.
    pub async fn call_tool(
        &self,
        tool_name: &str,
        params: serde_json::Value,
    ) -> Result<CallToolResult> {
        for client in self.clients.values() {
            let tools = client.peer().list_all_tools().await?;
            if !tools.iter().any(|t| t.name == tool_name) {
                continue;
            }

            return client
                .peer()
                .call_tool(CallToolRequestParam {
                    name: tool_name.to_owned().into(),
                    arguments: params.as_object().cloned(),
                })
                .await
                .map_err(Into::into);
        }

        Err(Error::UnknownTool(tool_name.to_string()))
    }

    pub async fn handle_servers(&mut self, configs: &[McpServer]) -> Result<()> {
        let servers_to_stop: Vec<String> = self
            .clients
            .keys()
            .filter(|&name| configs.iter().all(|s| &s.id.to_string() != name))
            .cloned()
            .collect();

        // Stop servers that are no longer needed
        for id in &servers_to_stop {
            trace!(id, "Stopping MCP server.");
            self.clients.remove(id);
        }

        // Determine which servers to start (in configs but not currently active)
        let servers_to_start: Vec<&McpServer> = configs
            .iter()
            .filter(|server| !self.clients.contains_key(&server.id.to_string()))
            .collect();

        // Start new servers
        for server in servers_to_start {
            trace!(id = %server.id, "Starting MCP server.");

            let client = Self::create_client(server).await?;
            self.clients.insert(server.id.to_string(), client);
        }

        Ok(())
    }

    /// Create a new MCP client for a server configuration
    async fn create_client(config: &McpServer) -> Result<RunningService<RoleClient, ()>> {
        match config.transport {
            Transport::Stdio(ref config) => {
                // Build environment variables
                let vars = config
                    .environment_variables
                    .iter()
                    .filter_map(|key| Some((key.to_owned(), env::var(key).ok()?)))
                    .collect::<HashMap<_, _>>();

                // Create command
                let mut cmd = Command::new(&config.command);
                cmd.stderr(Stdio::null());
                cmd.args(&config.args);

                // Add environment variables
                for (key, value) in vars {
                    cmd.env(key, value);
                }

                // Create the child process transport
                let child_process = TokioChildProcess::new(&mut cmd)?;

                // Create a timeout for the connection
                let timeout = Duration::from_secs(60);

                // Serve the client with timeout
                let client = tokio::time::timeout(timeout, async { ().serve(child_process).await })
                    .await??;

                Ok(client)
            }
        }
    }
}
