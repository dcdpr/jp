//! Axum router and HTTP handlers.

use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex},
};

use axum::{
    Form, Json, Router,
    extract::{Path, Query, Request, State},
    http::{HeaderMap, Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Redirect, Response},
};
use jp_plugin::message::LockState;
use maud::Markup;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tracing::{debug, error, info, warn};

use crate::{
    client::{ClientError, PluginClient},
    render, style, views,
};

/// Shared state for axum handlers.
#[derive(Clone)]
struct AppState {
    client: PluginClient,

    /// What each conversation's most recent delegated turn is doing.
    ///
    /// A turn outlives the request that started it, so its outcome has to live
    /// somewhere the polling endpoint can find it.
    turns: Arc<Mutex<HashMap<String, TurnStatus>>>,

    /// Identifies this run of the server.
    ///
    /// A page polls it and can tell that the process it loaded from has been
    /// replaced, which is the only way it can know its own markup and styles
    /// are out of date.
    /// Data recovers on its own; the page itself does not.
    boot: String,
}

/// The state of a turn started from the browser.
#[derive(Debug, Clone)]
enum TurnStatus {
    /// The host is working on it.
    ///
    /// `pending` is the message the browser submitted, held until it shows up
    /// in the transcript.
    /// The host appends the request only after it has waited for MCP servers
    /// and resolved tools, so there are a few seconds where the turn is
    /// underway and the conversation has no record of what was asked.
    /// Showing it from here closes that gap without moving the host's commit
    /// point.
    Running {
        pending: Option<String>,

        /// Which client asked for it, when one said.
        ///
        /// Kept here rather than on the lock: this distinction never leaves the
        /// process, so it is nobody else's business.
        /// Another peer only needs to know the turn is this server's, which the
        /// lock already says.
        client: Option<String>,

        /// How long the transcript was when the message was submitted, when the
        /// submitter said.
        ///
        /// What tells `pending` it can go: the request is recorded as soon as
        /// the transcript is longer than this, whether or not the assistant has
        /// already begun answering it.
        sent_at: Option<usize>,
    },

    /// It failed, and nobody has been told yet.
    Failed(String),
}

/// What the page needs to know about a turn this server started.
struct TurnView {
    running: bool,
    error: Option<String>,
    pending: Option<String>,
    client: Option<String>,
}

/// What stopping the running turn would take.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum StopMode {
    /// Nothing to stop.
    None,

    /// The asker started it.
    /// Stopping is theirs to do.
    Own,

    /// This server is running it, for somebody else.
    /// Stoppable, with a warning: the work belongs to another window, and they
    /// get no say.
    Shared,

    /// Another process entirely.
    /// There is no way to reach it from here — a signal would run that
    /// process's own interrupt policy, which may be to prompt a terminal nobody
    /// is watching.
    Unreachable,
}

/// Start the HTTP server on an already-bound listener and block until
/// `shutdown` resolves.
pub(crate) async fn serve(
    client: PluginClient,
    listener: std::net::TcpListener,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), String> {
    let state = AppState {
        client,
        turns: Arc::new(Mutex::new(HashMap::new())),
        boot: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or_else(|_| "unknown".to_owned(), |d| d.as_millis().to_string()),
    };

    let app = Router::new()
        .route("/", axum::routing::get(index))
        .route("/conversations", axum::routing::get(conversation_list))
        .route(
            "/conversations/{id}",
            axum::routing::get(conversation_detail),
        )
        .route("/conversations/{id}/turn", axum::routing::post(start_turn))
        .route("/conversations/{id}/messages", axum::routing::get(messages))
        .route(
            "/conversations/{id}/interrupt",
            axum::routing::post(interrupt),
        )
        .route(
            "/conversations/new",
            axum::routing::get(new_conversation_form).post(start_conversation),
        )
        .route(
            "/conversations/{id}/draft",
            axum::routing::get(read_draft).post(write_draft),
        )
        .route(
            "/conversations/digest",
            axum::routing::get(conversation_digest),
        )
        .route(
            "/conversations/{id}/archive",
            axum::routing::post(archive_conversation),
        )
        .route("/conversations/{id}/title", axum::routing::post(set_title))
        .route("/configs", axum::routing::get(list_configs))
        .route("/status", axum::routing::get(status))
        .route("/assets/style.css", axum::routing::get(serve_css))
        .route("/assets/icon.svg", axum::routing::get(serve_icon))
        .route("/manifest.webmanifest", axum::routing::get(serve_manifest))
        .layer(middleware::from_fn(same_origin_only))
        .with_state(state);

    let local_addr = listener.local_addr().ok();
    let listener =
        TcpListener::from_std(listener).map_err(|e| format!("failed to adopt listener: {e}"))?;

    if let Some(addr) = local_addr {
        info!(%addr, "Web server listening");
    }

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
        .map_err(|e| format!("server error: {e}"))
}

