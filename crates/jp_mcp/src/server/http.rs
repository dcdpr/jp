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
};

use axum::Router;
use jp_tool::Error as ToolError;
use reqwest_mcp::{Client as HttpClient, redirect::Policy};
use rmcp::{
    ErrorData, ServerHandler, ServiceExt as _,
    model::{
        CallToolRequestParams, CallToolResult, ListToolsResult, PaginatedRequestParams,
        ServerCapabilities, ServerInfo, Tool,
    },
    service::{RequestContext, RoleClient, RoleServer, RunningService},
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
    task::{JoinError, JoinHandle},
};
use tokio_util::sync::CancellationToken;

use super::service::{CallRequest, Service, ServiceError};

/// Failure starting, connecting to, or stopping the in-process endpoint.
#[derive(Debug, thiserror::Error)]
pub enum EndpointError {
    /// Listener or HTTP server I/O failed.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// The HTTP task failed.
    #[error(transparent)]
    Task(#[from] JoinError),
    /// Execution service shutdown failed.
    #[error(transparent)]
    Service(#[from] ServiceError),
    /// The MCP handshake failed.
    #[error("Could not connect to JP MCP Server: {0}")]
    Connect(Box<dyn StdError + Send + Sync>),
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
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await?;
        let address = listener.local_addr()?;
        let origin = format!("http://{address}");
        let url = format!("{origin}/mcp");
        let service = Arc::new(service);
        let factory = service.clone();
        let cancellation = CancellationToken::new();
        let mut config = StreamableHttpServerConfig::default();
        config.allowed_hosts = vec![address.to_string()];
        config.allowed_origins = vec![origin];
        config.cancellation_token = cancellation.clone();
        let transport = StreamableHttpService::new(
            move || {
                Ok(Handler {
                    service: factory.clone(),
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
        let client = HttpClient::builder()
            .no_proxy()
            .redirect(Policy::none())
            .build()
            .map_err(|error| EndpointError::Connect(Box::new(error)))?;
        let config = StreamableHttpClientTransportConfig::with_uri(self.url.clone())
            .reinit_on_expired_session(false);
        ().serve(StreamableHttpClientTransport::with_client(client, config))
            .await
            .map_err(|error| EndpointError::Connect(Box::new(error)))
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
        self.service.shutdown().await?;
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
}

impl ServerHandler for Handler {
    fn get_info(&self) -> ServerInfo {
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
                Tool::new(
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
                )
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
