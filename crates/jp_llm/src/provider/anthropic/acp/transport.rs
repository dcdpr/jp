//! One isolated ACP connection per provider request.

use std::{
    collections::BTreeMap,
    env, fs,
    io::{self, Write as _},
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use agent_client_protocol::{
    AcpAgent, AcpAgentConfig, Client, ConnectTo, Error as RpcError, on_receive_notification,
    on_receive_request,
    schema::{
        ProtocolVersion,
        v1::{
            InitializeRequest, LoadSessionRequest, NewSessionRequest, PromptRequest,
            RequestPermissionRequest, SessionConfigKind, SessionNotification,
            SetSessionConfigOptionRequest,
        },
    },
};
use camino::{Utf8Path, Utf8PathBuf};
use chrono::Utc;
use futures::StreamExt as _;
use jp_config::assistant::{request::CachePolicy, tool_choice::ToolChoice};
use serde_json::json;
use tokio::sync::mpsc;
use tracing::{debug, warn};
use uuid::Uuid;

use super::{
    Error, options,
    protocol::{AuthUpdate, SdkNotification, State},
    removes_variable,
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
    });
    while let Some(event) = receiver.recv().await { yield event; }
    }
    .boxed())
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

async fn run(
    prepared: PreparedRequest,
    context: QueryContext,
    tools: Vec<String>,
    cache: CachePolicy,
    sender: mpsc::Sender<Result<Event, StreamError>>,
) -> Result<(), Error> {
    if !cfg!(unix) {
        return Err(Error::PlatformUnsupported);
    }
    let artifact = NativeArtifact::write(&prepared, &context.root)?;
    let environment = options::environment(&prepared, cache);
    // `env -u` removes routing variables without passing their values in argv.
    // The ACP SDK's launcher owns descendant termination but only supports
    // setting environment variables, not removing inherited ones. Explicit JP
    // overrides must survive `env -u`; envs() replaces their inherited values.
    let mut launch = AcpAgentConfig::new("env");
    for (key, _) in env::vars_os() {
        if let Some(key) = key.to_str()
            && removes_variable(key)
            && !environment.contains_key(key)
        {
            launch = launch.args(["-u", key]);
        }
    }
    launch = launch.arg("claude-agent-acp");
    launch = launch.envs(&environment);
    drive(
        prepared,
        context,
        tools,
        environment,
        artifact,
        AcpAgent::new(launch),
        sender,
    )
    .await
}

