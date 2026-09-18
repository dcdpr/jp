//! Loopback Streamable HTTP transport for the JP MCP Server.
//!
//! [`Endpoint`] owns its listener and execution service.
//! The private Host receiver returned when constructing the service remains
//! with the MCP Host.

use std::{
    error::Error as StdError,
    io,
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
    time::{Duration, Instant},
};

use axum::Router;
use jp_tool::Error as ToolError;
use rmcp::{
    ErrorData, ServerHandler, ServiceExt as _,
    model::{
        CallToolRequestParams, CallToolResult, ListToolsResult, Meta, PaginatedRequestParams,
        ProgressNotificationParam, ProgressToken, ServerCapabilities, ServerInfo, Tool,
    },
    service::{Peer, RequestContext, RoleClient, RoleServer, RunningService},
    transport::{
        StreamableHttpClientTransport,
        streamable_http_client::StreamableHttpClientTransportConfig,
        streamable_http_server::{
            session::local::LocalSessionManager,
            tower::{StreamableHttpServerConfig, StreamableHttpService},
        },
    },
};
use tokio::{
    net::TcpListener,
    sync::broadcast,
    task::{JoinError, JoinHandle},
    time::{MissedTickBehavior, interval},
};
use tokio_util::sync::CancellationToken;
use url::Url;

use super::{
    http_client::LoopbackClient,
    service::{CallRequest, InvocationId, Progress, Service, ServiceError},
};

/// How often a running call tells its caller that it is still alive, when the
/// tool itself has nothing to say.
///
/// A caller waiting on a call it has heard nothing from cannot tell a slow tool
/// from a dead one, and clients commonly abandon such a call after a few
/// minutes.
/// This sits well inside those limits, and inside the idle window the transport
/// applies to a session carrying no traffic.
const PROGRESS_HEARTBEAT: Duration = Duration::from_secs(30);

/// Failure starting, connecting to, or stopping the in-process endpoint.
#[derive(Debug, thiserror::Error)]
pub enum EndpointError {
    /// Listener or HTTP server I/O failed.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// The HTTP task failed.
    #[error(transparent)]
    Task(#[from] JoinError),
    /// The tool catalog the Host supplied cannot be served.
    #[error(transparent)]
    Service(#[from] ServiceError),
    /// The MCP handshake failed.
    #[error("Could not connect to JP MCP Server: {0}")]
    Connect(Box<dyn StdError + Send + Sync>),
}

/// Open an independent MCP session using JP's HTTP transport.
/// Expired sessions fail rather than replaying tool execution automatically.
pub async fn connect(url: &Url) -> Result<RunningService<RoleClient, ()>, EndpointError> {
    // A loopback connection must not be routed through an environment
    // proxy or followed to another host.
    let client = LoopbackClient::new().map_err(|error| EndpointError::Connect(Box::new(error)))?;
    let config = StreamableHttpClientTransportConfig::with_uri(url.to_string())
        .reinit_on_expired_session(false);
    ().serve(StreamableHttpClientTransport::with_client(client, config))
        .await
        .map_err(|error| EndpointError::Connect(Box::new(error)))
}

/// Owns a loopback listener with an OS-assigned port.
pub struct Endpoint {
    url: String,
    service: Arc<Service>,
    cancellation: CancellationToken,
    task: Option<JoinHandle<io::Result<()>>>,
}

impl Endpoint {
    /// Start the endpoint.
    /// Does not consume or drive the private Host receiver.
    pub async fn start(service: Service) -> Result<Self, EndpointError> {
        Self::start_reporting_every(service, PROGRESS_HEARTBEAT).await
    }

    /// Start the endpoint, choosing how often a running call reports liveness.
    ///
    /// Separate from [`start`] so a test can observe repeated reports without
    /// waiting out the interval a real caller is served.
    ///
    /// [`start`]: Self::start
    async fn start_reporting_every(
        service: Service,
        heartbeat: Duration,
    ) -> Result<Self, EndpointError> {
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await?;
        let address = listener.local_addr()?;
        let origin = format!("http://{address}");
        let url = format!("{origin}/mcp");
        let service = Arc::new(service);
        let factory = service.clone();
        let cancellation = CancellationToken::new();
        // Only this listener's own address is an acceptable Host or Origin, so
        // a page in a browser cannot reach the endpoint by resolving some other
        // name to loopback.
        //
        // Assigned field by field because rmcp marks the config
        // `#[non_exhaustive]`, which rules out struct-update syntax downstream.
        let mut config = StreamableHttpServerConfig::default();
        config.allowed_hosts = vec![address.to_string()];
        config.allowed_origins = vec![origin];
        config.cancellation_token = cancellation.clone();
        let transport = StreamableHttpService::new(
            move || {
                Ok(Handler {
                    service: factory.clone(),
                    heartbeat,
                })
            },
            Arc::new(LocalSessionManager::default()),
            config,
        );
        let router = Router::new().nest_service("/mcp", transport);
        let shutdown = cancellation.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await
        });
        Ok(Self {
            url,
            service,
            cancellation,
            task: Some(task),
        })
    }

    /// URL provided to MCP callers; JP's terminal stdio is not used.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Establish the MCP Host's ordinary HTTP connection to this endpoint.
    pub async fn connect(&self) -> Result<RunningService<RoleClient, ()>, EndpointError> {
        let url = self
            .url
            .parse()
            .map_err(|error| EndpointError::Connect(Box::new(error)))?;
        connect(&url).await
    }

    /// Private in-process control for the MCP Host, not exposed through HTTP.
    #[must_use]
    pub fn service(&self) -> Arc<Service> {
        self.service.clone()
    }

