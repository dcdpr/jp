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
    openrouter::{ChatMessage, Client, Role},
    workspace::message::Message,
};

pub async fn process_question(
    client: &Client,
    ctx: &Context,
    question: &str,
    reason_preference: Reasoning,
) -> Result<()> {
    // Build a context from the history
    let mut messages = Vec::new();

    // System message explaining the context
    messages.push(ChatMessage {
        role: Role::System,
        content: "You are an AI assistant in an ongoing conversation. Below is the conversation history, followed by the user's new question. Respond to the new question taking the conversation history into account.".to_string(),
    });

    // Add history as messages
    for msg in ctx.workspace.iter().flat_map(|w| &w.messages).rev() {
        // Add user's question
        messages.push(ChatMessage {
            role: Role::User,
            content: msg.query.clone(),
        });

        // Add assistant's response
        messages.push(ChatMessage {
            role: Role::Assistant,
            content: msg.response.clone(),
        });
    }

    // Add the new question
    messages.push(ChatMessage {
        role: Role::User,
        content: question.to_string(),
    });

    let reasoning = if !matches!(reason_preference, Reasoning::Disable) {
        let is_progress_mode = matches!(reason_preference, Reasoning::Progress);
        let reasoning_complete = Arc::new(AtomicBool::new(false));
        let reasoning_complete_clone = reasoning_complete.clone();

        // For progress mode, spawn a separate task to show elapsed time
        let progress_handle = if is_progress_mode {
            let start_time = Instant::now();

            // Spawn a background task that updates the timer every second
            Some(tokio::spawn(async move {
                while !reasoning_complete_clone.load(Ordering::Relaxed) {
                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                    if reasoning_complete_clone.load(Ordering::Relaxed) {
                        break;
                    }

                    let elapsed = start_time.elapsed();
                    let elapsed_secs = elapsed.as_secs();
                    print!(
                        "\rreasoning model running ({}:{:02}), this can take several minutes...",
                        elapsed_secs / 60,
                        elapsed_secs % 60
                    );
                    stdout().flush().unwrap_or(());
                }
            }))
        } else {
            None
        };

        let request = client.request(
            &ctx.config.llm.reasoning,
            messages.clone(),
            !matches!(reason_preference, Reasoning::Hide),
        );

        let chunks: Vec<_> = if !matches!(reason_preference, Reasoning::Hide) {
            request
                .stream(|_, delta| async move {
                    if !matches!(reason_preference, Reasoning::Show) {
                        return Ok(());
                    }

                    if let Some(txt) = &delta.reasoning {
                        print!("{txt}");
                        stdout().flush()?;
                    }

                    Ok(())
                })
                .await?
                .into_iter()
                .filter_map(|delta| delta.reasoning)
                .chain(std::iter::once("\n".to_string()))
                .collect()
        } else {
            request
                .send()
                .await?
                .choices
                .into_iter()
                .filter_map(|choice| choice.message.reasoning)
                .collect()
        };

        // Signal that reasoning is complete and print a newline
        reasoning_complete.store(true, Ordering::Relaxed);

        // Wait for the progress task to finish if it exists
        if let Some(handle) = progress_handle {
            // We don't need to wait for it, but it's good practice
            handle.await.unwrap_or(());

            // Print a newline at the end for cleaner output
            print!(
                "{}{}",
                termion::clear::CurrentLine,
                termion::cursor::Left(64)
            );
        }

        let reasoning = (!chunks.is_empty()).then(|| chunks.join(""));

        if let Some(content) = reasoning.clone() {
            messages.push(ChatMessage {
                role: Role::Assistant,
                content,
            });
        }

        reasoning
    } else {
        None
    };

    // Create response with the main LLM
    let response = client
        .request(&ctx.config.llm.chat, messages, true)
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
        Message::new(question, reasoning, response)
            .save(&workspace.root, &workspace.active_session)?;
    }

    Ok(())
}
