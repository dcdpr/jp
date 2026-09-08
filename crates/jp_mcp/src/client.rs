use std::{
    collections::{HashMap, HashSet, VecDeque},
    env,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
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
    sync::{RwLock, broadcast},
    task::JoinSet,
};
use tracing::{trace, warn};

use crate::{
    Error,
    error::Result,
    id::{McpServerId, McpToolId},
};

/// One line a starting MCP server wrote to stderr, tagged with its server.
pub type StderrLine = (McpServerId, String);

/// A batch of MCP servers starting in the background.
///
/// Returned by [`Client::run_services`].
/// Await the tasks in [`joins`] to learn when each server finishes starting;
/// [`pending`] names the servers the batch is starting, so callers can report
/// which ones are still booting.
///
/// [`joins`]: Self::joins
/// [`pending`]: Self::pending
pub struct StartupSet {
    /// One task per starting server.
    /// Each resolves to how that server's startup finished, or to the startup
    /// error for a required server.
    pub joins: JoinSet<Result<Startup>>,

    /// Ids of the servers being started, sorted by name.
    pub pending: Vec<McpServerId>,

    /// Stderr lines from the starting servers, tagged with their source.
    ///
    /// A server can spend minutes building before anyone drains this, so
    /// whatever is queued when a reader arrives is the backlog.
    /// The queue is bounded and drops its oldest entries rather than making a
    /// child wait on a reader: a display shows the most recent lines by
    /// definition, and a forwarder parked on a send would let the child block
    /// on a full pipe.
    pub stderr: broadcast::Receiver<StderrLine>,
}

/// How one server's startup finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Startup {
    /// The server is running.
    Ready(McpServerId),

    /// The server is marked optional, failed to start, and was skipped.
    ///
    /// The failure is logged under the `mcp` target.
    /// A caller that wants to tell the user which tools went away with it needs
    /// to know it happened at all, which is why this is distinct from
    /// [`Self::Ready`] rather than folded into it.
    Skipped(McpServerId),
}

impl Startup {
    /// The server this outcome is about.
    #[must_use]
    pub const fn id(&self) -> &McpServerId {
        match self {
            Self::Ready(id) | Self::Skipped(id) => id,
        }
    }

    /// Whether the server was skipped rather than started.
    #[must_use]
    pub const fn was_skipped(&self) -> bool {
        matches!(self, Self::Skipped(_))
    }
}

/// Outcome of attempting to start an MCP server.
enum SpawnOutcome {
    /// Server started successfully.
    Started(RunningService<RoleClient, ()>),

    /// Server is marked optional and failed to start.
    /// Already logged.
    OptionalFailed,
}

/// Manages multiple MCP clients and delegates operations to them
#[derive(Clone, Default)]
pub struct Client {
    /// All MCP servers known to the client.
    servers: Arc<RwLock<IndexMap<McpServerId, McpProviderConfig>>>,

    /// Running MCP services.
    services: Arc<RwLock<HashMap<McpServerId, RunningService<RoleClient, ()>>>>,