/// Refuse a write that a page on some other origin asked for.
///
/// A form post needs no preflight, so any page a browser visits can submit one
/// here and start a turn, which spends tokens and runs whatever tools the
/// conversation allows.
/// The same-origin policy stops that page reading the answer, not sending the
/// request, and binding to loopback does not help: the request comes from the
/// user's own browser, which is already inside.
///
/// Reads are left alone.
/// They are as exposed as the port is, which the startup warning and the README
/// already say, and a `GET` is where a supervisor and a `curl` live.
async fn same_origin_only(request: Request, next: Next) -> Response {
    let writing = !matches!(*request.method(), Method::GET | Method::HEAD);

    if writing && !same_origin(request.headers()) {
        warn!(
            method = %request.method(),
            path = %request.uri().path(),
            "Refused a write from another origin",
        );

        return (
            StatusCode::FORBIDDEN,
            "This request came from another site.\n",
        )
            .into_response();
    }

    next.run(request).await
}

/// Whether a request came from one of this server's own pages.
///
/// `Sec-Fetch-Site` is the browser's own answer and is taken where it is given.
/// `Origin` against `Host` is the fallback for a browser too old to send it: a
/// cross-site form post carries the submitting page's origin, which is not this
/// server's.
///
/// A request carrying neither is allowed.
/// `curl`, a script, or another tool sends no such header, and no browser can
/// be made to omit both — so refusing here would lock out every non-browser
/// caller to stop nothing.
fn same_origin(headers: &HeaderMap) -> bool {
    if let Some(site) = headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
    {
        return site == "same-origin" || site == "none";
    }

    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        return true;
    };

    // Whatever the browser was told to connect to, which is the authority half
    // of the origin its own pages carry. Its absence with an `Origin` present
    // leaves nothing to compare against, and guessing is not worth it.
    headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|host| {
            origin
                .split_once("://")
                .is_some_and(|(_, authority)| authority == host)
        })
}

async fn index() -> Redirect {
    debug!("GET / -> redirect to /conversations");
    Redirect::permanent("/conversations")
}

async fn conversation_list(State(state): State<AppState>) -> Result<Markup, AppError> {
    debug!("GET /conversations");

    let conversations = state
        .client
        .list_conversations()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    debug!(count = conversations.len(), "Rendered conversation list");
    Ok(views::list::render(&conversations))
}

/// Whether this server is in the middle of anything.
///
/// Exists for whoever supervises the process: restarting to pick up a new build
/// aborts a turn in flight, because a turn started from the browser runs inside
/// the host process this plugin is attached to.
/// A supervisor polls this and waits for `busy` to go false before stopping the
/// server.
///
/// `busy` counts turns this server started and hasn't seen finish.
/// A turn started from a terminal is somebody else's process and doesn't appear
/// here — stopping this server wouldn't interrupt it.
#[derive(Debug, Serialize)]
struct StatusBody {
    busy: bool,
    turns: Vec<String>,
}

async fn status(State(state): State<AppState>) -> Json<StatusBody> {
    let turns: Vec<String> = state
        .turns
        .lock()
        .expect("turns lock poisoned")
        .iter()
        .filter(|(_, status)| matches!(status, TurnStatus::Running { .. }))
        .map(|(id, _)| id.clone())
        .collect();

    Json(StatusBody {
        busy: !turns.is_empty(),
        turns,
    })
}

/// The configuration choices a form carries.
///
/// One `cfg` field per row of the chooser, each holding a whole `--cfg`
/// argument: a configuration to load by name, or a value to assign.
/// Both are what `--cfg` takes, and posting them under one name is what
/// preserves the order they were arranged in.
#[derive(Debug, Default)]
struct CfgFields {
    args: Vec<String>,
}

impl CfgFields {
    /// Take one decoded form pair, reporting whether it belonged here.
    ///
    /// An empty argument is a row nothing was chosen in, and is dropped.
    ///
    /// What is kept is trimmed: a soft keyboard puts a space after a word it
    /// thinks is finished, and an argument carrying one reaches the config
    /// parser as a different key or value than the one that was typed.
    fn accept(&mut self, key: &str, value: &str) -> bool {
        if key != "cfg" {
            return false;
        }

        let argument = value.trim();
        if !argument.is_empty() {
            self.args.push(argument.to_owned());
        }

        true
    }

    /// The `--cfg` arguments this form asks for, in the order they apply.
    fn args(&self) -> &[String] {
        &self.args
    }
}

/// A new turn, as posted by the composer form.
///
/// Read from decoded pairs rather than through `Form`, for the same reason the
/// new-conversation form is: the chooser posts `cfg` once per row, and the
/// urlencoded deserialiser cannot collect repeats.
#[derive(Debug, Default)]
struct TurnForm {
    content: String,
    cfg: CfgFields,
    client: Option<String>,

