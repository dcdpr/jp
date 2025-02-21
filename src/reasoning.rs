use std::future::{Future, Ready};

use anyhow::Result;

use crate::{
    context::Context,
    openrouter::{ChatMessage, Client, Role, StreamDelta},
    ThreadBuilder,
};

type NoopHandler = fn(usize, StreamDelta) -> Ready<Result<()>>;

pub async fn get_with_handler<F, Fut>(
    client: &Client,
    ctx: &Context,
    thread: ThreadBuilder,
    handler: Option<F>,
) -> Result<Option<ChatMessage>>
where
    F: FnMut(usize, StreamDelta) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let messages = thread
        .with_system(ctx.config.llm.reasoning.model().system_prompt().to_string())
        .with_instructions(&ctx.config.llm.reasoning.instructions)
        .build()?;

    let request = client.request(
        &ctx.config.llm.reasoning.model(),
        messages.clone(),
        handler.is_some(),
    );

    let content = if let Some(handler) = handler {
        request
            .stream(handler)
            .await?
            .into_iter()
            .filter_map(|delta| delta.reasoning)
            .collect::<Vec<_>>()
            .join("")
    } else {
        request
            .send()
            .await?
            .choices
            .into_iter()
            .filter_map(|choice| choice.message.reasoning)
            .collect::<Vec<_>>()
            .join("")
    };

    Ok((!content.is_empty()).then_some(ChatMessage {
        role: Role::Assistant,
        content,
    }))
}

// Add this function to handle the non-handler case
pub async fn get(
    client: &Client,
    ctx: &Context,
    thread: ThreadBuilder,
) -> Result<Option<ChatMessage>> {
    get_with_handler::<NoopHandler, Ready<Result<()>>>(client, ctx, thread, None).await
}
