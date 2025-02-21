use std::{
    io::{stdout, Write as _},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Instant,
};

use anyhow::Result;

use crate::{
    cmd::ask::Reasoning,
    context::Context,
    openrouter::{ChatMessage, Client, Role, StreamDelta},
    reasoning,
    workspace::{message::Message, Workspace},
};

pub async fn process_question(
    client: &Client,
    ctx: &Context,
    message: &str,
    reason_preference: Reasoning,
) -> Result<()> {
    let mut messages: Vec<_> = ctx
        .workspace
        .iter()
        .flat_map(Workspace::chat_history)
        .chain(std::iter::once(ChatMessage {
            role: Role::User,
            content: message.to_string(),
        }))
        .collect();

    let reasoning_response = (!matches!(reason_preference, Reasoning::Disable)).then_some({
        let messages = messages.clone();
        async {
            let handler = (!matches!(reason_preference, Reasoning::Hide)).then_some({
                |_, delta: StreamDelta| async move {
                    if !matches!(reason_preference, Reasoning::Show) {
                        return Ok(());
                    }

                    if let Some(txt) = &delta.reasoning {
                        print!("{txt}");
                        stdout().flush()?;
                    }

                    Ok(())
                }
            });

            reasoning::get_with_handler(client, ctx, messages, handler).await
        }
    });

    // For progress mode, spawn a separate task to show elapsed time
    let loading = matches!(reason_preference, Reasoning::Progress).then(|| {
        let done = Arc::new(AtomicBool::new(false));
        let done_ref = done.clone();
        let start_time = Instant::now();

        // Spawn a background task that updates the timer every second
        let handle = tokio::spawn(async move {
            while !done_ref.load(Ordering::Relaxed) {
                let elapsed_secs = start_time.elapsed().as_secs();
                print!(
                    "\rreasoning model running ({}:{:02}), this can take several minutes...",
                    elapsed_secs / 60,
                    elapsed_secs % 60
                );
                let _ = stdout().flush();

                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            }
        });

        (done, handle)
    });

    let reasoning = match reasoning_response {
        Some(reasoning) => reasoning.await?,
        None => None,
    };

    if let Some((done, handle)) = loading {
        done.store(true, Ordering::Relaxed);
        handle.await?;

        // Clear progress indicator
        print!(
            "{}{}",
            termion::clear::CurrentLine,
            termion::cursor::Left(64)
        );
    }

    if let Some(message) = reasoning.clone() {
        messages.push(message);
    }

    // Insert chat system message at the beginning of the conversation.
    messages.insert(0, ChatMessage {
        role: Role::System,
        content: ctx.config.llm.chat.system_prompt().to_string(),
    });

    // Create response with the main LLM
    let response = client
        .request(&ctx.config.llm.chat, messages.clone(), true)
        .stream(|_, delta| async move {
            if let Some(content) = &delta.content {
                print!("{content}");
                stdout().flush()?;
            }

            Ok(())
        })
        .await?
        .into_iter()
        .filter_map(|delta| delta.content)
        .chain(std::iter::once("\n".to_string()))
        .collect::<Vec<_>>()
        .join("");

    if let Some(workspace) = &ctx.workspace {
        Message::new(message, reasoning.map(|r| r.content), response)
            .save(&workspace.root, &workspace.active_session)?;
    }

    Ok(())
}
