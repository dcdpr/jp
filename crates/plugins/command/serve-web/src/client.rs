//! Protocol client for communicating with the JP host.
//!
//! Wraps the shared [`Client`] with the requests the web UI makes, each
//! answering with what the host sent or a [`ClientError`].
//! Thread-safe and shareable across axum handlers.

use std::{io::BufRead, time::Duration};

use jp_plugin::{
    client::{self, Answer, Client, Writer},
    message::{
        ConfigEntry, ConversationRequest, ConversationSummary, DraftResponse, EventsResponse,
        ExitMessage, HostToPlugin, InterruptRequest, OptionalId, PluginToHost, QueryRequest,
        ReadEventsRequest, SetTitleRequest, WriteDraftRequest,
    },
};
use tokio::sync::watch;

/// How long a request waits for a matching host response before giving up.
///
/// Guards against a lost or id-less response leaving a handler awaiting
/// forever, which would otherwise stall graceful shutdown.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// A protocol client that talks to the JP host over stdin/stdout.
///
/// Cheap to clone: pass it into axum state directly.
#[derive(Clone)]
pub struct PluginClient {
    client: Client,
}

/// The still-running turn a newly created conversation was started with.
///
/// Held by whoever needs to know when that turn ends — which is not the
/// request that created the conversation, since it returned as soon as there
/// was somewhere to send the reader.
pub struct TurnOutcome {
    finished: Answer,
}

impl TurnOutcome {
    /// Wait for the turn to finish.
    ///
    /// Takes as long as the turn does, which is however long the assistant
    /// takes: an agent loop that calls tools between thoughts can run for the
    /// better part of an hour.
    /// Waits without a deadline for that reason.
    /// The wait is already bounded by the connection — the reader drops every
    /// waiter when the host's stdout closes — so a deadline here could only
    /// report a failure for a turn that is still running, and the host would go
    /// on holding the conversation after this said it had stopped.
    pub async fn finished(self) -> Result<(), ClientError> {
        match self.finished.wait(None).await? {
            HostToPlugin::QueryComplete(_) => Ok(()),
            HostToPlugin::Error(e) => Err(ClientError::Host(e.message)),
            other => Err(ClientError::Unexpected(format!("{other:?}"))),
        }
    }
}

impl PluginClient {
    /// Start the protocol client.
    ///
    /// Spawns a background thread that reads from `stdin` and dispatches
    /// responses to pending requests.
    /// Returns the client and a watch channel that signals when a shutdown
    /// message is received from the host.
    pub fn start(
        stdin: impl BufRead + Send + 'static,
        writer: Writer,
    ) -> Result<(Self, watch::Receiver<bool>), ClientError> {
        let client = Client::new(writer);
        let shutdown = client.listen(stdin)?;

        Ok((Self { client }, shutdown))
    }

    /// Request the list of conversations from the host.
    pub async fn list_conversations(&self) -> Result<Vec<ConversationSummary>, ClientError> {
        let msg = PluginToHost::ListConversations(OptionalId {
            id: Some(self.client.next_id()),
        });

        match self.request(&msg).await? {
            HostToPlugin::Conversations(resp) => Ok(resp.data),
            HostToPlugin::Error(e) => Err(ClientError::Host(e.message)),
            other => Err(ClientError::Unexpected(format!("{other:?}"))),
        }
    }

    /// Request events for a specific conversation.
    pub async fn read_events(&self, conversation: &str) -> Result<EventsResponse, ClientError> {
        let msg = PluginToHost::ReadEvents(ReadEventsRequest {
            id: Some(self.client.next_id()),
            conversation: conversation.to_owned(),
        });

        match self.request(&msg).await? {
            HostToPlugin::Events(resp) => Ok(resp),
            HostToPlugin::Error(e) => Err(ClientError::Host(e.message)),
            other => Err(ClientError::Unexpected(format!("{other:?}"))),
        }
    }