    /// How much of the transcript the submitting page held, which is what the
    /// provisional copy of this message is retired against.
    count: Option<usize>,
}

impl TurnForm {
    fn parse(body: &str) -> Self {
        let mut form = Self::default();

        for (key, value) in form_urlencoded::parse(body.as_bytes()) {
            if form.cfg.accept(&key, &value) {
                continue;
            }

            match key.as_ref() {
                "content" => form.content = value.into_owned(),
                // Without this the turn is recorded unattributed, and the page
                // that started it is told the turn is somebody else's.
                "client" => form.client = Some(value.into_owned()),
                "count" => form.count = value.parse().ok(),
                _ => {}
            }
        }

        form
    }
}

/// Start a turn on this conversation and send the browser straight back to it.
///
/// The turn runs in the background rather than on this request.
/// A turn can take many minutes, and holding the response open for it means the
/// page renders nothing until the whole thing is over: no request appearing, no
/// tool calls, no partial answer.
/// Returning immediately lets the page poll instead, and the turn loop persists
/// at every streaming boundary, so progress shows up as it happens.
///
/// Answers with `204` when the caller asks for JSON, and a redirect otherwise,
/// so the page can post in the background while a plain form post still lands
/// somewhere.
async fn start_turn(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
    body: String,
) -> Response {
    debug!(%id, "POST /conversations/{{id}}/turn");

    let form = TurnForm::parse(&body);

    // The page posts in the background and updates itself from the poll, so it
    // wants nothing back. A plain form post has no such option and needs somewhere
    // to land.
    let wants_json = headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|accept| accept.contains("application/json"));

    let content = form.content.trim().to_owned();

    // Built before the turn is spawned, which takes ownership of `id`.
    //
    // The provisional message is rendered here rather than left to the next poll:
    // that would cost a second round trip and a re-render of the whole transcript,
    // and a second of nothing after pressing send reads as a failure. Rendered by
    // the same function the poll would use, so it is the final markup, not an
    // approximation of it.
    let response = if wants_json {
        Json(TurnStarted {
            pending: views::detail::pending(&content).into_string(),
        })
        .into_response()
    } else {
        Redirect::to(&format!("/conversations/{id}")).into_response()
    };

    if content.is_empty() {
        return response;
    }

    // Sending while a turn runs interrupts it and answers it, inside that turn.
    //
    // Not a stop followed by a second turn: the conversation is never unlocked in
    // between, so there is no window for the send to be refused as already-locked
    // and no need to guess when the first turn has finished unwinding.
    // This is what Ctrl-C then `[r] Reply` does at a terminal.
    let busy = matches!(
        state.turns.lock().expect("turns lock poisoned").get(&id),
        Some(TurnStatus::Running { .. })
    );

    if busy && let Some(error) = busy_refusal(&form) {
        return (
            StatusCode::CONFLICT,
            Json(TurnRefused {
                error: error.to_owned(),
            }),
        )
            .into_response();
    }

    if busy {
        match state.client.reply(&id, &content).await {
            Ok(()) => {
                // The same turn, told something new. Whoever started it still
                // owns it, so only the provisional copy of the message changes.
                if let Some(TurnStatus::Running { pending, .. }) = state
                    .turns
                    .lock()
                    .expect("turns lock poisoned")
                    .get_mut(&id)
                {
                    *pending = Some(content);
                }

                return response;
            }

            // The turn ended without acting on the reply. There is nothing left
            // to interrupt, which makes this an ordinary message: falling
            // through starts a turn with it.
            Err(ClientError::Host(error)) => {
                debug!(%id, %error, "No turn left to reply to; starting one instead.");
            }

            // The host gave no answer, so whether the turn has the message is
            // unknown. Starting a second turn with it could send it twice.
            Err(error) => {
                error!(%id, %error, "Reply failed");
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(TurnRefused {
                        error: format!("The message could not be delivered: {error}"),
                    }),
                )
                    .into_response();
            }
        }
    }

    state
        .turns
        .lock()
        .expect("turns lock poisoned")
        .insert(id.clone(), TurnStatus::Running {
            pending: Some(content.clone()),
            client: form.client.clone(),
            sent_at: form.count,
        });

    let client = state.client.clone();
    let turns = Arc::clone(&state.turns);
    let cfg = form.cfg.args().to_vec();
    tokio::spawn(async move {
        let failure = match client.query(&id, &content, cfg).await {
            Ok(()) => {
                info!(%id, "Turn completed");
                None
            }
            Err(error) => {
                error!(%id, %error, "Turn failed");
                Some(TurnStatus::Failed(error.to_string()))
            }
        };

        let mut turns = turns.lock().expect("turns lock poisoned");
        match failure {
            Some(failed) => turns.insert(id, failed),
            None => turns.remove(&id),
        };
    });

    response
}

