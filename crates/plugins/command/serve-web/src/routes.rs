//! Axum router and HTTP handlers.

use std::{future::Future, net::SocketAddr};

use axum::{
    Router,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};
use maud::Markup;
use tokio::net::TcpListener;
use tracing::{debug, info};

use crate::{client::PluginClient, render, style, views};

/// Shared state for axum handlers.
#[derive(Clone)]
struct AppState {
    client: PluginClient,
}

/// Start the HTTP server and block until `shutdown` resolves.
pub(crate) async fn serve(
    client: PluginClient,
    addr: &str,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), String> {
    let state = AppState { client };

    let app = Router::new()
        .route("/", axum::routing::get(index))
        .route("/conversations", axum::routing::get(conversation_list))
        .route(
            "/conversations/{id}",
            axum::routing::get(conversation_detail),
        )
        .route("/assets/style.css", axum::routing::get(serve_css))
        .with_state(state);

    let socket_addr: SocketAddr = addr
        .parse()
        .map_err(|e| format!("invalid address `{addr}`: {e}"))?;

    let listener = TcpListener::bind(socket_addr)
        .await
        .map_err(|e| format!("failed to bind {addr}: {e}"))?;

    info!(%socket_addr, "Web server listening");

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

async fn conversation_detail(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Markup, AppError> {
    debug!(%id, "GET /conversations/{{id}}");

    let resp = state
        .client
        .read_events(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    // Find the title from the conversation list (protocol doesn't include it
    // in the events response). Fall back to "Untitled".
    let title = match state.client.list_conversations().await {
        Ok(convos) => convos
            .iter()
            .find(|c| c.id == id)
            .and_then(|c| c.title.clone())
            .unwrap_or_else(|| "Untitled".into()),
        Err(_) => "Untitled".into(),
    };

    let rendered = render::render_events(&resp.data);
    debug!(%id, events = rendered.len(), "Rendered conversation detail");
    Ok(views::detail::render(&title, &rendered))
}

async fn serve_css() -> impl IntoResponse {
    use axum::http::HeaderValue;

    debug!("GET /assets/style.css");

    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/css; charset=utf-8"),
    );
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
    NotFound(String),
    Internal(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            Self::NotFound(msg) => {
                let body = views::layout::error_page("Not Found", &msg);
                (StatusCode::NOT_FOUND, body).into_response()
            }
            Self::Internal(msg) => {
                tracing::error!(%msg, "internal server error");
                let body = views::layout::error_page("Server Error", "Something went wrong.");
                (StatusCode::INTERNAL_SERVER_ERROR, body).into_response()
            }
        }
    }
}
