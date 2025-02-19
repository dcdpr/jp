use anyhow::Result;
use axum::{
    extract::State,
    response::{
        sse::{Event, Sse},
        IntoResponse,
    },
    routing::post,
    Json, Router,
};
use log::info;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};

use crate::config::Config;
use crate::openrouter::Request;
use crate::{
    chat::{ChatCompletionChoice, ChatCompletionMessage, ChatCompletionResponse},
    openrouter::Client,
};

// Server state
#[derive(Clone)]
pub struct AppState {
    pub client: Client,
    pub config: Arc<Config>,
}

#[axum::debug_handler]
async fn handle_chat_completion(
    State(state): State<AppState>,
    Json(request): Json<Request>,
) -> Result<impl IntoResponse, axum::http::StatusCode> {
    // Extract user's question from the request
    let question = if let Some(last_message) = request.messages.last() {
        &last_message.content
    } else {
        return Err(axum::http::StatusCode::BAD_REQUEST);
    };

    match crate::chat::http_response(&state.client, &state.config, question).await {
        Ok(content) => {
            // Return OpenAI-compatible response
            let response = ChatCompletionResponse {
                id: format!("chatcmpl-{}", uuid::Uuid::new_v4()),
                object: "chat.completion".to_string(),
                created: chrono::Utc::now().timestamp() as u64,
                model: state.config.llm.chat.model().to_owned(),
                choices: vec![ChatCompletionChoice {
                    index: 0,
                    message: ChatCompletionMessage {
                        role: "assistant".to_string(),
                        content,
                    },
                    finish_reason: Some("stop".to_string()),
                }],
            };
            Ok(Json(response))
        }
        Err(e) => {
            // Log the actual error but return a generic error code
            log::error!("Error processing completion request: {}", e);
            Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

#[axum::debug_handler]
async fn handle_chat_completion_stream(
    State(state): State<AppState>,
    Json(request): Json<Request>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, axum::Error>>>, axum::http::StatusCode>
{
    // Extract user's question from the request
    let question = if let Some(last_message) = request.messages.last() {
        last_message.content.clone()
    } else {
        return Err(axum::http::StatusCode::BAD_REQUEST);
    };

    // Get streaming response
    match crate::chat::http_response_stream(&state.client, &state.config, &question).await {
        Ok(stream) => Ok(Sse::new(stream).keep_alive(
            axum::response::sse::KeepAlive::new()
                .interval(std::time::Duration::from_secs(15))
                .text("keep-alive"),
        )),
        Err(_) => Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub async fn start_server(config: Arc<Config>) -> Result<()> {
    let port = config.server.port;
    let address = config.server.address.clone();

    info!("Starting server on {}:{}", address, port);

    let app_state = AppState {
        client: Client::from_config(&config)?,
        config: config.clone(),
    };

    // Create CORS middleware
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    // Build our application with routes
    let app = Router::new()
        .route("/v1/chat/completions", post(handle_chat_completion))
        .route(
            "/v1/chat/completions/stream",
            post(handle_chat_completion_stream),
        )
        .layer(cors)
        .with_state(app_state);

    // Run our app
    let listener = tokio::net::TcpListener::bind(format!("{}:{}", address, port)).await?;
    info!("Server started successfully");
    axum::serve(listener, app).await?;

    Ok(())
}