/// Why a message for a running turn cannot be delivered as it was sent.
///
/// A reply joins the turn already running, and that turn keeps the
/// configuration it started with, so configuration chosen alongside the reply
/// has nowhere to go.
/// Refusing keeps both the message and the choices on the page, rather than
/// delivering one and quietly dropping the other.
fn busy_refusal(form: &TurnForm) -> Option<&'static str> {
    (!form.cfg.args().is_empty()).then_some(
        "A turn is running, and configuration applies to a new turn. Stop it first, or clear the \
         configuration to reply.",
    )
}

/// Stop the turn the host is running, then send the browser back.
///
/// The turn ends the way an interrupted terminal turn does: whatever the
/// assistant produced so far is kept, and the conversation is left in a state
/// another turn can continue from.
/// Answers `204` for a background post and a redirect otherwise, so the page
/// can stop a turn without navigating while the form still works on its own.
async fn interrupt(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
) -> Response {
    debug!(%id, "POST /conversations/{{id}}/interrupt");

    if let Err(error) = state.client.interrupt(&id) {
        error!(%id, %error, "Interrupt failed");
        state
            .turns
            .lock()
            .expect("turns lock poisoned")
            .insert(id.clone(), TurnStatus::Failed(error.to_string()));
    }

    if wants_json(&headers) {
        StatusCode::NO_CONTENT.into_response()
    } else {
        Redirect::to(&format!("/conversations/{id}")).into_response()
    }
}

/// What the new-conversation form submits.
///
/// Read from decoded pairs rather than through `Form`, because the chooser
/// posts `cfg` once per row, and the urlencoded deserialiser behind `Form` has
/// no way to express "collect the repeats" — it sees the second `cfg` and
/// reports a string where a sequence was expected.
#[derive(Debug, Default)]
struct NewConversationForm {
    content: String,
    title: String,
    cfg: CfgFields,

    /// Which page is asking, so the turn it starts is attributed to it.
    client: Option<String>,
}

impl NewConversationForm {
    /// Read a form body, keeping every value of a repeated field.
    ///
    /// Unknown fields are ignored, which is the same latitude `Form` allows and
    /// keeps a stray browser-added field from failing the whole submission.
    fn parse(body: &str) -> Self {
        let mut form = Self::default();

        for (key, value) in form_urlencoded::parse(body.as_bytes()) {
            if form.cfg.accept(&key, &value) {
                continue;
            }

            match key.as_ref() {
                "content" => form.content = value.into_owned(),
                "title" => form.title = value.into_owned(),
                "client" => form.client = Some(value.into_owned()),
                _ => {}
            }
        }

        form
    }
}

async fn new_conversation_form(State(state): State<AppState>) -> Result<Markup, AppError> {
    debug!("GET /conversations/new");

    let configs = state
        .client
        .list_configs()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok(views::new::render(&configs, "", "", &[], None))
}

/// Start a conversation, then send the browser to it.
///
/// Unlike a turn on an existing conversation, this waits for the host: the
/// conversation has no id until the host has made one, and there is nowhere to
/// redirect to until then.
async fn start_conversation(
    State(state): State<AppState>,
    body: String,
) -> Result<Response, AppError> {
    debug!("POST /conversations/new");

    let form = NewConversationForm::parse(&body);

    let content = form.content.trim().to_owned();
    let title = Some(form.title.trim().to_owned()).filter(|t| !t.is_empty());

    let error = if content.is_empty() {
        Some("A message is required.".to_owned())
    } else {
        match state
            .client
            .start_conversation(&content, title, form.cfg.args().to_vec())
            .await
        {
            Ok((id, outcome)) => {
                info!(%id, "Started a conversation.");

                // Recorded before the redirect, so the page it lands on shows the
                // working indicator and the stop button from its first paint. The
                // request is already in the conversation, so no pending copy is
                // needed.
                // Attributed to whoever filled the form, so the page they land on
                // can stop the first turn without being asked whose it is.
                state.turns.lock().expect("turns lock poisoned").insert(
                    id.clone(),
                    TurnStatus::Running {
                        pending: None,
                        client: form.client.clone(),
                        sent_at: None,
                    },
                );

                // Cleared when the turn ends, which is the half that has to exist:
                // an entry nothing ever removes leaves the conversation busy for the
                // life of the process.
                let turns = Arc::clone(&state.turns);
                let finished_id = id.clone();
                tokio::spawn(async move {
                    let failure = match outcome.finished().await {
                        Ok(()) => {
                            info!(id = %finished_id, "First turn completed.");
                            None
                        }
                        Err(error) => {
                            error!(id = %finished_id, %error, "First turn failed.");
                            Some(TurnStatus::Failed(error.to_string()))
                        }
                    };

                    let mut turns = turns.lock().expect("turns lock poisoned");
                    match failure {
                        Some(failed) => turns.insert(finished_id, failed),
                        None => turns.remove(&finished_id),
                    };
                });

                return Ok(Redirect::to(&format!("/conversations/{id}")).into_response());
            }
            Err(error) => {
                error!(%error, "Failed to start a conversation.");
                Some(error.to_string())
            }
        }
    };

    // Re-listed rather than carried through the failure: the form has to be drawn
    // again, and drawing it without its choices would lose them.
    let configs = state.client.list_configs().await.unwrap_or_default();

    Ok(views::new::render(
        &configs,
        &content,
        &form.title,
        form.cfg.args(),
        error.as_deref(),
    )
    .into_response())
}

