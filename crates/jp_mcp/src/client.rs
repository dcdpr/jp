use std::{collections::HashMap, env, process::Stdio, sync::Arc, time::Duration};

use rmcp::{
    model::{
        CallToolRequestParam, CallToolResult, ReadResourceRequestParam, Resource, ResourceContents,
        Tool,
    },
    service::{RoleClient, RunningService, ServiceExt},
    transport::TokioChildProcess,
};
use tokio::{process::Command, sync::Mutex};
use tracing::trace;

use crate::{
    config::{McpServer, McpServerId},
    error::Result,
    server::embedded::EmbeddedServer,
    transport::Transport,
    Error,
};

/// Manages multiple MCP clients and delegates operations to them
#[derive(Default, Clone)]
pub struct Client {
    clients: Arc<Mutex<HashMap<McpServerId, RunningService<RoleClient, ()>>>>,
    embedded_server: Option<Arc<EmbeddedServer>>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("clients", &self.clients.blocking_lock().keys())
            .field("embedded_server", &self.embedded_server)
            .finish()
    }
}

impl Client {
    #[must_use]
    pub fn with_embedded_server(mut self, server: EmbeddedServer) -> Self {
        self.embedded_server = Some(Arc::new(server));
        self
    }

    /// Get all available tools from all connected MCP servers
    pub async fn list_tools(&self) -> Result<Vec<Tool>> {
        let mut tools = vec![];

        if let Some(server) = self.embedded_server.as_ref() {
            tools.extend(server.list_all_tools().await?);
        }

        for client in self.clients.lock().await.values() {
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
        if let Some(server) = self.embedded_server.as_ref() {
            let tools = server.list_all_tools().await?;
            if tools.iter().any(|t| t.name == tool_name) {
                return server
                    .run_tool(CallToolRequestParam {
                        name: tool_name.to_owned().into(),
                        arguments: params.as_object().cloned(),
                    })
                    .await
                    .map_err(Into::into);
            }
        }

        for client in self.clients.lock().await.values() {
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

    /// Get all available resources from a specific MCP server.
    ///
    /// This does not return the contents of the resources, but instead returns
    /// a list of URIs which can be sent to [`Self::get_resource`] to retrieve
    /// the contents.
    pub async fn list_resources(&self, id: &McpServerId) -> Result<Vec<Resource>> {
        let clients = self.clients.lock().await;
        let client = clients.get(id).ok_or(Error::UnknownServer(id.clone()))?;

        Ok(client.peer().list_all_resources().await?)
    }

    /// Get the contents of a resource from a specific MCP server.
    ///
    /// TODO: Make an `mcp_resource` attachment handler, so that you can embed
    /// attachments from MCP servers that support querying for resources
    pub async fn get_resource_contents(
        &self,
        id: &McpServerId,
        uri: impl Into<String>,
    ) -> Result<Vec<ResourceContents>> {
        let clients = self.clients.lock().await;
        let client = clients.get(id).ok_or(Error::UnknownServer(id.clone()))?;

        Ok(client
            .peer()
            .read_resource(ReadResourceRequestParam { uri: uri.into() })
            .await?
            .contents)
    }

    pub async fn handle_servers(&mut self, configs: &[McpServer]) -> Result<()> {
        let mut clients = self.clients.lock().await;
        let servers_to_stop: Vec<_> = clients
            .keys()
            .filter(|&name| configs.iter().all(|s| &s.id != name))
            .cloned()
            .collect();

        // Stop servers that are no longer needed
        for id in &servers_to_stop {
            trace!(id = %id, "Stopping MCP server.");
            clients.remove(id);
        }

        for server in configs {
            // Determine which servers to start (in configs but not currently
            // active)
            if clients.contains_key(&server.id) {
                continue;
            }

            trace!(id = %server.id, "Starting MCP server.");

            let client = Self::create_client(server).await?;
            clients.insert(server.id.clone(), client);
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
