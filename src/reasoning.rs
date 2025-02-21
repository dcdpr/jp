use anyhow::Result;
use exodus_trace::{debug, info};

use crate::{
    context::Context,
    openrouter::{ChatMessage, Client, Role},
};

pub async fn get(client: &Client, ctx: &Context, question: &str) -> Result<Option<String>> {
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
        &ctx.config.llm.reasoning,
        messages,
        false, // No streaming for reasoning generation
    );

    debug!(
        "Sending request to reasoning model: {}",
        &ctx.config.llm.reasoning.model()
    );

    request.send().await.map(|response| {
        response
            .choices
            .first()
            .map(|choice| choice.message.content.clone())
    })
}