/// Move a conversation to the archive.
///
/// Answers `204` for a background post and a redirect otherwise, so the list
/// page works with or without script.
async fn archive_conversation(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
) -> Response {
    debug!(%id, "POST /conversations/{{id}}/archive");

    match state.client.archive(&id).await {
        Ok(()) => {
            info!(%id, "Archived a conversation.");
            if wants_json(&headers) {
                StatusCode::NO_CONTENT.into_response()
            } else {
                Redirect::to("/conversations").into_response()
            }
        }
        Err(error) => {
            error!(%id, %error, "Failed to archive.");
            AppError::Internal(error.to_string()).into_response()
        }
    }
}

/// What a rename posts.
#[derive(Debug, Deserialize)]
struct TitleForm {
    title: String,
}

async fn set_title(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
    Form(form): Form<TitleForm>,
) -> Response {
    debug!(%id, "POST /conversations/{{id}}/title");

    match state.client.set_title(&id, &form.title).await {
        Ok(()) => {
            if wants_json(&headers) {
                StatusCode::NO_CONTENT.into_response()
            } else {
                Redirect::to(&format!("/conversations/{id}")).into_response()
            }
        }
        Err(error) => {
            error!(%id, %error, "Failed to rename.");
            AppError::Internal(error.to_string()).into_response()
        }
    }
}

/// Whether the caller posted in the background and wants no page back.
fn wants_json(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|accept| accept.contains("application/json"))
}

/// What the conversation list currently amounts to.
#[derive(Debug, Serialize)]
struct ConversationDigest {
    digest: String,
}

/// A fingerprint of the conversation list.
///
/// Enough for a page to tell whether the list it is showing is still the list,
/// without re-rendering one it already has.
///
/// A fingerprint rather than a count, because most of what a page would want to
/// redraw for leaves the count alone: a rename, a conversation being used, an
/// archive and a creation that happen to balance.
/// It costs nothing extra — answering at all means reading every
/// conversation's metadata, which is where a title comes from anyway.
async fn conversation_digest(
    State(state): State<AppState>,
) -> Result<Json<ConversationDigest>, AppError> {
    state
        .client
        .list_conversations()
        .await
        .map(|list| {
            Json(ConversationDigest {
                digest: views::list::digest(&list),
            })
        })
        .map_err(|e| AppError::Internal(e.to_string()))
}

/// The chooser for the configurations a message can be run under.
///
/// Fetched by the page when its configuration dialog is first opened, rather
/// than rendered into every conversation, since most visits never open it.
///
/// Markup rather than data: the new-conversation form offers the same choices,
/// and building the same chooser twice is how the two come to group, label and
/// post them differently.
async fn list_configs(State(state): State<AppState>) -> Result<Markup, AppError> {
    state
        .client
        .list_configs()
        .await
        .map(|entries| views::configs::chooser(&entries, &[]))
        .map_err(|e| AppError::Internal(e.to_string()))
}

/// The reply to a turn the page started in the background.
#[derive(Debug, Serialize)]
struct TurnStarted {
    /// The submitted message, rendered as it will appear in the transcript.
    pending: String,
}

/// Why a message was not sent, for the page to show while it keeps the text.
#[derive(Debug, Serialize)]
struct TurnRefused {
    error: String,
}

/// A query draft, as the page sees it.
#[derive(Debug, Serialize)]
struct DraftBody {
    content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    revision: Option<String>,
    conflict: bool,
}

impl From<jp_plugin::message::DraftResponse> for DraftBody {
    fn from(resp: jp_plugin::message::DraftResponse) -> Self {
        Self {
            content: resp.content,
            revision: resp.revision,
            conflict: resp.conflict,
        }
    }
}

/// What the page sends when saving a draft.
#[derive(Debug, Deserialize)]
struct DraftForm {
    content: String,

    /// The revision the page last saw, absent when it believes there is no
    /// draft.
    #[serde(default)]
    revision: Option<String>,
}

