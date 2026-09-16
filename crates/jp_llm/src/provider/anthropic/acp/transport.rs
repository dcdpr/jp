//! One isolated ACP connection per provider request.

use std::{
    collections::BTreeMap,
    env, fs,
    io::{self, Write as _},
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use camino::{Utf8Path, Utf8PathBuf};
use chrono::Utc;
use futures::{StreamExt as _, future::BoxFuture};
use jp_config::assistant::{request::CachePolicy, tool_choice::ToolChoice};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tokio::sync::mpsc;
use tracing::{debug, instrument::WithSubscriber as _, warn};
use uuid::Uuid;

use super::{
    Error, cassette, options, process,
    protocol::{AuthUpdate, SdkNotification, State},
    rpc::{Handler, Inbound, Peer, Request, RpcError, Tap},
    schema::{
        ClientCapabilities, InitializeRequest, InitializeResponse, LoadSessionRequest,
        LoadSessionResponse, NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse,
        ProtocolVersion, RequestPermissionRequest, SessionConfigKind, SessionNotification,
        SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, agent_method, client_method,
    },
    transcript::PreparedRequest,
};
use crate::{
    error::StreamError,
    event::Event,
    model::ModelDetails,
    query::{ChatQuery, QueryContext},
    stream::EventStream,
};

/// Launch work independently of stream polling so MCP tools can complete while
/// the Host is processing their events.
/// Dropping the stream closes the channel and cancels the connection, including
/// the SDK-owned process group.
pub(crate) fn stream(
    model: &ModelDetails,
    query: ChatQuery,
    context: QueryContext,
) -> crate::error::Result<EventStream> {
    let cache = query.thread.events.config()?.assistant.request.cache;
    let tools = if query.tool_choice == ToolChoice::None {
        vec![]
    } else {
        query
            .tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<Vec<_>>()
    };
    if !tools.is_empty() && context.mcp_endpoint.is_none() {
        return Err(Error::ToolHostRequired.into());
    }
    let prepared = PreparedRequest::new(model, query)?;
    Ok(async_stream::stream! {
    let (sender, mut receiver) = mpsc::channel(32);
    tokio::spawn(async move {
        let result = tokio::select! {
            biased;
            () = sender.closed() => return,
            result = run(prepared, context, tools, cache, sender.clone()) => result,
        };
        if let Err(error) = result {
            let error = match error {
                Error::Stream(error) => *error,
                error => StreamError::other(error.to_string()).with_source(error),
            };
            drop(sender.send(Err(error)).await);
        }
    }.with_current_subscriber());
    while let Some(event) = receiver.recv().await { yield event; }
    }
    .boxed())
}

// The requests JP issues, paired with the answers the adapter returns.
impl Request for InitializeRequest {
    const METHOD: &'static str = agent_method::INITIALIZE;

    type Response = InitializeResponse;
}

impl Request for NewSessionRequest {
    const METHOD: &'static str = agent_method::SESSION_NEW;

    type Response = NewSessionResponse;
}

impl Request for LoadSessionRequest {
    const METHOD: &'static str = agent_method::SESSION_LOAD;

    type Response = LoadSessionResponse;
}

impl Request for SetSessionConfigOptionRequest {
    const METHOD: &'static str = agent_method::SESSION_SET_CONFIG_OPTION;

    type Response = SetSessionConfigOptionResponse;
}

impl Request for PromptRequest {
    const METHOD: &'static str = agent_method::SESSION_PROMPT;

    type Response = PromptResponse;
}

/// Establishes one connection and runs it until the request sequence finishes.
///
/// [`Spawned`] is what production uses.
/// A test supplies `cassette::Recorded`, which answers from a recording over an
/// in-memory pipe, so the same handler and foreground run either way.
///
/// Consumed by connecting, since one of these describes one connection.
pub(super) trait Transport: Send {
    fn connect(
        self: Box<Self>,
        handler: Handler,
        foreground: Foreground,
    ) -> BoxFuture<'static, Result<(), RpcError>>;
}

/// Any closure with the right shape, so a test can script one inline rather
/// than declare a type for it.
impl<F> Transport for F
where
    F: FnOnce(Handler, Foreground) -> BoxFuture<'static, Result<(), RpcError>> + Send,
{
    fn connect(
        self: Box<Self>,
        handler: Handler,
        foreground: Foreground,
    ) -> BoxFuture<'static, Result<(), RpcError>> {
        (*self)(handler, foreground)
    }
}

/// The request sequence JP drives once the connection is up.
pub(super) type Foreground =
    Box<dyn FnOnce(Peer) -> BoxFuture<'static, Result<(), RpcError>> + Send>;

