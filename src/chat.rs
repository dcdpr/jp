use anyhow::Result;
use axum::response::sse::Event;
use log::{error, info, warn};
use std::io::Write;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::config::Config;
use crate::openrouter::{ChatMessage, Client, Role};
use crate::reasoning;

// Stream responses to stdout in CLI mode
pub async fn stdout(client: &Client, config: &Config, question: &str) -> Result<()> {
    let messages = vec![ChatMessage {
        role: Role::User,
        content: question.to_owned(),
    }];

    let request = client.request(
        &config.llm.chat,
        messages,
        true, // Stream mode
    );

    info!(
        "Sending streaming request to chat model: {}",
        config.llm.chat.model()
    );

    println!();

    request
        .stream(|line| async move {
            print!("{}", line);
            std::io::stdout().flush()?;

            Ok(())
        })
        .await
}

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
    config: &Config,
    question: &str,
) -> Result<impl futures_util::Stream<Item = Result<Event, axum::Error>>> {
    let (tx, rx) = mpsc::channel(100);
    let config = config.clone();
    let question = question.to_string();
    let client = client.clone();

    tokio::spawn(async move {
        let content = generate_prompt(&client, &config, &question).await;

        let messages = vec![ChatMessage {
            role: Role::User,
            content,
        }];

        let request = client.request(
            &config.llm.chat,
            messages,
            true, // Stream mode
        );

        let model = config.llm.chat.model().to_owned();

        let result = request
            .stream(|line| {
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
                                content: Some(line),
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
pub async fn http_response(client: &Client, config: &Config, question: &str) -> Result<String> {
    let content = generate_prompt(client, config, question).await;
    let messages = vec![ChatMessage {
        role: Role::User,
        content,
    }];

    let response = client
        .request(
            &config.llm.chat,
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

async fn generate_prompt(client: &Client, config: &Config, question: &str) -> String {
    match reasoning::get(client, config, question).await {
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