    /// Working directory for spawned stdio servers.
    ///
    /// `None` inherits the process cwd.
    child_cwd: Option<PathBuf>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("servers", &self.servers)
            .field("services", &self.services.blocking_read().keys())
            .field("child_cwd", &self.child_cwd)
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
            child_cwd: None,
        }
    }

    /// Set the working directory spawned stdio servers inherit.
    ///
    /// `None` (the default) inherits the JP process cwd.
    /// The bootstrap sets this to the selected workspace root for from-anywhere
    /// runs (RFD 087).
    #[must_use]
    pub fn with_child_cwd(mut self, cwd: Option<PathBuf>) -> Self {
        self.child_cwd = cwd;
        self
    }

    /// Look up a tool definition on a specific MCP server.
    ///
    /// The server must be configured (i.e. present in the [`Client`]'s server
    /// map).
    /// If the server isn't currently running, it is started on demand and
    /// cached — same fail-soft policy as the rest of the client for `optional`
    /// servers.
    pub async fn get_tool(&self, id: &McpToolId, server_id: &McpServerId) -> Result<Tool> {
        let servers = self.servers.read().await;
        let server = servers
            .get(server_id)
            .ok_or(Error::UnknownServer(server_id.clone()))?;

        let running = self.services.read().await;
        let tools = if let Some(client) = running.get(server_id) {
            client.peer().list_all_tools().await?
        } else {
            drop(running);
            match Self::try_create_client(server_id, server, self.child_cwd.as_deref(), None)
                .await?
            {
                SpawnOutcome::Started(client) => client.list_all_tools().await?,
                SpawnOutcome::OptionalFailed => return Err(Error::UnknownTool(id.to_string())),
            }
        };

        tools
            .into_iter()
            .find(|t| t.name == id.as_str())
            .ok_or_else(|| Error::UnknownTool(id.to_string()))
    }

    /// Call a tool by name on a specific MCP server.
    pub async fn call_tool(
        &self,
        tool_name: &str,
        server_name: &str,
        params: &serde_json::Value,
    ) -> Result<CallToolResult> {
        let server_id = McpServerId::new(server_name);
        let services = self.services.read().await;
        let client = services
            .get(&server_id)
            .ok_or(Error::UnknownServer(server_id.clone()))?;

        let mut call_params = CallToolRequestParams::new(tool_name.to_owned());
        call_params.arguments = params.as_object().cloned();

        client
            .peer()
            .call_tool(call_params)
            .await
            .map_err(Into::into)
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
    ) -> Result<StartupSet> {
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
        let (stderr_tx, stderr_rx) = broadcast::channel(STDERR_CHANNEL_LINES);
        let mut joins = JoinSet::<Result<Startup>>::new();
        let mut pending = Vec::new();
        for server_id in server_ids {
            // Determine which servers to start (in configs but not currently
            // active)
            if clients.contains_key(&server_id) {
                continue;
            }

            trace!(id = %server_id, "Starting MCP server.");
            pending.push(server_id.clone());

            joins.spawn({
                let servers = self.servers.clone();
                let clients = self.services.clone();
                let child_cwd = self.child_cwd.clone();
                let stderr_tx = stderr_tx.clone();
                async move {
                    let servers = servers.read().await;
                    let server = servers
                        .get(&server_id)
                        .ok_or(Error::UnknownServer(server_id.clone()))?;

                    let outcome = Self::try_create_client(
                        &server_id,
                        server,
                        child_cwd.as_deref(),
                        Some(&stderr_tx),
                    )
                    .await?;

                    match outcome {
                        SpawnOutcome::Started(client) => {
                            clients.write().await.insert(server_id.clone(), client);
                            Ok(Startup::Ready(server_id))
                        }
                        SpawnOutcome::OptionalFailed => Ok(Startup::Skipped(server_id)),
                    }
                }
            });
        }

        // `server_ids` is a set, so spawn order is nondeterministic; sort for
        // a stable presentation order.
        pending.sort_by(|a, b| a.as_str().cmp(b.as_str()));

        Ok(StartupSet {
            joins,
            pending,
            stderr: stderr_rx,
        })
    }

    /// Check whether a server has an active running service.
    ///
    /// Returns `false` for servers that aren't configured at all, that haven't
    /// been started yet, or that failed to start while marked `optional`.
    /// The tool-resolution pipeline uses this to filter out MCP tools whose
    /// backing server is unavailable before they reach the LLM.
    pub async fn is_running(&self, id: &McpServerId) -> bool {
        self.services.read().await.contains_key(id)
    }

    /// Attempt to create an MCP client for a server configuration, honoring the
    /// `optional` flag.
    ///
    /// For required servers (the default), any failure is returned as `Err` and
    /// propagates up to abort the operation.
    /// For optional servers, the failure is logged at `warn` and the helper
    /// returns [`SpawnOutcome::OptionalFailed`] so the caller can skip the
    /// server.
    async fn try_create_client(
        id: &McpServerId,
        config: &McpProviderConfig,
        child_cwd: Option<&Path>,
        stderr_lines: Option<&broadcast::Sender<StderrLine>>,
    ) -> Result<SpawnOutcome> {
        match Self::create_client(id, config, child_cwd, stderr_lines).await {
            Ok(client) => Ok(SpawnOutcome::Started(client)),
            Err(error) if config.optional() => {
                warn!(
                    server = %id,
                    %error,
                    "Optional MCP server failed to start; tools that depend on \
                     this server will be unavailable for this session."
                );
                Ok(SpawnOutcome::OptionalFailed)
            }
            Err(error) => Err(error),
        }
    }

    /// Create a new MCP client for a server configuration
    async fn create_client(
        id: &McpServerId,
        config: &McpProviderConfig,
        child_cwd: Option<&Path>,
        stderr_lines: Option<&broadcast::Sender<StderrLine>>,
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

                // The root-as-working-directory invariant (RFD 087): when JP
                // operates on a workspace other than the launch cwd's own,
                // servers run as if launched from the selected workspace root.
                if let Some(cwd) = child_cwd {
                    cmd.current_dir(cwd);
                }

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

                // Build a human-readable command line (program + args) so
                // diagnostic errors include enough context to reproduce the
                // failure.
                let cmd_display = render_command(&cmd);

                // Create the child process transport. Stderr is piped so we
                // can forward it to tracing; dropping it would close the pipe
                // and the child would see EPIPE on writes.
                let (child_process, stderr) = TokioChildProcess::builder(cmd)
                    .stderr(Stdio::piped())
                    .spawn()
                    .map_err(|error| Error::CannotSpawnProcess {
                        cmd: cmd_display.clone(),
                        error,
                    })?;

                // Capture stderr into a bounded ring buffer in addition to
                // forwarding it to tracing, so initialization failures can
                // surface the underlying error (e.g. a build script's output)
                // even when the user hasn't enabled `mcp::stderr` logging.
                let stderr_tail = Arc::new(Mutex::new(VecDeque::<String>::with_capacity(
                    STDERR_TAIL_LINES,
                )));
                if let Some(stderr) = stderr {
                    spawn_stderr_forwarder(
                        stderr,
                        id.clone(),
                        Arc::clone(&stderr_tail),
                        stderr_lines.cloned(),
                    );
                }

                // Give the server time to start and answer the MCP
                // `initialize` handshake. Configurable per server because a
                // server that builds from source on spawn can legitimately
                // take minutes; `0` disables the timeout entirely.
                let serve = async { ().serve(child_process).await };

                // An initialization failure attaches the captured stderr
                // tail: it usually names the actual problem (build output,
                // missing dependency, lock contention).
                let init_error =
                    |error: rmcp::service::ClientInitializeError| Error::InitializeError {
                        cmd: cmd_display.clone(),
                        error: error.to_string(),
                        stderr: render_stderr_tail(&stderr_tail),
                    };

                let client = if config.startup_timeout_secs == 0 {
                    serve.await.map_err(init_error)?
                } else {
                    let timeout = Duration::from_secs(config.startup_timeout_secs.into());
                    match tokio::time::timeout(timeout, serve).await {
                        Ok(result) => result.map_err(init_error)?,
                        Err(_) => {
                            return Err(Error::InitializeTimeout {
                                cmd: cmd_display,
                                timeout_secs: config.startup_timeout_secs,
                                stderr: render_stderr_tail(&stderr_tail),
                            });
                        }
                    }
                };

                Ok(client)
            }
        }
    }
}