/// Spawn the adapter and speak ACP over its stdio.
///
/// `tap` observes the conversation; [`cassette::tap`] supplies one that records
/// under `RECORD`, and an inert one otherwise.
pub(super) struct Spawned {
    pub(super) command: tokio::process::Command,
    pub(super) tap: Tap,
}

impl Transport for Spawned {
    fn connect(
        self: Box<Self>,
        handler: Handler,
        foreground: Foreground,
    ) -> BoxFuture<'static, Result<(), RpcError>> {
        Box::pin(process::run(self.command, self.tap, handler, foreground))
    }
}

fn decode<T: DeserializeOwned>(params: Value) -> Result<T, RpcError> {
    serde_json::from_value(params).map_err(RpcError::into_internal_error)
}

async fn emit(
    sender: &mpsc::Sender<Result<Event, StreamError>>,
    events: Vec<Event>,
) -> Result<(), RpcError> {
    for event in events {
        sender
            .send(Ok(event))
            .await
            .map_err(|_| RpcError::internal_error().data("JP event receiver closed"))?;
    }
    Ok(())
}

fn record_failure(state: &Mutex<State>, error: StreamError) -> RpcError {
    let response = RpcError::into_internal_error(&error);
    state
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .failure
        .get_or_insert(error);
    response
}

/// What a connection needs on disk and in the child's environment before it can
/// reach the adapter.
pub(super) struct Launch {
    /// The derived transcript the adapter resumes from, removed when dropped.
    pub(super) artifact: NativeArtifact,

    /// The environment the adapter is given, and that the session options are
    /// derived from.
    pub(super) environment: BTreeMap<String, String>,

    /// The adapter, ready to spawn.
    pub(super) command: tokio::process::Command,
}

/// Prepare one connection: write its transcript, build its environment, and
/// construct the command that would spawn the adapter.
///
/// Reads `HOME` and `CLAUDE_CONFIG_DIR`, and writes into the directory they
/// name, so a caller that has no adapter to run should build its own pieces
/// rather than call this.
pub(super) fn launch(
    prepared: &PreparedRequest,
    context: &QueryContext,
    cache: CachePolicy,
) -> Result<Launch, Error> {
    let directory = native_directory()?;
    let project = project_name(context);
    let artifact = NativeArtifact::write(prepared, &context.root, &directory, &project)?;
    let mut environment = options::environment(prepared, cache);
    configure_storage_environment(
        &mut environment,
        env::var("CLAUDE_CONFIG_DIR").ok().as_deref(),
        &project,
    );
    let mut command = process::command();
    command.envs(&environment);

    Ok(Launch {
        artifact,
        environment,
        command,
    })
}

async fn run(
    prepared: PreparedRequest,
    context: QueryContext,
    tools: Vec<String>,
    cache: CachePolicy,
    sender: mpsc::Sender<Result<Event, StreamError>>,
) -> Result<(), Error> {
    let Launch {
        artifact,
        environment,
        command,
    } = launch(&prepared, &context, cache)?;

    drive(
        prepared,
        context,
        tools,
        environment,
        artifact,
        Box::new(Spawned {
            command,
            tap: cassette::tap("live"),
        }),
        sender,
    )
    .await
}

/// Route everything the adapter initiates into the shared translation state.
///
/// An error here ends the connection, which is how a revoked subscription stops
/// a prompt that is already streaming.
fn inbound(state: Arc<Mutex<State>>, sender: mpsc::Sender<Result<Event, StreamError>>) -> Handler {
    Box::new(move |message| {
        let state = state.clone();
        let sender = sender.clone();
        Box::pin(async move {
            let (Inbound::Notification { method, params } | Inbound::Request { method, params }) =
                message;

            if method == AuthUpdate::METHOD {
                let notification: AuthUpdate = decode(params)?;
                let mut locked = state.lock().unwrap_or_else(PoisonError::into_inner);
                locked.authenticated = notification.auth_status.is_subscription();
                if locked.live && !locked.authenticated {
                    return Err(RpcError::into_internal_error(Error::SubscriptionRequired));
                }
                return Ok(Value::Null);
            }

            if method == SdkNotification::METHOD {
                let notification: SdkNotification = decode(params)?;
                let (active, result) = {
                    let mut locked = state.lock().unwrap_or_else(PoisonError::into_inner);
                    let active =
                        locked.live && locked.session.as_ref() == Some(&notification.session_id);
                    (active, locked.sdk(notification))
                };
                let mut events = result.map_err(|error| record_failure(&state, error))?;
                // Non-rendered SDK updates still prove the connection is active.
                if active && events.is_empty() {
                    events.push(Event::KeepAlive);
                }
                emit(&sender, events).await?;
                return Ok(Value::Null);
            }

            if method == client_method::SESSION_UPDATE {
                let notification: SessionNotification = decode(params)?;
                let active = {
                    let mut locked = state.lock().unwrap_or_else(PoisonError::into_inner);
                    let active =
                        locked.live && locked.session.as_ref() == Some(&notification.session_id);
                    locked.observe(notification);
                    active
                };
                if active {
                    emit(&sender, vec![Event::KeepAlive]).await?;
                }
                return Ok(Value::Null);
            }

            if method == client_method::SESSION_REQUEST_PERMISSION {
                let request: RequestPermissionRequest = decode(params)?;
                let (response, events) = state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .permission(request)
                    .map_err(RpcError::into_internal_error)?;
                emit(&sender, events).await?;
                return serde_json::to_value(response).map_err(RpcError::into_internal_error);
            }

            // The adapter reports more than JP reads, and answering an unknown
            // request with null is friendlier than failing the connection.
            debug!(%method, "Ignoring unhandled ACP message");
            Ok(Value::Null)
        })
    })
}