async fn read_draft(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<DraftBody>, AppError> {
    state
        .client
        .read_draft(&id)
        .await
        .map(|resp| Json(resp.into()))
        .map_err(|e| AppError::Internal(e.to_string()))
}

/// Save the draft, refusing if it moved since the page last read it.
///
/// A refusal is a 200 with `conflict` set, not an error: the body carries what
/// is on disk so the page can offer both rather than discard either.
async fn write_draft(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(form): Json<DraftForm>,
) -> Result<Json<DraftBody>, AppError> {
    state
        .client
        .write_draft(&id, &form.content, form.revision)
        .await
        .map(|resp| Json(resp.into()))
        .map_err(|e| AppError::Internal(e.to_string()))
}

/// The messages of a conversation, for the page's poller.
///
/// `count` lets the page skip the swap when nothing has changed, which is the
/// common case: the host re-reads the conversation from disk on every request,
/// so this reflects writes by any `jp` process, not just turns started here.
///
/// `running` says whether a turn started from this server is still going, which
/// is how the page knows to keep the working indicator up.
/// `error` is delivered once and then cleared, so a failure reaches whoever is
/// watching without sticking around forever.
#[derive(Debug, Serialize)]
struct MessagesBody {
    count: usize,

    /// Rendered messages the caller does not have, or the whole transcript when
    /// it cannot be told what it has.
    ///
    /// Absent when the caller is up to date.
    /// Rendering means running markdown over every message included, so sending
    /// the whole conversation once a second to produce something the page
    /// already has is waste at both ends.
    #[serde(skip_serializing_if = "Option::is_none")]
    html: Option<String>,

    /// Where `html` starts.
    ///
    /// Zero means it is the whole transcript and replaces what the caller has;
    /// anything else means it continues from there and is appended.
    /// A conversation only grows, so continuing is the usual case — and
    /// appending leaves the messages already on the page untouched, which is
    /// what keeps their disclosure state, their measured heights and the scroll
    /// position intact.
    from: usize,

    running: bool,

    /// The first index that can still change.
    ///
    /// The caller holds every rendered entry below this one in its final form,
    /// and sends it back on the next poll so an entry that changes after it was
    /// delivered is sent again.
    settled: usize,

    /// What stopping the running turn would take, from the asker's side.
    stop: StopMode,

    /// This run of the server; a change means the page should reload.
    boot: String,

    /// A submitted message the transcript doesn't carry yet, rendered the same
    /// way the real request will be so the swap is invisible.
    #[serde(skip_serializing_if = "Option::is_none")]
    pending: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// How many rendered events a page holds at once.
///
/// Enough that scrolling back a little never waits, small enough that the first
/// paint is cheap however long the conversation is.
/// The cost of getting this wrong is a fetch, not a broken view.
const WINDOW: usize = 200;

/// Where a caller's copy of the transcript stops being usable.
///
/// Everything from here on is sent again: either the caller does not hold it,
/// or it holds a rendering that has since changed.
///
/// `total` is how many rendered events there are, `held` how many the caller
/// says it has, `floor` the index from which its copy was provisional when it
/// was rendered, and `provisional` the index from which the transcript can
/// still change now, as [`provisional_from`] reports it.
///
/// The caller's own floor is what carries a tool call's result to it.
/// A call is rendered when it is requested and gains its result later, and by
/// the time that lands `settled` has moved past it — the next call in the
/// batch is the one waiting.
/// Only the caller knows how far back its copy went provisional.
///
/// A caller that cannot say either is given the tail, and a boundary that moved
/// backwards — the transcript was compacted or edited — wins over a floor
/// that predates it.
fn resend_from(
    total: usize,
    held: Option<usize>,
    floor: Option<usize>,
    provisional: usize,
) -> usize {
    held.filter(|&count| count <= total)
        .unwrap_or_else(|| total.saturating_sub(WINDOW))
        .min(floor.unwrap_or(usize::MAX))
        .min(provisional)
}

/// The first rendered entry that can still change while `running`.
///
/// A tool call waiting on its result is provisional wherever it sits.
/// While a turn runs, the newest entry is too: a block of assistant text or
/// reasoning grows with the next flush, and neither that nor a tool call
/// gaining its result moves the count.
///
/// This is also what the caller is told to send back as its floor, so the
/// newest entry has to be counted here rather than only when deciding what to
/// resend.
/// A block that grows once more before the turn ends is otherwise delivered in
/// its earlier form, and the poll after the turn ends has nothing left marking
/// it provisional.
fn provisional_from(rendered: &[render::RenderedEvent], running: bool) -> usize {
    let settled = render::settled_upto(rendered);

    if running && render::tail_can_change(rendered) {
        settled.min(rendered.len().saturating_sub(1))
    } else {
        settled
    }
}

/// What the poller already has, so the answer can leave it out.
#[derive(Debug, Deserialize)]
struct MessagesQuery {
    /// Ask for the events *before* this index instead of the ones after
    /// `count`.
    ///
    /// How the page walks backwards through a conversation it only holds the
    /// tail of.
    #[serde(default)]
    before: Option<usize>,

    /// With `before`, take everything preceding it rather than one window.
    ///
    /// For jumping to the top, and for the platforms that would rather hold the
    /// whole conversation than fetch it a window at a time.
    #[serde(default)]
    all: Option<u8>,

    /// The event count the caller last rendered.
    #[serde(default)]
    count: Option<usize>,

    /// The index from which the caller's copy is provisional.
    ///
    /// What it was last told was still in flight, so an entry that has changed
    /// since is sent again rather than left as the caller first drew it.
    #[serde(default)]
    settled: Option<usize>,

    /// Which client is asking, so a turn it started can be told from one it
    /// merely shares a server with.
    #[serde(default)]
    client: Option<String>,
}

async fn messages(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<MessagesQuery>,
) -> Result<Json<MessagesBody>, AppError> {
    let resp = read_conversation(&state, &id).await?;
    let rendered = render::render_events(&resp.data);

    // Walking backwards: a window of what came before what the caller holds.
    //
    // Read without consuming. A history fetch happens while the same page is
    // polling, and this answer carries no failure and no provisional message, so
    // taking either here would deliver it to nobody.
    if let Some(before) = query.before {
        let view = peek_turn_status(&state, &id);
        let running = view.running || resp.lock.is_held();
        let before = before.min(rendered.len());
        let from = if query.all.is_some_and(|all| all != 0) {
            0
        } else {
            before.saturating_sub(WINDOW)
        };

        return Ok(Json(MessagesBody {
            count: rendered.len(),
            from,
            html: (from < before)
                .then(|| views::detail::messages(&rendered[from..before]).into_string()),
            pending: None,
            settled: provisional_from(&rendered, running),
            stop: stop_mode(&view, resp.lock, query.client.as_deref()),
            boot: state.boot.clone(),
            running,
            error: None,
        }));
    }

    // The lock is the authority on whether a turn is running. Inferring it from a
    // transcript ending in a request cannot tell a live turn from one that
    // failed, and got that wrong in the direction that blocks the composer for a
    // conversation nothing is working on.
    //
    // `view.running` still counts, for the moment between this server starting a
    // turn and the host taking the lock.
    let view = take_turn_status(&state, &id, &rendered);
    let running = view.running || resp.lock.is_held();

    let settled = provisional_from(&rendered, running);
    let from = resend_from(rendered.len(), query.count, query.settled, settled);
    let stale = from != rendered.len();

    Ok(Json(MessagesBody {
        count: rendered.len(),
        from,
        html: stale.then(|| views::detail::messages(&rendered[from..]).into_string()),
        pending: view
            .pending
            .as_deref()
            .map(|content| views::detail::pending(content).into_string()),
        stop: stop_mode(&view, resp.lock, query.client.as_deref()),
        boot: state.boot.clone(),
        running,
        settled,
        error: view.error,
    }))
}

/// What stopping the running turn would take, for the client that is asking.
///
/// Three cases, because "this server can reach it" and "you started it" are not
/// the same question once more than one browser is connected.
fn stop_mode(view: &TurnView, lock: LockState, asker: Option<&str>) -> StopMode {
    if !(view.running || lock.is_here()) {
        return if lock.is_held() {
            StopMode::Unreachable
        } else {
            StopMode::None
        };
    }

    // Unattributed turns count as shared: a turn started before this page knew
    // its own identity is not one it can claim.
    match (view.client.as_deref(), asker) {
        (Some(owner), Some(asker)) if owner == asker => StopMode::Own,
        _ => StopMode::Shared,
    }
}

/// Read a conversation's turn state, consuming what should only be seen once.
///
/// A failure is reported once: leaving it in place would have every later poll
/// re-raise an error the reader has already seen.
/// The provisional copy of a submitted message is dropped once `rendered`
/// carries the request.
fn take_turn_status(state: &AppState, id: &str, rendered: &[render::RenderedEvent]) -> TurnView {
    let mut turns = state.turns.lock().expect("turns lock poisoned");

    match turns.get_mut(id) {
        Some(TurnStatus::Running {
            pending,
            client,
            sent_at,
        }) => {
            // Counted rather than read off the end of the transcript. A fast
            // first flush can persist the request and an answer to it between
            // two polls, and a transcript ending in assistant output says
            // nothing about whether the request below it is the one submitted
            // here — which left the provisional copy up beside the real one for
            // the rest of the turn.
            //
            // A submission that named no count — a plain form post, or a page
            // from an older build — has only the end of the transcript to go
            // on.
            let landed = sent_at.map_or_else(
                || render::awaiting_response(rendered),
                |sent| rendered.len() > sent,
            );

            if landed {
                pending.take();
            }

            TurnView {
                running: true,
                error: None,
                pending: pending.clone(),
                client: client.clone(),
            }
        }
        Some(TurnStatus::Failed(_)) => {
            let error = match turns.remove(id) {
                Some(TurnStatus::Failed(message)) => Some(message),
                _ => None,
            };

            TurnView {
                running: false,
                error,
                pending: None,
                client: None,
            }
        }
        None => TurnView {
            running: false,
            error: None,
            pending: None,
            client: None,
        },
    }
}

/// A conversation's turn state, leaving everything where it is.
///
/// For a read that is not the live view.
/// A failure is delivered once, so it has to be taken by the poll that drives
/// the indicator and not by whatever else the page happens to be asking for at
/// the time.
fn peek_turn_status(state: &AppState, id: &str) -> TurnView {
    let turns = state.turns.lock().expect("turns lock poisoned");

    match turns.get(id) {
        Some(TurnStatus::Running { client, .. }) => TurnView {
            running: true,
            error: None,
            pending: None,
            client: client.clone(),
        },
        Some(TurnStatus::Failed(_)) | None => TurnView {
            running: false,
            error: None,
            pending: None,
            client: None,
        },
    }
}

/// A page whose content changes while it is open, marked as never reusable.
///
/// Without this a browser is free to show the copy it already has — on a
/// reload, on a back navigation, or when restoring a backgrounded tab — and a
/// transcript from ten minutes ago looks like a transcript from now.
/// The poll would correct it within a second or three, which is long enough to
/// read as broken.
fn uncached(markup: Markup) -> Response {
    use axum::http::HeaderValue;

    let mut response = markup.into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));

    response
}

