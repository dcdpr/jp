//! Axum router and HTTP handlers.

use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex},
};

use axum::{
    Form, Json, Router,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};
use jp_plugin::message::LockState;
use maud::Markup;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tracing::{debug, error, info};

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
            "/conversations/count",
            axum::routing::get(conversation_count),
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

/// A new turn, as posted by the composer form.
///
/// Read from decoded pairs rather than through `Form`, for the same reason the
/// new-conversation form is: a set of checkboxes sharing a name posts that name
/// once per ticked box, and the urlencoded deserialiser cannot collect repeats.
#[derive(Debug, Default)]
struct TurnForm {
    content: String,
    cfg: Vec<String>,
    client: Option<String>,
}

impl TurnForm {
    fn parse(body: &str) -> Self {
        let mut form = Self::default();

        for (key, value) in form_urlencoded::parse(body.as_bytes()) {
            match key.as_ref() {
                "content" => form.content = value.into_owned(),
                "cfg" => form.cfg.push(value.into_owned()),
                // Without this the turn is recorded unattributed, and the page
                // that started it is told the turn is somebody else's.
                "client" => form.client = Some(value.into_owned()),
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

    // Sending while a turn runs is refused rather than made to interrupt it.
    //
    // Interrupting and immediately starting a second turn was tried and withdrawn:
    // it relied on a fixed delay to guess when the first turn had released the
    // conversation, and the turn that followed came back empty. Stopping and
    // sending are separate acts until the host can say when a turn has finished
    // unwinding.
    let busy = matches!(
        state.turns.lock().expect("turns lock poisoned").get(&id),
        Some(TurnStatus::Running { .. })
    );

    if busy {
        return (
            StatusCode::CONFLICT,
            Json(TurnRefused {
                error: "A turn is still running. Stop it first, then send.".to_owned(),
            }),
        )
            .into_response();
    }

    state
        .turns
        .lock()
        .expect("turns lock poisoned")
        .insert(id.clone(), TurnStatus::Running {
            pending: Some(content.clone()),
            client: form.client.clone(),
        });

    let client = state.client.clone();
    let turns = Arc::clone(&state.turns);
    let cfg = form.cfg;
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
/// Read from decoded pairs rather than through `Form`, because a set of
/// checkboxes sharing a name posts the name once per ticked box, and the
/// urlencoded deserialiser behind `Form` has no way to express "collect the
/// repeats" — it sees the second `cfg` and reports a string where a sequence
/// was expected.
#[derive(Debug, Default)]
struct NewConversationForm {
    content: String,
    title: String,
    cfg: Vec<String>,

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
            match key.as_ref() {
                "content" => form.content = value.into_owned(),
                "title" => form.title = value.into_owned(),
                "cfg" => form.cfg.push(value.into_owned()),
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
            .start_conversation(&content, title, form.cfg.clone())
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

    Ok(
        views::new::render(&configs, &content, &form.title, &form.cfg, error.as_deref())
            .into_response(),
    )
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

/// How many conversations there are.
#[derive(Debug, Serialize)]
struct ConversationCount {
    count: usize,
}

/// How many conversations there are.
///
/// Enough for a page to tell whether its copy of the list is still the whole
/// list, without asking for the list itself.
async fn conversation_count(
    State(state): State<AppState>,
) -> Result<Json<ConversationCount>, AppError> {
    state
        .client
        .list_conversations()
        .await
        .map(|list| Json(ConversationCount { count: list.len() }))
        .map_err(|e| AppError::Internal(e.to_string()))
}

/// The configurations a message can be run under.
///
/// Fetched by the page when its configuration dialog is first opened, rather
/// than rendered into every conversation, since most visits never open it.
async fn list_configs(
    State(state): State<AppState>,
) -> Result<Json<Vec<jp_plugin::message::ConfigEntry>>, AppError> {
    state
        .client
        .list_configs()
        .await
        .map(Json)
        .map_err(|e| AppError::Internal(e.to_string()))
}

/// The reply to a turn the page started in the background.
#[derive(Debug, Serialize)]
struct TurnStarted {
    /// The submitted message, rendered as it will appear in the transcript.
    pending: String,
}

/// Why a turn was not started, for the page to show and to keep the text.
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
    // A pending message is only worth showing until the transcript carries it.
    let landed = render::awaiting_response(&rendered);
    let view = take_turn_status(&state, &id, landed);

    // Walking backwards: a window of what came before what the caller holds.
    if let Some(before) = query.before {
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
            stop: stop_mode(&view, resp.lock, query.client.as_deref()),
            boot: state.boot.clone(),
            running: view.running || resp.lock.is_held(),
            error: None,
        }));
    }

    // What the caller already has, when that is a prefix of what is here. A count
    // beyond the end means the transcript was rewritten under it — compacted, or
    // edited on disk — and the only safe answer is the tail, from scratch.
    let from = query
        .count
        .filter(|&count| count <= rendered.len())
        .unwrap_or_else(|| rendered.len().saturating_sub(WINDOW))
        // Never past an event that can still change. A tool call is rendered when
        // it is requested and gains its result later, so sending only what comes
        // after it would leave the caller holding the question forever.
        .min(render::settled_upto(&rendered));

    let stale = from != rendered.len();

    Ok(Json(MessagesBody {
        count: rendered.len(),
        from,
        html: stale.then(|| views::detail::messages(&rendered[from..]).into_string()),
        pending: view
            .pending
            .as_deref()
            .map(|content| views::detail::pending(content).into_string()),
        // The lock is the authority on whether a turn is running. Inferring it
        // from a transcript ending in a request cannot tell a live turn from one
        // that failed, and got that wrong in the direction that blocks the
        // composer for a conversation nothing is working on.
        //
        // `view.running` still counts, for the moment between this server
        // starting a turn and the host taking the lock.
        stop: stop_mode(&view, resp.lock, query.client.as_deref()),
        boot: state.boot.clone(),
        running: view.running || resp.lock.is_held(),
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
/// The pending message is dropped as soon as `landed` says the transcript has
/// the request, so the page stops showing its provisional copy.
fn take_turn_status(state: &AppState, id: &str, landed: bool) -> TurnView {
    let mut turns = state.turns.lock().expect("turns lock poisoned");

    match turns.get_mut(id) {
        Some(TurnStatus::Running { pending, client }) => {
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