#[expect(
    clippy::too_many_lines,
    reason = "Keep the connection handlers and foreground request with their shared session state"
)]
async fn drive(
    prepared: PreparedRequest,
    context: QueryContext,
    tools: Vec<String>,
    environment: BTreeMap<String, String>,
    artifact: NativeArtifact,
    agent: impl ConnectTo<Client>,
    sender: mpsc::Sender<Result<Event, StreamError>>,
) -> Result<(), Error> {
    let state = Arc::new(Mutex::new(State::new(
        prepared.model.clone(),
        tools.iter().cloned(),
        prepared.schema.is_some(),
    )));
    let sdk_state = state.clone();
    let sdk_sender = sender.clone();
    let permission_state = state.clone();
    let permission_sender = sender.clone();
    let auth_state = state.clone();
    let update_state = state.clone();
    let connection = Client
        .builder()
        .on_receive_notification(
            async move |notification: AuthUpdate, _cx| {
                let mut state = auth_state.lock().unwrap_or_else(PoisonError::into_inner);
                state.authenticated = notification.auth_status.is_subscription();
                if state.live && !state.authenticated {
                    return Err(RpcError::into_internal_error(Error::SubscriptionRequired));
                }
                Ok(())
            },
            on_receive_notification!(),
        )
        .on_receive_notification(
            async move |notification: SdkNotification, _cx| {
                let result = sdk_state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .sdk(notification);
                let events = result.map_err(|error| record_failure(&sdk_state, error))?;
                emit(&sdk_sender, events).await
            },
            on_receive_notification!(),
        )
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                update_state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .observe(notification);
                Ok(())
            },
            on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: RequestPermissionRequest, responder, _cx| {
                let (response, events) = permission_state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .permission(request)
                    .map_err(RpcError::into_internal_error)?;
                emit(&permission_sender, events).await?;
                responder.respond(response)
            },
            on_receive_request!(),
        );
    let foreground_state = state.clone();
    let heartbeat = sender.clone();
    let result = connection.connect_with(agent, async move |cx| {
        let init = cx.send_request(InitializeRequest::new(ProtocolVersion::V1)).block_task().await?;
        if init.protocol_version != ProtocolVersion::V1 || (!tools.is_empty() && !init.agent_capabilities.mcp_capabilities.http) {
            return Err(RpcError::into_internal_error(Error::InitializationCapabilities));
        }
        let servers = if tools.is_empty() { vec![] } else {
            vec![json!({"type":"http","name":"jp","url":context.mcp_endpoint.as_ref().expect("tool host validated").as_str(),"headers":[]})]
        };
        let options = options::metadata(&prepared, &environment).map_err(RpcError::into_internal_error)?;
        let session = if let Some(id) = artifact.session {
            if !init.agent_capabilities.load_session { return Err(RpcError::into_internal_error(Error::HistoryLoadingUnsupported)); }
            let request: LoadSessionRequest = serde_json::from_value(json!({"sessionId":id,"cwd":context.root,"mcpServers":servers,"_meta":options})).map_err(RpcError::into_internal_error)?;
            cx.send_request(request).block_task().await?;
            id.to_string().into()
        } else {
            let request: NewSessionRequest = serde_json::from_value(json!({"cwd":context.root,"mcpServers":servers,"_meta":options})).map_err(RpcError::into_internal_error)?;
            cx.send_request(request).block_task().await?.session_id
        };
        for (config_id, value) in [("model", prepared.model.as_ref()), ("mode", "default")] {
            let request: SetSessionConfigOptionRequest = serde_json::from_value(json!({"sessionId":session,"configId":config_id,"value":value})).map_err(RpcError::into_internal_error)?;
            let response = cx.send_request(request).block_task().await.map_err(|source| {
                if config_id != "model" { return source; }
                let error = Error::ModelSelection { model: prepared.model.clone(), source };
                record_failure(&foreground_state, StreamError::other(error.to_string()).with_source(error))
            })?;
            // Claude Code can normalize a model alias to a canonical identifier.
            let applied = response.config_options.iter().any(|option| option.id.0.as_ref() == config_id
                && matches!(&option.kind, SessionConfigKind::Select(select) if
                    (config_id == "model" && !select.current_value.0.is_empty()) || select.current_value.0.as_ref() == value));
            if !applied { return Err(RpcError::into_internal_error(Error::SettingNotApplied { setting: config_id.into() })); }
        }
        {
            let mut state = foreground_state.lock().unwrap_or_else(PoisonError::into_inner);
            if !state.authenticated { return Err(RpcError::into_internal_error(Error::SubscriptionRequired)); }
            state.session = Some(session.clone());
            state.live = true;
        }
        let request: PromptRequest = serde_json::from_value(json!({"sessionId":session,"prompt":[{"type":"text","text":prepared.prompt}]})).map_err(RpcError::into_internal_error)?;
        cx.send_request(request).block_task().await?;
        let events = {
            let mut state = foreground_state.lock().unwrap_or_else(PoisonError::into_inner);
            state.final_events.take().ok_or_else(|| RpcError::into_internal_error(Error::MissingSdkResult))?
        };
        emit(&sender, events).await
    });
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
                // Host interactions are activity, not a silent model stream.
                let pending = !state.lock().unwrap_or_else(PoisonError::into_inner).pending_tools.is_empty();
                if pending {
                    emit(&heartbeat, vec![Event::KeepAlive]).await.map_err(Error::Protocol)?;
                }
            }
        }
    }
}

struct NativeArtifact {
    session: Option<Uuid>,
    path: Option<Utf8PathBuf>,
}

impl NativeArtifact {
    fn write(prepared: &PreparedRequest, root: &Utf8Path) -> Result<Self, Error> {
        if prepared.history.is_empty() {
            return Ok(Self {
                session: None,
                path: None,
            });
        }
        let home = env::var("CLAUDE_CONFIG_DIR")
            .or_else(|_| env::var("HOME").map(|home| format!("{home}/.claude")))
            .map_err(|_| Error::NativeDirectory)?;
        let directory = Utf8PathBuf::from(home);
        if !directory.is_absolute() || !root.is_absolute() {
            return Err(Error::NativeDirectory);
        }
        let project: String = root
            .as_str()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        if project.len() > 200 {
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
