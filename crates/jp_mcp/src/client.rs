use std::{collections::HashMap, env, path::Path, process::Stdio, sync::Arc, time::Duration};

use hex::ToHex as _;
use indexmap::IndexMap;
use jp_config::providers::mcp::{AlgorithmConfig, McpProviderConfig};
use rmcp::{
    model::{
        CallToolRequestParam, CallToolResult, ReadResourceRequestParam, Resource, ResourceContents,
        Tool,
    },
    service::{RoleClient, RunningService, ServiceExt},
    transport::TokioChildProcess,
};
use sha1::{Digest as _, Sha1};
use sha2::Sha256;
use tokio::{process::Command, sync::Mutex};
use tracing::trace;

use crate::{
    error::Result,
    id::{McpServerId, McpToolId},
    Error,
};

/// Manages multiple MCP clients and delegates operations to them
#[derive(Clone)]
pub struct Client {
    /// All MCP servers known to the client.
    servers: IndexMap<McpServerId, McpProviderConfig>,

    /// Running MCP services.
    services: Arc<Mutex<HashMap<McpServerId, RunningService<RoleClient, ()>>>>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("servers", &self.servers)
            .field("services", &self.services.blocking_lock().keys())
            .finish()
    }
}

impl Client {
    /// Create a new MCP client.
    #[must_use]
    pub fn new(providers: IndexMap<String, McpProviderConfig>) -> Self {
        let servers = providers
            .into_iter()
            .map(|(name, config)| (McpServerId::new(name), config))
            .collect();

        Self {
            services: Arc::new(Mutex::new(HashMap::new())),
            servers,
        }
    }

    pub async fn get_tool(&self, id: &McpToolId, server_id: Option<&McpServerId>) -> Result<Tool> {
        let server_ids = match server_id {
            Some(server_id) => vec![server_id],
            None => self.servers.keys().collect(),
        };

        for server_id in server_ids {
            let Some(server) = self.servers.get(server_id) else {
                continue;
            };

            let tools = match self.services.lock().await.get(server_id) {
                Some(client) => client.peer().list_all_tools().await?,
                None => {
                    Self::create_client(server_id, server)
                        .await?
                        .list_all_tools()
                        .await?
                }
            };

            if let Some(tool) = tools.iter().find(|t| t.name == id.as_str()) {
                return Ok(tool.clone());
            }
        }

        Err(Error::UnknownTool(id.to_string()))
    }

    /// Get the server ID of the given tool ID.
    pub async fn get_tool_server_id(
        &self,
        id: &McpToolId,
        server_name: Option<&McpServerId>,
    ) -> Result<&McpServerId> {
        for server_id in self.servers.keys() {
            if let Some(name) = server_name
                && name != server_id
            {
                continue;
            }

            let tools = self.list_tools_by_server_id(server_id).await?;
            if tools.iter().any(|t| t.name == id.as_str()) {
                return Ok(server_id);
            }
        }

        Err(Error::UnknownTool(id.to_string()))
    }