async fn conversation_detail(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    debug!(%id, "GET /conversations/{{id}}");

    let resp = read_conversation(&state, &id).await?;
    let title = resp.title.clone().unwrap_or_else(|| "Untitled".into());

    // Read without consuming: the poll that follows within a couple of seconds
    // is what clears a failure, and it drives the same indicator.
    let started_here = matches!(
        state.turns.lock().expect("turns lock poisoned").get(&id),
        Some(TurnStatus::Running { .. })
    );

    let rendered = render::render_events(&resp.data);
    let running = started_here || resp.lock.is_held();

    // Only the tail is rendered into the page. A long conversation is thousands of
    // nodes, and painting them all is what made scrolling crawl; the page asks for
    // the rest as it scrolls back.
    let first = rendered.len().saturating_sub(WINDOW);

    // Which client is asking is a browser-side fact, so the first paint can only
    // say whether this server could stop it at all. The poll a second later knows
    // the asker and refines `own` from `shared` — invisibly, since both render the
    // same button.
    let stoppable = started_here || resp.lock.is_here();

    debug!(%id, events = rendered.len(), running, "Rendered conversation detail");
    Ok(uncached(views::detail::render(
        &id,
        &title,
        &rendered[first..],
        first,
        rendered.len(),
        provisional_from(&rendered, running),
        running,
        stoppable,
    )))
}

