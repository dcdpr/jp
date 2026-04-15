use std::{str::FromStr, time::Duration};

use chrono::Utc;
use jp_config::{
    AppConfig, PartialAppConfig, ToPartial as _, model::id::PartialModelIdOrAliasConfig,
};
use jp_conversation::{
    ConversationEvent, ConversationStream,
    event::{ChatRequest, ChatResponse},
    thread::ThreadBuilder,
};
use jp_llm::{
    event::Event,
    event_builder::EventBuilder,
    provider,
    retry::{RetryConfig, collect_with_retry},
    title,
};
use jp_printer::PrinterWriter;
use jp_workspace::ConversationHandle;

use super::path::resolve_paths;
use crate::{
    cmd::{
        ConversationLoadRequest, Output,
        conversation_id::PositionalIds,
        lock::{LockOutcome, LockRequest, acquire_lock},
    },
    ctx::Ctx,
    error::Error,
};

#[derive(Debug, clap::Args)]
pub(crate) struct Edit {
    #[command(flatten)]
    target: PositionalIds<true, true>,

    /// Toggle the conversation between user and workspace-scoped.
    ///
    /// A user-scoped conversation is stored on your local machine and is not
    /// part of the workspace storage. This means, when using a VCS, user
    /// conversations are not stored in the VCS, but are otherwise identical to
    /// workspace conversations.
    #[arg(long, group = "property")]
    local: Option<Option<bool>>,

    /// Toggle pinning of the conversation.
    ///
    /// Pinned conversations are displayed prominently in listings and pickers.
    /// Without a value, toggles the current pin state.
    #[arg(long, group = "property")]
    pin: Option<Option<bool>>,

    /// Toggle or set the expiration time of the conversation.
    ///
    /// Without a value, toggles: removes expiration if set, or sets it to
    /// expire immediately (when no longer active) if unset.
    ///
    /// Accepts a duration (e.g. `1h`, `30m`) or `now` for immediate expiration.
    #[arg(long = "tmp", group = "property", conflicts_with = "no_expires_at")]
    expires_at: Option<Option<ExpirationDuration>>,

    /// Remove the expiration time of the conversation.
    #[arg(long = "no-tmp", group = "property")]
    no_expires_at: bool,

    /// Edit the title of the conversation.
    #[arg(long, group = "property", conflicts_with = "no_title")]
    title: Option<Option<String>>,

    /// Remove the title of the conversation.
    #[arg(long, group = "property")]
    no_title: bool,

    /// Open `events.json` in `$EDITOR`.
    #[arg(long, group = "file", conflicts_with = "property")]
    events: bool,

    /// Open `metadata.json` in `$EDITOR`.
    #[arg(long, group = "file", conflicts_with = "property")]
    metadata: bool,

    /// Open `base_config.json` in `$EDITOR`.
    #[arg(long, group = "file", conflicts_with = "property")]
    base_config: bool,
}

impl Edit {
    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        ConversationLoadRequest::explicit_or_session(&self.target.ids)
    }

    pub(crate) async fn run(self, ctx: &mut Ctx, handles: Vec<ConversationHandle>) -> Output {
        if self.has_property_flags() {
            return self.run_property_edit(ctx, handles).await;
        }

        self.run_open_editor(ctx, handles)
    }

    /// Whether any property mutation flag is set.
    fn has_property_flags(&self) -> bool {
        self.local.is_some()
            || self.pin.is_some()
            || self.expires_at.is_some()
            || self.no_expires_at
            || self.title.is_some()
            || self.no_title
    }

    /// Open the conversation directory or specific files in `$EDITOR`.
    fn run_open_editor(self, ctx: &mut Ctx, handles: Vec<ConversationHandle>) -> Output {
        let config = ctx.config();
        let cmd = config.editor.command().ok_or(Error::MissingEditor)?;

        let mut paths = Vec::new();
        for handle in handles {
            let id = handle.id();
            paths.extend(resolve_paths(
                &ctx.workspace,
                &id,
                self.events,
                self.metadata,
                self.base_config,
            )?);
        }

        let output = cmd
            .before_spawn(move |cmd| {
                for path in &paths {
                    cmd.arg(path.as_str());
                }
                Ok(())
            })
            .unchecked()
            .run()?;

        if !output.status.success() {
            return Err(
                Error::Editor(format!("Editor exited with error: {}", output.status)).into(),
            );
        }

        Ok(())
    }

    /// Mutate conversation properties
    async fn run_property_edit(self, ctx: &mut Ctx, handles: Vec<ConversationHandle>) -> Output {
        for handle in handles {
            let lock = match acquire_lock(LockRequest::from_ctx(handle, ctx)).await? {
                LockOutcome::Acquired(lock) => lock,
                LockOutcome::NewConversation => unreachable!("new conversation not allowed"),
                LockOutcome::ForkConversation(_) => unreachable!("fork not allowed"),
            };

            let conv = lock.into_mut();
            if let Some(user) = self.local {
                conv.update_metadata(|m| m.user = user.unwrap_or(!m.user));
            }

            if let Some(pinned) = self.pin {
                conv.update_metadata(|m| m.pinned = pinned.unwrap_or(!m.pinned));
            }

            if let Some(ref title) = self.title {
                let events = conv.events().clone();
                let title = match title {
                    Some(title) => title.clone(),
                    None => {
                        generate_titles(&ctx.config(), ctx.printer.out_writer(), events, vec![])
                            .await?
                    }
                };

                conv.update_metadata(|m| m.title = Some(title));
            } else if self.no_title {
                conv.update_metadata(|m| m.title = None);
            }

            if let Some(ephemeral) = self.expires_at {
                conv.update_metadata(|m| match ephemeral {
                    Some(dur) => m.expires_at = Some(Utc::now() + dur.0),
                    None => {
                        // Toggle: remove if set, expire now if unset.
                        if m.expires_at.is_some() {
                            m.expires_at = None;
                        } else {
                            m.expires_at = Some(Utc::now());
                        }
                    }
                });
            } else if self.no_expires_at {
                conv.update_metadata(|m| m.expires_at = None);
            }
        }

        ctx.printer.println("Conversation(s) updated.");
        Ok(())
    }
}