/// Maximum number of stderr lines retained for diagnostic error reporting.
const STDERR_TAIL_LINES: usize = 100;

/// Capacity of the tagged stderr channel a caller can read startup output from.
///
/// Comfortably more than any display can show, so a reader that arrives late
/// still finds recent output, while a build that emits thousands of lines
/// before anyone reads costs a bounded amount of memory.
const STDERR_CHANNEL_LINES: usize = 256;

/// Render a command (program + arguments) as a single human-readable line.
fn render_command(cmd: &Command) -> String {
    let std_cmd = cmd.as_std();
    let prog = std_cmd.get_program().to_string_lossy();
    let args = std_cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    if args.is_empty() {
        prog.into_owned()
    } else {
        format!("{prog} {}", args.join(" "))
    }
}

/// Render the captured stderr tail for inclusion in an `InitializeError`.
///
/// Returns an empty string if the buffer is empty, otherwise a block prefixed
/// with a leading newline and `stderr:` header, with each line indented by two
/// spaces.
fn render_stderr_tail(buffer: &Mutex<VecDeque<String>>) -> String {
    let snapshot: Vec<String> = match buffer.lock() {
        Ok(buf) => buf.iter().cloned().collect(),
        Err(poisoned) => poisoned.into_inner().iter().cloned().collect(),
    };

    if snapshot.is_empty() {
        return String::new();
    }

    let body = snapshot
        .into_iter()
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>()
        .join("\n");

    format!("\nstderr:\n{body}")
}

/// Spawn a background task that forwards an MCP server's stderr to tracing and
/// a bounded ring buffer.
///
/// Each line is emitted under `target: "mcp::stderr"` tagged with the server
/// id, so users can opt in via e.g. `RUST_LOG=mcp::stderr=trace`.
/// Uses byte-level line reading so non-UTF-8 output doesn't terminate the
/// forwarder.
/// The task exits when the pipe closes (child exit).
///
/// The same lines are appended to `tail` (capped at [`STDERR_TAIL_LINES`]) so
/// initialization failures can attach the recent stderr output to the resulting
/// error without requiring the user to enable trace logging.
///
/// When `lines` is set, each line is also published on it tagged with `server`,
/// for a caller that wants to show startup output while it waits.
/// The send never blocks and its failures are ignored: reading has to keep up
/// with the child regardless of whether anyone is listening, or the child
/// stalls on a full pipe and the handshake never completes.
fn spawn_stderr_forwarder(
    stderr: ChildStderr,
    server: McpServerId,
    tail: Arc<Mutex<VecDeque<String>>>,
    lines: Option<broadcast::Sender<StderrLine>>,
) {
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
                    if trimmed.is_empty() {
                        continue;
                    }

                    trace!(
                        target: "mcp::stderr",
                        server = %server,
                        "{trimmed}"
                    );

                    if let Ok(mut buf) = tail.lock() {
                        if buf.len() >= STDERR_TAIL_LINES {
                            buf.pop_front();
                        }
                        buf.push_back(trimmed.to_owned());
                    }

                    if let Some(lines) = &lines {
                        drop(lines.send((server.clone(), trimmed.to_owned())));
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

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;

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