    /// Ask the host to run a turn on a conversation.
    ///
    /// Returns once the turn has finished and its events are persisted; read
    /// them back with [`Self::read_events`].
    /// The host owns the agent loop, so this resolves the model, calls the
    /// provider, and runs tools without the plugin seeing any of it.
    ///
    /// Waits without a deadline, for the reason [`TurnOutcome::finished`]
    /// gives.
    pub async fn query(
        &self,
        conversation: &str,
        content: &str,
        cfg: Vec<String>,
    ) -> Result<(), ClientError> {
        let msg = PluginToHost::Query(QueryRequest {
            new: false,
            title: None,
            cfg,
            id: Some(self.client.next_id()),
            conversation: conversation.to_owned(),
            content: content.to_owned(),
            schema: None,
            expires_in: None,
        });

        match self.client.request(&msg, None).await? {
            HostToPlugin::QueryComplete(_) => Ok(()),
            HostToPlugin::Error(e) => Err(ClientError::Host(e.message)),
            other => Err(ClientError::Unexpected(format!("{other:?}"))),
        }
    }

    /// List the configurations a new conversation can be started with.
    pub async fn list_configs(&self) -> Result<Vec<ConfigEntry>, ClientError> {
        let msg = PluginToHost::ListConfigs(OptionalId {
            id: Some(self.client.next_id()),
        });

        match self.request(&msg).await? {
            HostToPlugin::Configs(resp) => Ok(resp.data),
            HostToPlugin::Error(e) => Err(ClientError::Host(e.message)),
            other => Err(ClientError::Unexpected(format!("{other:?}"))),
        }
    }

    /// Start a conversation and set its first turn running.
    ///
    /// Returns the id the host gave it, which is the only place that id exists:
    /// the conversation did not exist when the request was sent.
    ///
    /// Returns as soon as the conversation exists, not when the turn finishes.
    /// The turn's progress is in the conversation's events, which is where a
    /// reader sent to it will be looking anyway.
    pub async fn start_conversation(
        &self,
        content: &str,
        title: Option<String>,
        cfg: Vec<String>,
    ) -> Result<(String, TurnOutcome), ClientError> {
        let id = self.client.next_id();
        let msg = PluginToHost::Query(QueryRequest {
            id: Some(id.clone()),
            conversation: String::new(),
            content: content.to_owned(),
            new: true,
            title,
            cfg,
            schema: None,
            expires_in: None,
        });

        // Both answers are waited for before the request goes out. Waiting for
        // the second only after the first arrives would race a turn that
        // finished in between, and a lost completion leaves the conversation
        // marked busy forever.
        let [created, finished] = self.client.send_expecting(&msg)?;

        // The default timeout, not the turn's: the host answers as soon as the
        // conversation exists, without waiting for its first turn.
        let conversation = match created.wait(Some(REQUEST_TIMEOUT)).await {
            Ok(HostToPlugin::Created(resp)) => resp.conversation,
            answer => {
                self.client.forget(&id);
                return Err(match answer {
                    Ok(HostToPlugin::Error(e)) => ClientError::Host(e.message),
                    Ok(other) => ClientError::Unexpected(format!("{other:?}")),
                    Err(error) => error.into(),
                });
            }
        };

        Ok((conversation, TurnOutcome { finished }))
    }

    /// Move a conversation to the archive.
    pub async fn archive(&self, conversation: &str) -> Result<(), ClientError> {
        let msg = PluginToHost::ArchiveConversation(ConversationRequest {
            id: Some(self.client.next_id()),
            conversation: conversation.to_owned(),
        });

        self.done(&msg).await
    }

    /// Rename a conversation.
    /// An empty title clears it.
    pub async fn set_title(&self, conversation: &str, title: &str) -> Result<(), ClientError> {
        let msg = PluginToHost::SetTitle(SetTitleRequest {
            id: Some(self.client.next_id()),
            conversation: conversation.to_owned(),
            title: Some(title.to_owned()),
        });

        self.done(&msg).await
    }

    /// Send a request whose only answer is whether it worked.
    async fn done(&self, msg: &PluginToHost) -> Result<(), ClientError> {
        match self.request(msg).await? {
            HostToPlugin::Done(_) => Ok(()),
            HostToPlugin::Error(e) => Err(ClientError::Host(e.message)),
            other => Err(ClientError::Unexpected(format!("{other:?}"))),
        }
    }

    /// Read a conversation's query draft.
    pub async fn read_draft(&self, conversation: &str) -> Result<DraftResponse, ClientError> {
        let msg = PluginToHost::ReadDraft(ConversationRequest {
            id: Some(self.client.next_id()),
            conversation: conversation.to_owned(),
        });

        match self.request(&msg).await? {
            HostToPlugin::Draft(resp) => Ok(resp),
            HostToPlugin::Error(e) => Err(ClientError::Host(e.message)),
            other => Err(ClientError::Unexpected(format!("{other:?}"))),
        }
    }