    /// Call a tool by name with given parameters.
    pub async fn call_tool(
        &self,
        tool_name: &str,
        server_name: Option<&str>,
        params: &serde_json::Value,
    ) -> Result<CallToolResult> {
        for (server_id, client) in self.services.lock().await.iter() {
            if let Some(server) = server_name
                && server_id.as_str() != server
            {
                continue;
            }

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
    /// a list of URIs which can be sent to [`Self::get_resource_contents`] to
    /// retrieve the contents.
    pub async fn list_resources(&self, id: &McpServerId) -> Result<Vec<Resource>> {
        let clients = self.services.lock().await;
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
        let clients = self.services.lock().await;
        let client = clients.get(id).ok_or(Error::UnknownServer(id.clone()))?;

        Ok(client
            .peer()
            .read_resource(ReadResourceRequestParam { uri: uri.into() })
            .await?
            .contents)
    }

    pub async fn run_services(&mut self, server_ids: &[McpServerId]) -> Result<()> {
        let mut clients = self.services.lock().await;
        let servers_to_stop: Vec<_> = clients
            .keys()
            .filter(|&name| server_ids.iter().all(|s| s != name))
            .cloned()
            .collect();

        // Stop servers that are no longer needed
        for server_id in &servers_to_stop {
            trace!(id = %server_id, "Stopping MCP server.");
            clients.remove(server_id);
        }

        for server_id in server_ids {
            // Determine which servers to start (in configs but not currently
            // active)
            if clients.contains_key(server_id) {
                continue;
            }

            trace!(id = %server_id, "Starting MCP server.");

            let server = self
                .servers
                .get(server_id)
                .ok_or(Error::UnknownServer(server_id.clone()))?;

            let client = Self::create_client(server_id, server).await?;
            clients.insert(server_id.clone(), client);
        }

        Ok(())
    }

    /// Create a new MCP client for a server configuration
    async fn create_client(
        id: &McpServerId,
        config: &McpProviderConfig,
    ) -> Result<RunningService<RoleClient, ()>> {
        match config {
            McpProviderConfig::Stdio(config) => {
                if let Some(checksum) = &config.checksum {
                    verify_file_checksum(
                        id.as_str(),
                        &config.command,
                        &checksum.value,
                        checksum.algorithm,
                    )?;
                }

                // Build environment variables
                let vars = config
                    .variables
                    .iter()
                    .map(|key| Ok((key.to_owned(), env::var(key)?)))
                    .collect::<Result<HashMap<_, _>>>()?;

                // Create command
                let mut cmd = Command::new(&config.command);
                cmd.stderr(Stdio::null());
                cmd.args(&config.arguments);

                // Add environment variables
                for (key, value) in vars {
                    cmd.env(key, value);
                }

                // Create the child process transport
                let child_process = TokioChildProcess::new(&mut cmd).map_err(|error| {
                    Error::CannotSpawnProcess {
                        cmd: cmd.as_std().get_program().to_string_lossy().to_string(),
                        error,
                    }
                })?;

                // Create a timeout for the connection
                let timeout = Duration::from_secs(60);

                // Serve the client with timeout
                let client = tokio::time::timeout(timeout, async { ().serve(child_process).await })
                    .await?
                    .map_err(|error| Error::ProcessError {
                        cmd: cmd.as_std().get_program().to_string_lossy().to_string(),
                        error,
                    })?;

                Ok(client)
            }
        }
    }

    /// List tools available on a specific server.
    async fn list_tools_by_server_id(&self, server_id: &McpServerId) -> Result<Vec<Tool>> {
        let Some(server) = self.servers.get(server_id) else {
            return Err(Error::UnknownServer(server_id.clone()));
        };

        Ok(match self.services.lock().await.get(server_id) {
            Some(client) => client.peer().list_all_tools().await?,
            None => {
                Self::create_client(server_id, server)
                    .await?
                    .list_all_tools()
                    .await?
            }
        })
    }
}

pub fn verify_file_checksum(
    server: &str,
    command: &Path,
    hash: &str,
    algo: AlgorithmConfig,
) -> Result<()> {
    let path = which::which(command).map_err(|error| Error::CannotLocateBinary {
        path: command.to_path_buf(),
        error: Box::new(error),
    })?;

    let contents = std::fs::read(&path).map_err(|error| Error::CannotReadFile {
        path: path.clone(),
        error: Box::new(error),
    })?;

    let digest = match algo {
        AlgorithmConfig::Sha256 => format!("{:x}", Sha256::digest(&contents)),
        AlgorithmConfig::Sha1 => format!("{:x}", Sha1::digest(&contents)),
    };

    if digest.eq_ignore_ascii_case(hash) {
        return Ok(());
    }

    Err(Error::ChecksumMismatch {
        server: server.to_string(),
        path,
        expected: hash.to_string(),
        got: digest.encode_hex(),
    })
}