async fn generate_titles(
    config: &AppConfig,
    mut writer: PrinterWriter<'_>,
    mut events: ConversationStream,
    mut rejected: Vec<String>,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let count = 3;
    let model = config
        .conversation
        .title
        .generate
        .model
        .clone()
        .unwrap_or_else(|| config.assistant.model.clone());

    let model_id = model.id.resolved();

    let mut partial = PartialAppConfig::empty();
    partial.assistant.model.id = PartialModelIdOrAliasConfig::Id(model_id.to_partial());
    events.add_config_delta(partial);

    let provider = provider::get_provider(model_id.provider, &config.providers.llm)?;
    let model_details = provider.model_details(&model_id.name).await?;

    let sections = title::title_instructions(count, &rejected);
    let schema = title::title_schema(count);

    let thread = ThreadBuilder::default()
        .with_events(events.clone())
        .with_sections(sections)
        .build()?;

    let mut thread_events = thread.events.clone();
    thread_events.start_turn(ChatRequest {
        content: "Generate titles for this conversation.".into(),
        schema: Some(schema),
    });

    let query = jp_llm::query::ChatQuery {
        thread: jp_conversation::thread::Thread {
            events: thread_events,
            ..thread
        },
        tools: vec![],
        tool_choice: jp_config::assistant::tool_choice::ToolChoice::default(),
    };

    let retry_config = RetryConfig::default();
    let llm_events =
        collect_with_retry(provider.as_ref(), &model_details, query, &retry_config).await?;

    // Pipe raw streaming events through the EventBuilder so that structured
    // JSON chunks are concatenated and parsed into a proper Value (rather than
    // individual Value::String fragments).
    let mut builder = EventBuilder::new();
    let mut flushed = Vec::new();
    for event in llm_events {
        match event {
            Event::Part {
                index,
                part,
                metadata,
            } => {
                builder.handle_part(index, part, metadata);
            }
            Event::Flush { index, metadata } => {
                flushed.extend(builder.handle_flush(index, metadata));
            }
            Event::Finished(_) => flushed.extend(builder.drain()),
            Event::Patch(_) => {}
        }
    }

    let structured_data = flushed
        .into_iter()
        .filter_map(ConversationEvent::into_chat_response)
        .find_map(ChatResponse::into_structured_data);

    let titles = structured_data
        .as_ref()
        .map(title::extract_titles)
        .unwrap_or_default();

    if titles.is_empty() {
        return Err("No titles generated".into());
    }

    let mut choices = titles.clone();
    choices.extend(rejected.clone());
    choices.push("More...".to_owned());
    choices.push("Manually enter a title".to_owned());

    let result =
        inquire::Select::new("Conversation Title", choices).prompt_with_writer(&mut writer)?;

    match result.as_str() {
        "More..." => {
            rejected.extend(titles);
            Box::pin(generate_titles(config, writer, events, rejected)).await
        }
        "Manually enter a title" => {
            let title = inquire::Text::new("Title").prompt_with_writer(&mut writer)?;
            Ok(title.trim().to_owned())
        }
        choice => Ok(choice.to_owned()),
    }
}

/// Duration value for `--tmp`, supporting `now` as an alias for zero duration.
#[derive(Debug, Clone, Copy)]
struct ExpirationDuration(Duration);

impl FromStr for ExpirationDuration {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("now") {
            Ok(Self(Duration::ZERO))
        } else {
            humantime::parse_duration(s)
                .map(Self)
                .map_err(|e| e.to_string())
        }
    }
}

#[cfg(test)]
#[path = "edit_tests.rs"]
mod tests;
