use std::{
    collections::{HashMap, HashSet},
    env,
    path::Path,
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use indexmap::IndexMap;
use jp_config::providers::mcp::{AlgorithmConfig, McpProviderConfig};
use rmcp::{
    model::{
        CallToolRequestParams, CallToolResult, ReadResourceRequestParams, Resource,
        ResourceContents, Tool,
    },
    service::{RoleClient, RunningService, ServiceExt},
    transport::TokioChildProcess,
};
use sha1::{Digest as _, Sha1};
use sha2::Sha256;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{ChildStderr, Command},
    runtime::Handle,
    sync::RwLock,
    task::JoinSet,
};
use tracing::{trace, warn};

use crate::{
    Error,
    error::Result,
    id::{McpServerId, McpToolId},
};

/// Manages multiple MCP clients and delegates operations to them
#[derive(Clone, Default)]
pub struct Client {
    /// All MCP servers known to the client.
    servers: Arc<RwLock<IndexMap<McpServerId, McpProviderConfig>>>,

    /// Running MCP services.
    services: Arc<RwLock<HashMap<McpServerId, RunningService<RoleClient, ()>>>>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("servers", &self.servers)
            .field("services", &self.services.blocking_read().keys())
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
            services: Arc::new(RwLock::new(HashMap::new())),
            servers: Arc::new(RwLock::new(servers)),
        }
    }

    pub async fn get_tool(&self, id: &McpToolId, server_id: Option<&McpServerId>) -> Result<Tool> {
        let servers = self.servers.read().await;
        let server_ids = match server_id {
            Some(server_id) => vec![server_id],
            None => servers.keys().collect(),
        };

        for server_id in server_ids {
            let Some(server) = servers.get(server_id) else {
                continue;
            };

            let tools = match self.services.read().await.get(server_id) {
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
    ) -> Result<McpServerId> {
        let servers = self.servers.read().await;
        for server_id in servers.keys() {
            if let Some(name) = server_name
                && name != server_id
            {
                continue;
            }

            let tools = self.list_tools_by_server_id(server_id).await?;
            if tools.iter().any(|t| t.name == id.as_str()) {
                return Ok(server_id.clone());
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
        let services = self.services.read().await;
        for (server_id, client) in services.iter() {
            if let Some(server) = server_name
                && server_id.as_str() != server
            {
                continue;
            }

            let tools = client.peer().list_all_tools().await?;
            if !tools.iter().any(|t| t.name == tool_name) {
                continue;
            }

            let mut call_params = CallToolRequestParams::new(tool_name.to_owned());
            call_params.arguments = params.as_object().cloned();

            return client
                .peer()
                .call_tool(call_params)
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
        let clients = self.services.read().await;
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
        let clients = self.services.read().await;
        let client = clients.get(id).ok_or(Error::UnknownServer(id.clone()))?;

        Ok(client
            .peer()
            .read_resource(ReadResourceRequestParams::new(uri))
            .await?
            .contents)
    }

    pub async fn run_services(
        &mut self,
        server_ids: HashSet<McpServerId>,
        handle: Handle,
    ) -> Result<JoinSet<Result<()>>> {
        let mut clients = self.services.write().await;
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

        let _guard = handle.enter();
        let mut joins = JoinSet::<Result<()>>::new();
        for server_id in server_ids {
            // Determine which servers to start (in configs but not currently
            // active)
            if clients.contains_key(&server_id) {
                continue;
            }

            trace!(id = %server_id, "Starting MCP server.");

            joins.spawn({
                let servers = self.servers.clone();
                let clients = self.services.clone();
                async move {
                    let servers = servers.read().await;
                    let server = servers
                        .get(&server_id)
                        .ok_or(Error::UnknownServer(server_id.clone()))?;

                    let client = Self::create_client(&server_id, server).await?;
                    clients.write().await.insert(server_id.clone(), client);
                    Ok(())
                }
            });
        }

        Ok(joins)
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
                    .filter_map(|key| {
                        env::var(key)
                            .inspect_err(|error| {
                                warn!(
                                    key,
                                    error = error.to_string(),
                                    server = id.to_string(),
                                    "Failed to read MCP server environment variable"
                                );
                            })
                            .ok()
                            .map(|value| (key.to_owned(), value))
                    })
                    .collect::<HashMap<_, _>>();

                // Create command
                let mut cmd = Command::new(&config.command);
                cmd.args(&config.arguments);

                // Put the MCP server in its own process group so terminal
                // signals (Ctrl+C / SIGINT) don't kill it. JP manages the
                // server lifecycle through the MCP protocol and
                // kill-on-drop, not through Unix signals.
                #[cfg(unix)]
                cmd.process_group(0);

                // Add environment variables
                for (key, value) in vars {
                    cmd.env(key, value);
                }

                // Create the child process transport. Stderr is piped so we
                // can forward it to tracing; dropping it would close the pipe
                // and the child would see EPIPE on writes.
                let cmd_name = cmd.as_std().get_program().to_string_lossy().to_string();
                let (child_process, stderr) = TokioChildProcess::builder(cmd)
                    .stderr(Stdio::piped())
                    .spawn()
                    .map_err(|error| Error::CannotSpawnProcess {
                        cmd: cmd_name.clone(),
                        error,
                    })?;

                if let Some(stderr) = stderr {
                    spawn_stderr_forwarder(stderr, id.clone());
                }

                // Create a timeout for the connection
                let timeout = Duration::from_mins(1);

                // Serve the client with timeout
                let client = tokio::time::timeout(timeout, async { ().serve(child_process).await })
                    .await?
                    .map_err(|error| Error::InitializeError {
                        cmd: cmd_name,
                        error: error.to_string(),
                    })?;

                Ok(client)
            }
        }
    }

    /// List tools available on a specific server.
    async fn list_tools_by_server_id(&self, server_id: &McpServerId) -> Result<Vec<Tool>> {
        let servers = self.servers.read().await;
        let Some(server) = servers.get(server_id) else {
            return Err(Error::UnknownServer(server_id.clone()));
        };

        Ok(match self.services.read().await.get(server_id) {
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

/// Spawn a background task that forwards an MCP server's stderr to tracing.
///
/// Each line is emitted under `target: "mcp::stderr"` tagged with the server
/// id, so users can opt in via e.g. `RUST_LOG=mcp::stderr=trace`. Uses
/// byte-level line reading so non-UTF-8 output doesn't terminate the
/// forwarder. The task exits when the pipe closes (child exit).
fn spawn_stderr_forwarder(stderr: ChildStderr, server: McpServerId) {
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr);
        let mut line = Vec::new();

        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line).await {
                Ok(0) => break,
                Ok(_) => {
                    let text = String::from_utf8_lossy(&line);
                    let trimmed = text.trim_end_matches(['\n', '\r']);
                    if !trimmed.is_empty() {
                        trace!(
                            target: "mcp::stderr",
                            server = %server,
                            "{trimmed}"
                        );
                    }
                }
                Err(error) => {
                    warn!(
                        server = %server,
                        error = %error,
                        "Error reading MCP server stderr"
                    );
                    break;
                }
            }
        }
    });
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
        got: digest,
    })
}