/// Read one conversation's events, mapping a missing one to a 404.
async fn read_conversation(
    state: &AppState,
    id: &str,
) -> Result<jp_plugin::message::EventsResponse, AppError> {
    state.client.read_events(id).await.map_err(|e| match e {
        // The host reports a missing conversation as an error response; other
        // variants are server-side failures.
        ClientError::Host(msg) => {
            debug!(%id, %msg, "conversation not found");
            AppError::NotFound
        }
        e => AppError::Internal(e.to_string()),
    })
}

async fn serve_icon() -> impl IntoResponse {
    static_asset("image/svg+xml", style::ICON)
}

async fn serve_manifest() -> impl IntoResponse {
    static_asset("application/manifest+json", style::MANIFEST)
}

/// A small embedded asset, cached for a day.
///
/// Shorter than the stylesheet's year: these URLs carry no content hash, so a
/// changed icon has to be able to reach a browser that has seen the old one.
fn static_asset(content_type: &'static str, body: &'static str) -> impl IntoResponse {
    use axum::http::HeaderValue;

    let mut headers = axum::http::HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=86400"),
    );

    (StatusCode::OK, headers, body)
}

async fn serve_css() -> impl IntoResponse {
    use axum::http::HeaderValue;

    debug!("GET /assets/style.css");

    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/css; charset=utf-8"),
    );
    // Safe to pin for a year: the URL carries `?v=<content hash>`, so a changed
    // stylesheet is a changed URL.
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    if let Ok(val) = HeaderValue::from_str(&style::css_etag()) {
        headers.insert(header::ETAG, val);
    }

    (StatusCode::OK, headers, style::CSS)
}

enum AppError {
    NotFound,
    Internal(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            Self::NotFound => {
                let body = views::layout::error_page("Not Found", "Conversation not found.");
                (StatusCode::NOT_FOUND, body).into_response()
            }
            Self::Internal(msg) => {
                error!(%msg, "internal server error");
                let body = views::layout::error_page("Server Error", "Something went wrong.");
                (StatusCode::INTERNAL_SERVER_ERROR, body).into_response()
            }
        }
    }
}

#[cfg(test)]
#[path = "routes_tests.rs"]
mod tests;
