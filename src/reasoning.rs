use anyhow::Result;
use log::{debug, info};

use crate::config::Config;
use crate::openrouter::{ChatMessage, Client, Role};

pub async fn get(client: &Client, config: &Config, question: &str) -> Result<Option<String>> {
    info!("Generating reasoning for: {}", question);

    let messages = vec![
        ChatMessage {
            role: Role::System,
            content: "You are a helpful AI assistant. Process each request thoughtfully and methodically.".to_string(),
        },
        ChatMessage {
            role: Role::User,
            content: question.to_string(),
        },
    ];

    let request = client.request(
        &config.llm.reasoning,
        messages,
        false, // No streaming for reasoning generation
    );

    debug!(
        "Sending request to reasoning model: {}",
        &config.llm.reasoning.model()
    );

    request.send().await.map(|response| {
        response
            .choices
            .first()
            .map(|choice| choice.message.content.clone())
    })
}