#[expect(
    clippy::too_many_lines,
    reason = "The session setup is one ordered sequence; splitting it hides the order"
)]
pub(super) async fn drive(
    prepared: PreparedRequest,
    context: QueryContext,
    tools: Vec<String>,
    environment: BTreeMap<String, String>,
    artifact: NativeArtifact,
    transport: Box<dyn Transport>,
    sender: mpsc::Sender<Result<Event, StreamError>>,
) -> Result<(), Error> {
    let state = Arc::new(Mutex::new(State::new(
        prepared.model.clone(),
        tools.iter().cloned(),
        prepared.schema.is_some(),
    )));
    let handler = inbound(state.clone(), sender.clone());
    let foreground_state = state.clone();
    let heartbeat = sender.clone();
    let foreground: Foreground = Box::new(move |peer| {
        Box::pin(async move {
            let init = peer
                .request(InitializeRequest {
                    protocol_version: ProtocolVersion::V1,
                    client_capabilities: ClientCapabilities::default(),
                })
                .await?;
            if init.protocol_version != ProtocolVersion::V1
                || (!tools.is_empty() && !init.agent_capabilities.mcp_capabilities.http)
            {
                return Err(RpcError::into_internal_error(
                    Error::InitializationCapabilities,
                ));
            }
            let servers = if tools.is_empty() {
                vec![]
            } else {
                vec![
                    json!({"type":"http","name":"jp","url":context.mcp_endpoint.as_ref().expect("tool host validated").as_str(),"headers":[]}),
                ]
            };
            let options = options::metadata(&prepared, &environment)
                .map_err(RpcError::into_internal_error)?;
            let session = if let Some(id) = artifact.session {
                if !init.agent_capabilities.load_session {
                    return Err(RpcError::into_internal_error(
                        Error::HistoryLoadingUnsupported,
                    ));
                }
                let request: LoadSessionRequest = serde_json::from_value(
                    json!({"sessionId":id,"cwd":context.root,"mcpServers":servers,"_meta":options}),
                )
                .map_err(RpcError::into_internal_error)?;
                peer.request(request).await?;
                id.to_string().into()
            } else {
                let request: NewSessionRequest = serde_json::from_value(
                    json!({"cwd":context.root,"mcpServers":servers,"_meta":options}),
                )
                .map_err(RpcError::into_internal_error)?;
                peer.request(request).await?.session_id
            };
            for (config_id, value) in [("model", prepared.model.as_ref()), ("mode", "default")] {
                let request: SetSessionConfigOptionRequest = serde_json::from_value(
                    json!({"sessionId":session,"configId":config_id,"value":value}),
                )
                .map_err(RpcError::into_internal_error)?;
                let response = peer.request(request).await.map_err(|source| {
                    if config_id != "model" {
                        return source;
                    }
                    let error = Error::ModelSelection {
                        model: prepared.model.clone(),
                        source,
                    };
                    record_failure(
                        &foreground_state,
                        StreamError::other(error.to_string()).with_source(error),
                    )
                })?;
                // Claude Code can normalize a model alias to a canonical identifier.
                let applied = response.config_options.iter().any(|option| option.id.0 == config_id
                && matches!(&option.kind, SessionConfigKind::Select { current_value } if
                    (config_id == "model" && !current_value.0.is_empty()) || current_value.0 == value));
                if !applied {
                    return Err(RpcError::into_internal_error(Error::SettingNotApplied {
                        setting: config_id.into(),
                    }));
                }
            }
            {
                let mut state = foreground_state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                if !state.authenticated {
                    return Err(RpcError::into_internal_error(Error::SubscriptionRequired));
                }
                state.session = Some(session.clone());
                state.live = true;
            }
            let request: PromptRequest = serde_json::from_value(
                json!({"sessionId":session,"prompt":[{"type":"text","text":prepared.prompt}]}),
            )
            .map_err(RpcError::into_internal_error)?;
            peer.request(request).await?;
            let events = {
                let mut state = foreground_state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                state
                    .final_events
                    .take()
                    .ok_or_else(|| RpcError::into_internal_error(Error::MissingSdkResult))?
            };
            emit(&sender, events).await
        })
    });
    let result = transport.connect(handler, foreground);
    tokio::pin!(result);
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    loop {
        tokio::select! {
            result = &mut result => {
                debug!(usage = %state.lock().unwrap_or_else(PoisonError::into_inner).usage_snapshot(), "Claude ACP usage snapshot");
                if let Some(error) = state.lock().unwrap_or_else(PoisonError::into_inner).failure.take() {
                    return Err(Error::Stream(Box::new(error)));
                }
                return result.map_err(Error::Protocol);
            },
            _ = tick.tick() => {
                // Anthropic can buffer an entire argument value. Keep the same
                // liveness policy as the direct flow while that block is open.
                let pending = state.lock().unwrap_or_else(PoisonError::into_inner).has_tool_activity();
                if pending {
                    emit(&heartbeat, vec![Event::KeepAlive]).await.map_err(Error::Protocol)?;
                }
            }
        }
    }
}