    /// Replace a conversation's query draft, if it still matches `revision`.
    ///
    /// A refusal comes back as a [`DraftResponse`] with `conflict` set and the
    /// current draft attached, rather than as an error: the caller needs the
    /// other side's text to do anything sensible about it.
    pub async fn write_draft(
        &self,
        conversation: &str,
        content: &str,
        revision: Option<String>,
    ) -> Result<DraftResponse, ClientError> {
        let msg = PluginToHost::WriteDraft(WriteDraftRequest {
            id: Some(self.client.next_id()),
            conversation: conversation.to_owned(),
            content: content.to_owned(),
            revision,
        });

        match self.request(&msg).await? {
            HostToPlugin::Draft(resp) => Ok(resp),
            HostToPlugin::Error(e) => Err(ClientError::Host(e.message)),
            other => Err(ClientError::Unexpected(format!("{other:?}"))),
        }
    }

    /// Ask the host to stop the turn it is running.
    ///
    /// Returns as soon as the request is on the wire.
    /// There is no reply to wait for: the interrupt lands in the conversation,
    /// and the turn's own outcome still arrives as the answer to the `query`
    /// that started it.
    pub fn interrupt(&self, conversation: &str) -> Result<(), ClientError> {
        Ok(self
            .client
            .send(&PluginToHost::Interrupt(InterruptRequest::stop(
                conversation.to_owned(),
            )))?)
    }

    /// Interrupt the turn the host is running and answer it with `content`.
    ///
    /// The assistant stops where it is, what it had produced is kept, and
    /// `content` becomes the next thing it reads — all within the turn that
    /// was already running, so the conversation is never unlocked in between.
    ///
    /// Awaited rather than fired off, because the answer is the difference
    /// between a delivered reply and one aimed at a turn that had just
    /// finished.
    /// [`ClientError::Host`] means the turn never acted on the message and the
    /// caller still owns it.
    ///
    /// No deadline: the host answers when the turn acts on the reply, which for
    /// a turn still starting its MCP servers can be minutes away, and a reply
    /// given up on here would still land there.
    /// The host always answers, because every turn ends.
    pub async fn reply(&self, conversation: &str, content: &str) -> Result<(), ClientError> {
        let msg = PluginToHost::Interrupt(
            InterruptRequest::reply(conversation.to_owned(), content.to_owned())
                .with_id(self.client.next_id()),
        );

        match self.client.request(&msg, None).await? {
            HostToPlugin::Done(_) => Ok(()),
            HostToPlugin::Error(e) => Err(ClientError::Host(e.message)),
            other => Err(ClientError::Unexpected(format!("{other:?}"))),
        }
    }

    /// Send a request and await its answer, for up to [`REQUEST_TIMEOUT`].
    async fn request(&self, msg: &PluginToHost) -> Result<HostToPlugin, ClientError> {
        Ok(self.client.request(msg, Some(REQUEST_TIMEOUT)).await?)
    }

    /// Send an exit message to the host.
    pub fn send_exit(&self, code: u8) {
        drop(self.client.send(&PluginToHost::Exit(ExitMessage {
            code,
            reason: None,
            expected: false,
        })));
    }
}

/// Errors from the plugin client.
#[derive(Debug)]
pub enum ClientError {
    /// The host returned an error response.
    Host(String),
    /// Unexpected response type.
    Unexpected(String),
    /// The response channel was closed (reader thread died).
    ChannelClosed,
    /// Protocol-level I/O or serialization error.
    Protocol(String),
    /// No response arrived within [`REQUEST_TIMEOUT`].
    Timeout,
}

impl From<client::Error> for ClientError {
    fn from(error: client::Error) -> Self {
        match error {
            client::Error::Closed => Self::ChannelClosed,
            client::Error::TimedOut => Self::Timeout,
            client::Error::Write(error) => Self::Protocol(error.to_string()),
            error @ (client::Error::Listen(_) | client::Error::NoId) => {
                Self::Protocol(error.to_string())
            }
        }
    }
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Host(msg) => write!(f, "host error: {msg}"),
            Self::Unexpected(msg) => write!(f, "unexpected response: {msg}"),
            Self::ChannelClosed => write!(f, "protocol channel closed"),
            Self::Protocol(msg) => write!(f, "protocol error: {msg}"),
            Self::Timeout => write!(f, "timed out waiting for host response"),
        }
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
