use std::sync::Arc;

use anyhow::Result;
use axum::response::sse::Event;
use exodus_trace::{error, warn};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::{
    context::Context,
    openrouter::{ChatMessage, Client, Role},
    reasoning,
};

// Server streaming response format (compatible with OpenAI)
#[derive(Debug, serde::Serialize)]
pub struct ChatCompletionChunk {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChatCompletionChunkChoice>,
}

#[derive(Debug, serde::Serialize)]
pub struct ChatCompletionChunkChoice {
    pub index: usize,
    pub delta: ChatCompletionChunkDelta,
    pub finish_reason: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct ChatCompletionChunkDelta {
    pub content: Option<String>,
}

// Non-streaming response format (compatible with OpenAI)
#[derive(Debug, serde::Serialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChatCompletionChoice>,
}

#[derive(Debug, serde::Serialize)]
pub struct ChatCompletionChoice {
    pub index: usize,
    pub message: ChatCompletionMessage,
    pub finish_reason: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct ChatCompletionMessage {
    pub role: String,
    pub content: String,
}

// Get streaming response for server mode
pub async fn http_response_stream(
    client: &Client,
    ctx: Arc<Context>,
    question: &str,
) -> Result<impl futures_util::Stream<Item = Result<Event, axum::Error>>> {
    let (tx, rx) = mpsc::channel(100);
    let ctx = ctx.clone();
    let question = question.to_string();
    let client = client.clone();

    tokio::spawn(async move {
        let content = generate_prompt(&client, &ctx, &question).await;

        let messages = vec![ChatMessage {
            role: Role::User,
            content,
        }];

        let request = client.request(
            &ctx.config.llm.chat,
            messages,
            true, // Stream mode
        );

        let model = ctx.config.llm.chat.model().to_owned();

        let result = request
            .stream(|_, line| {
                let model = model.clone();
                let tx = tx.clone();
                async move {
                    // Format as OpenAI-compatible chunk
                    let chunk = ChatCompletionChunk {
                        id: format!("chatcmpl-{}", uuid::Uuid::new_v4()),
                        object: "chat.completion.chunk".to_string(),
                        created: chrono::Utc::now().timestamp() as u64,
                        model,
                        choices: vec![ChatCompletionChunkChoice {
                            index: 0,
                            delta: ChatCompletionChunkDelta {
                                content: line.content,
                            },
                            finish_reason: None,
                        }],
                    };

                    if let Ok(json) = serde_json::to_string(&chunk) {
                        let _ = tx.send(Ok(Event::default().data(json))).await;
                    }

                    Ok(())
                }
            })
            .await;

        if let Err(err) = result {
            let error_message = format!("Connection error: {}", err);
            let _ = tx.send(Ok(Event::default().data(error_message))).await;
        }
    });

    Ok(ReceiverStream::new(rx))
}

// Non-streaming completion
pub async fn http_response(client: &Client, ctx: &Context, question: &str) -> Result<String> {
    let content = generate_prompt(client, ctx, question).await;
    let messages = vec![ChatMessage {
        role: Role::User,
        content,
    }];

    let response = client
        .request(
            &ctx.config.llm.chat,
            messages,
            false, // No streaming
        )
        .send()
        .await?;

    // Extract content
    if let Some(choice) = response.choices.first() {
        Ok(choice.message.content.clone())
    } else {
        Err(anyhow::anyhow!("No response content"))
    }
}

async fn generate_prompt(client: &Client, ctx: &Context, question: &str) -> String {
    match reasoning::get(client, ctx, question).await {
        Ok(Some(reasoning)) => {
            format!(
                "{}\n\nHere is some additional context added by an AI co-worker of mine, they are an expert on this subject and should be taken seriously:\n\n{}",
                question, reasoning
            )
        }
        Ok(None) => {
            warn!("Reasoning response was empty, using original question as prompt.");
            question.to_string()
        }
        Err(err) => {
            error!("Failed to generate reasoning: {err}, using original question as prompt.");
            question.to_string()
        }
    }
}