fn configure_storage_environment(
    environment: &mut BTreeMap<String, String>,
    configured: Option<&str>,
    project: &str,
) {
    // Setting CLAUDE_CONFIG_DIR can select a different Keychain entry even
    // when it names the default directory. Preserve the login environment.
    // Without an explicit config directory, resume finds our file by ID.
    if configured.is_some() {
        environment.insert("CLAUDE_CODE_PROJECT_DIR_NAME".into(), project.into());
    }
}

fn project_name(context: &QueryContext) -> String {
    if let Some(invocation) = &context.invocation {
        let name = format!(
            "jp-{}-{}",
            invocation.conversation_id, invocation.workspace_id
        );
        if name.len() <= 64
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return name;
        }
        return format!(
            "jp-{}",
            &format!("{:x}", Sha256::digest(name.as_bytes()))[..48]
        );
    }
    // Auxiliary requests have no conversation binding; their session files
    // remain independent even when they share this storage directory.
    format!(
        "jp-aux-{}",
        &format!("{:x}", Sha256::digest(context.root.as_str().as_bytes()))[..48]
    )
}

fn native_directory() -> Result<Utf8PathBuf, Error> {
    let directory = env::var("CLAUDE_CONFIG_DIR")
        .or_else(|_| {
            #[cfg(windows)]
            let home = env::var("USERPROFILE").or_else(|_| env::var("HOME"));
            #[cfg(not(windows))]
            let home = env::var("HOME");
            home.map(|home| format!("{home}/.claude"))
        })
        .map(Utf8PathBuf::from)
        .map_err(|_| Error::NativeDirectory)?;
    if !directory.is_absolute() {
        return Err(Error::NativeDirectory);
    }
    Ok(directory)
}

/// The transcript a connection resumes from.
///
/// `session` decides which request JP opens with: `session/load` when there is
/// one to resume, `session/new` otherwise.
/// `path` is the file backing it, which only a run with an adapter to read it
/// needs.
pub(super) struct NativeArtifact {
    pub(super) session: Option<Uuid>,
    pub(super) path: Option<Utf8PathBuf>,
}

impl NativeArtifact {
    fn write(
        prepared: &PreparedRequest,
        root: &Utf8Path,
        directory: &Utf8Path,
        project: &str,
    ) -> Result<Self, Error> {
        if prepared.history.is_empty() {
            return Ok(Self {
                session: None,
                path: None,
            });
        }
        if !root.is_absolute() {
            return Err(Error::NativeDirectory);
        }
        let directory = directory.join("projects").join(project);
        fs::create_dir_all(&directory).map_err(Error::NativeIo)?;
        let session = Uuid::new_v4();
        let path = directory.join(format!("{session}.jsonl"));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(&path).map_err(Error::NativeIo)?;
        let artifact = Self {
            session: Some(session),
            path: Some(path),
        };
        for record in prepared.records(session, root, Utc::now()) {
            serde_json::to_writer(&mut file, &record).map_err(Error::NativeJson)?;
            file.write_all(b"\n").map_err(Error::NativeIo)?;
        }
        file.flush().map_err(Error::NativeIo)?;
        Ok(artifact)
    }
}

impl Drop for NativeArtifact {
    fn drop(&mut self) {
        if let Some(path) = &self.path
            && let Err(error) = fs::remove_file(path)
            && error.kind() != io::ErrorKind::NotFound
        {
            warn!(%error, %path, "Could not remove derived Claude transcript");
        }
    }
}

#[cfg(test)]
#[path = "transport_tests.rs"]
mod tests;
