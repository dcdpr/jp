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
    Running { pending: Option<String> },

    /// It failed, and nobody has been told yet.
    Failed(String),
}

/// What the page needs to know about a turn this server started.
struct TurnView {
    running: bool,
    error: Option<String>,
    pending: Option<String>,
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
#[derive(Debug, Deserialize)]
struct TurnForm {
    content: String,
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
    Form(form): Form<TurnForm>,
) -> Response {
    debug!(%id, "POST /conversations/{{id}}/turn");

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
        });

    let client = state.client.clone();
    let turns = Arc::clone(&state.turns);
    tokio::spawn(async move {
        let failure = match client.query(&id, &content).await {
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
async fn interrupt(State(state): State<AppState>, Path(id): Path<String>) -> Redirect {
    debug!(%id, "POST /conversations/{{id}}/interrupt");

    if let Err(error) = state.client.interrupt(&id) {
        error!(%id, %error, "Interrupt failed");
        state
            .turns
            .lock()
            .expect("turns lock poisoned")
            .insert(id.clone(), TurnStatus::Failed(error.to_string()));
    }

    Redirect::to(&format!("/conversations/{id}"))
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
                state
                    .turns
                    .lock()
                    .expect("turns lock poisoned")
                    .insert(id.clone(), TurnStatus::Running { pending: None });

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

    /// The rendered transcript, sent only when the caller's copy is out of
    /// date.
    ///
    /// Rendering it means running every message through markdown, so doing it
    /// on every poll spends the cost of the whole conversation once a second to
    /// produce something the page already has.
    /// The count is enough to know that.
    #[serde(skip_serializing_if = "Option::is_none")]
    html: Option<String>,

    running: bool,

    /// Whether stopping the turn from here would do anything.
    ///
    /// Only turns this server started run in the host it can reach.
    /// A turn someone started in a terminal belongs to another process, and an
    /// interrupt sent from here would land nowhere.
    stoppable: bool,

    /// This run of the server; a change means the page should reload.
    boot: String,

    /// A submitted message the transcript doesn't carry yet, rendered the same
    /// way the real request will be so the swap is invisible.
    #[serde(skip_serializing_if = "Option::is_none")]
    pending: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// What the poller already has, so the answer can leave it out.
#[derive(Debug, Deserialize)]
struct MessagesQuery {
    /// The event count the caller last rendered.
    #[serde(default)]
    count: Option<usize>,
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

    let stale = query.count != Some(rendered.len());

    Ok(Json(MessagesBody {
        count: rendered.len(),
        html: stale.then(|| views::detail::messages(&rendered).into_string()),
        pending: view
            .pending
            .as_deref()
            .map(|content| views::detail::pending(content).into_string()),
        // Either this server started a turn, or the transcript says the
        // assistant owes a reply — which also covers a turn someone started from
        // a terminal, and one that outlived a restart of this process.
        stoppable: view.running,
        boot: state.boot.clone(),
        running: view.running || landed,
        error: view.error,
    }))
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
        Some(TurnStatus::Running { pending }) => {
            if landed {
                pending.take();
            }

            TurnView {
                running: true,
                error: None,
                pending: pending.clone(),
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
            }
        }
        None => TurnView {
            running: false,
            error: None,
            pending: None,
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
    let running = started_here || render::awaiting_response(&rendered);

    debug!(%id, events = rendered.len(), running, "Rendered conversation detail");
    Ok(uncached(views::detail::render(
        &id,
        &title,
        &rendered,
        running,
        started_here,
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