    /// Signal cancellation of current calls without closing the endpoint.
    pub fn cancel_current(&self) {
        self.service.cancel_current();
    }

    /// Stop tool work, close upstream services, and join the HTTP listener.
    pub async fn shutdown(mut self) -> Result<(), EndpointError> {
        self.service.shutdown().await;
        self.cancellation.cancel();
        if let Some(task) = self.task.take() {
            task.await??;
        }
        Ok(())
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        self.service.stop();
        self.cancellation.cancel();
    }
}

#[derive(Clone)]
struct Handler {
    service: Arc<Service>,

    /// How often a running call reports liveness while its tool is silent.
    heartbeat: Duration,
}

/// Reports one call's progress for as long as it is held.
///
/// The work runs in its own task so that it continues for the whole call rather
/// than only while the handler happens to be waiting on something.
struct Reporter(JoinHandle<()>);

impl Drop for Reporter {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Report a running call's output and liveness to the caller that asked for it.
///
/// Each line the tool writes to stderr becomes one notification, and a stretch
/// in which it writes nothing produces one every `heartbeat`.
/// The count rises by one per notification and carries no total, which is what
/// MCP asks of work whose size is not known in advance.
///
/// Returns immediately when the caller supplied no progress token, since a
/// notification has nowhere to go without one.
async fn report_progress(
    peer: Peer<RoleServer>,
    id: InvocationId,
    progress: Option<(ProgressToken, broadcast::Receiver<Progress>)>,
    heartbeat: Duration,
) {
    let Some((token, mut lines)) = progress else {
        return;
    };
    let mut ticker = interval(heartbeat);
    // Catching up on the ticks missed during a burst of tool output would spend
    // them all at once, on a call that plainly needs no reminder that it is
    // alive.
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    // The first tick is immediate, and the caller has just been told the call
    // began.
    ticker.tick().await;
    let started = Instant::now();
    let mut count = 0_f64;
    loop {
        let message = tokio::select! {
            received = lines.recv() => match received {
                Ok(progress) if progress.id == id => progress.line,
                // A line from another call in flight, or more lines than this
                // receiver kept up with. Neither says anything about this call,
                // and the receiver stays usable either way.
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => continue,
                // The service owning the sender is gone, so the call cannot
                // still be running.
                Err(broadcast::error::RecvError::Closed) => return,
            },
            _ = ticker.tick() => format!("running for {}s", started.elapsed().as_secs()),
        };
        count += 1.0;
        let param = ProgressNotificationParam::new(token.clone(), count).with_message(message);
        // A caller that stopped listening is no reason to stop the tool.
        if peer.notify_progress(param).await.is_err() {
            return;
        }
    }
}

impl ServerHandler for Handler {
    fn get_info(&self) -> ServerInfo {
        // Assigned field by field because rmcp marks `ServerInfo`
        // `#[non_exhaustive]`, which rules out struct-update syntax downstream.
        let mut info = ServerInfo::default();
        info.server_info.name = "jp".into();
        info.server_info.version = env!("CARGO_PKG_VERSION").into();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info
    }

    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let tools = self
            .service
            .definitions()
            .map(|definition| {
                let mut tool = Tool::new(
                    definition.name.clone(),
                    definition
                        .docs
                        .schema_description()
                        .unwrap_or_default()
                        .to_owned(),
                    Arc::new(
                        definition
                            .parameters
                            .as_object()
                            .cloned()
                            .unwrap_or_default(),
                    ),
                );
                tool.meta = self
                    .service
                    .tool_metadata(&definition.name)
                    .cloned()
                    .map(Meta);
                tool
            })
            .collect();
        Ok(ListToolsResult {
            tools,
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if request.task.is_some() {
            return Err(ErrorData::invalid_params(
                "JP tool calls do not support MCP tasks",
                None,
            ));
        }
        // Subscribed before the call is submitted, because a broadcast receiver
        // is sent only what is broadcast after it subscribes, and submitting the
        // call starts the tool.
        let progress = context
            .meta
            .get_progress_token()
            .map(|token| (token, self.service.subscribe_progress()));
        let call = self
            .service
            .start_call(CallRequest {
                name: request.name.into_owned(),
                arguments: request.arguments.unwrap_or_default(),
                correlation: context.meta.0,
            })
            .map_err(protocol_error)?;
        let cancellation = call.cancellation_token();
        // Explicit MCP cancellation or handler destruction must not orphan the
        // separately owned invocation. A dropped HTTP response stream alone
        // does not destroy a stateful session's request handler.
        let _guard = cancellation.clone().drop_guard();
        // Reporting ends with the call, however the call ends.
        let _reporter = Reporter(tokio::spawn(report_progress(
            context.peer.clone(),
            call.id(),
            progress,
            self.heartbeat,
        )));
        let result = call.finish_mcp();
        tokio::pin!(result);
        tokio::select! {
            biased;
            () = context.ct.cancelled() => { cancellation.cancel(); result.await.map_err(protocol_error) },
            result = &mut result => result.map_err(protocol_error),
        }
    }
}

fn protocol_error(error: ServiceError) -> ErrorData {
    match error {
        ServiceError::Tool(ToolError::NotFound { name }) => {
            ErrorData::invalid_params(format!("Unknown tool: {name}"), None)
        }
        ServiceError::InvalidArgument { path } => {
            ErrorData::invalid_params(format!("Invalid tool argument at `{path}`"), None)
        }
        other => ErrorData::internal_error(other.to_string(), None),
    }
}

#[cfg(test)]
#[path = "http_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "conformance_tests.rs"]
mod conformance_tests;
