mod event;
mod response_handler;

use std::{
    collections::{BTreeMap, HashSet},
    env, fs,
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

use clap::{builder::TypedValueParser as _, ArgAction};
use event::{handle_tool_calls, StreamEventHandler};
use futures::StreamExt as _;
use jp_attachment::Attachment;
use jp_config::{
    assignment::{AssignKeyValue as _, KvAssignment},
    assistant::{instructions::InstructionsConfig, tool_choice::ToolChoice, AssistantConfig},
    fs::{expand_tilde, load_partial},
    model::parameters::{PartialCustomReasoningConfig, PartialReasoningConfig, ReasoningConfig},
    PartialAppConfig,
};
use jp_conversation::{
    message::Messages,
    thread::{Thread, ThreadBuilder},
    AssistantMessage, Conversation, ConversationId, MessagePair, UserMessage,
};
use jp_llm::{
    provider,
    query::{ChatQuery, StructuredQuery},
    tool::{tool_definitions, ToolDefinition},
    StreamEvent, ToolError,
};
use jp_task::task::TitleGeneratorTask;
use jp_term::stdout;
use jp_workspace::Workspace;
use minijinja::{Environment, UndefinedBehavior};
use response_handler::ResponseHandler;
use tracing::{debug, error, info, trace, warn};
use url::Url;

use super::{attachment::register_attachment, Output};
use crate::{
    cmd::Success,
    ctx::IntoPartialAppConfig,
    editor::{self, Editor},
    error::{Error, Result},
    load_cli_cfg_args, parser, Ctx, PATH_STRING_PREFIX,
};

type BoxedResult<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Default, clap::Args)]
pub(crate) struct Query {
    /// The query to send. If not provided, uses `$JP_EDITOR`, `$VISUAL` or
    /// `$EDITOR` to open edit the query in an editor.
    #[arg(value_parser = string_or_path)]
    query: Option<Vec<String>>,

    /// Use the query string as a Jinja2 template.
    ///
    /// You can provide values for template variables using the
    /// `template.values` config key.
    #[arg(long)]
    template: bool,

    #[arg(long, value_parser = string_or_path.try_map(json_schema))]
    schema: Option<schemars::Schema>,

    /// Replay the last message in the conversation.
    ///
    /// If a query is provided, it will be appended to the end of the previous
    /// message. If no query is provided, $EDITOR will open with the last
    /// message in the conversation.
    #[arg(long = "replay", conflicts_with = "new_conversation")]
    replay: bool,

    /// Start a new conversation without any message history.
    #[arg(short = 'n', long = "new")]
    new_conversation: bool,

    /// Store the conversation locally, outside of the workspace.
    #[arg(short = 'l', long = "local", requires = "new_conversation")]
    local: bool,

    /// Add attachment to the configuration.
    #[arg(short = 'a', long = "attachment", value_parser = |s: &str| parser::attachment_url(s))]
    attachments: Vec<Url>,

    /// Whether and how to edit the query.
    ///
    /// Setting this flag to `true`, omitting it, or using it as a boolean flag
    /// (e.g. `--edit`) will use the default editor configured elsewhere, or
    /// return an error if no editor is configured and one is required.
    ///
    /// If set to `false`, the editor will be disabled (similar to `--no-edit`),
    /// which might result in an error if the editor is required.
    ///
    /// If set to any other value, it will be used as the command to open the
    /// editor.
    #[arg(short = 'e', long = "edit", conflicts_with = "no_edit")]
    edit: Option<Option<Editor>>,

    /// Do not edit the query.
    ///
    /// See `--edit` for more details.
    #[arg(short = 'E', long = "no-edit", conflicts_with = "edit")]
    no_edit: bool,

    /// The model to use.
    #[arg(short = 'o', long = "model")]
    model: Option<String>,

    /// The model parameters to use.
    #[arg(short = 'p', long = "param", value_name = "KEY=VALUE", action = ArgAction::Append, value_parser = KvAssignment::from_str)]
    parameters: Vec<KvAssignment>,

    /// Enable reasoning.
    #[arg(short = 'r', long = "reasoning")]
    reasoning: Option<ReasoningConfig>,

    /// Disable reasoning.
    #[arg(short = 'R', long = "no-reasoning")]
    no_reasoning: bool,

    /// Do not display the reasoning content.
    ///
    /// This does not stop the assistant from generating reasoning tokens to
    /// help with its accuracy, but it does not display them in the output.
    #[arg(long = "hide-reasoning")]
    hide_reasoning: bool,

    /// Do not display tool calls.
    ///
    /// This does not stop the assistant from running tool calls, but it does
    /// not display them in the output.
    #[arg(long = "hide-tool-calls")]
    hide_tool_calls: bool,

    /// Stream the assistant's response as it is generated.
    ///
    /// This is the default behaviour for TTY sessions, but can be forced for
    /// non-TTY sessions by setting this flag.
    #[arg(short = 's', long = "stream", conflicts_with = "no_stream")]
    stream: bool,

    /// Disable streaming the assistant's response.
    ///
    /// This is the default behaviour for non-TTY sessions, or for structured
    /// responses, but can be forced by setting this flag.
    #[arg(short = 'S', long = "no-stream", conflicts_with = "stream")]
    no_stream: bool,

    /// The tool(s) to enable.
    ///
    /// If an existing tool is configured with a matching name, it will be
    /// enabled for the duration of the query.
    ///
    /// If no arguments are provided, all configured tools will be enabled.
    ///
    /// You can provide this flag multiple times to enable multiple tools. It
    /// can be combined with `--no-tools` to disable all enabled tools before
    /// enabling a specific one.
    #[arg(
        short = 't',
        long = "tool",
        action = ArgAction::Append,
        num_args = 0..=1,
        value_parser = |s: &str| -> Result<Option<String>> {
            if s.is_empty() { Ok(None) } else { Ok(Some(s.to_string())) }
        },
        default_missing_value = "",
    )]
    tools: Vec<Option<String>>,

    /// Disable tools.
    ///
    /// If provided without a value, all enabled tools will be disabled,
    /// otherwise pass the argument multiple times to disable one or more tools.
    ///
    /// Any tools that were enabled before this flag is set will be disabled.
    #[arg(
        short = 'T',
        long = "no-tools",
        action = ArgAction::Append,
        num_args = 0..=1,
        value_parser = |s: &str| -> Result<Option<String>> {
            if s.is_empty() { Ok(None) } else { Ok(Some(s.to_string())) }
        },
        default_missing_value = "",
    )]
    no_tools: Vec<Option<String>>,

    /// The tool to use.
    ///
    /// If a value is provided, the tool matching the value will be used.
    ///
    /// Note that this setting is *not* persisted across queries. To persist
    /// tool choice behavior, set the `assistant.tool_choice` field in a
    /// configuration file.
    #[arg(short = 'u', long = "tool-use")]
    tool_use: Option<Option<String>>,

    /// Disable tool use by the assistant.
    #[arg(short = 'U', long = "no-tool-use")]
    no_tool_use: bool,
}

/// How to render the response to the user.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RenderMode {
    /// Use the default render mode, depending on whether the output is a TTY,
    /// and if a structured response is requested.
    #[default]
    Auto,

    /// Render the response as a stream of tokens.
    Streamed,

    /// Render the response as a buffered string.
    Buffered,
}

impl Query {
    pub(crate) async fn run(self, ctx: &mut Ctx) -> Output {
        debug!("Running `query` command.");
        trace!(args = ?self, "Received arguments.");

        let previous_id = self.update_active_conversation(ctx)?;
        let conversation_id = ctx.workspace.active_conversation_id();

        ctx.configure_active_mcp_servers().await?;
        let (user_query, query_file) = self.build_message(ctx, &conversation_id).await?;
        let history = ctx.workspace.get_messages(&conversation_id).to_messages();

        if let UserMessage::Query(query) = &user_query {
            if query.is_empty() {
                return cleanup(ctx, previous_id, query_file.as_deref()).map_err(Into::into);
            }

            // Generate title for new or empty conversations.
            if ctx.term.args.persist
                && (self.new_conversation || history.is_empty())
                && ctx.config().conversation.title.generate.auto
            {
                debug!("Generating title for new conversation");
                ctx.task_handler.spawn(TitleGeneratorTask::new(
                    conversation_id,
                    history.clone(),
                    ctx.config(),
                    Some(query.clone()),
                )?);
            }
        }

        let tools =
            tool_definitions(ctx.config().conversation.tools.iter(), &ctx.mcp_client).await?;

        let mut attachments = vec![];
        for attachment in &ctx.config().conversation.attachments {
            register_attachment(ctx, &attachment.to_url()?, &mut attachments).await?;
        }

        let thread = build_thread(
            user_query.clone(),
            history,
            attachments,
            &ctx.config().assistant,
            &tools,
        )?;

        let mut new_messages = vec![];
        if let Some(schema) = self.schema.clone() {
            new_messages.push(handle_structured_output(ctx, thread, schema).await?);
        } else {
            self.handle_stream(
                ctx,
                thread,
                ctx.config().assistant.tool_choice.clone(),
                tools,
                &mut new_messages,
                0,
            )
            .await?;
        }

        let reply = self.store_messages(ctx, conversation_id, new_messages)?;

        // Clean up the query file.
        if let Some(path) = query_file {
            fs::remove_file(path)?;
        }

        if self.schema.is_some() && !reply.is_empty() {
            if let RenderMode::Streamed = self.render_mode() {
                stdout::typewriter(&reply, ctx.config().style.typewriter.code_delay.into())?;
            } else {
                return Ok(Success::Json(serde_json::from_str(&reply)?));
            }
        }

        Ok(Success::Ok)
    }

    async fn build_message(
        &self,
        ctx: &mut Ctx,
        conversation_id: &ConversationId,
    ) -> Result<(UserMessage, Option<PathBuf>)> {
        // If replaying, remove the last message from the conversation, and use
        // its query message to build the new query.
        let mut message = self
            .replay
            .then(|| ctx.workspace.pop_message(conversation_id))
            .flatten()
            .map_or(UserMessage::Query(String::new()), |m| m.message);

        // If replaying a tool call, re-run the requested tool(s) and return the
        // new results.
        if let UserMessage::ToolCallResults(_) = &mut message {
            let messages = ctx.workspace.get_messages(conversation_id);
            let Some(response) = messages.last() else {
                return Err(Error::Replay("No assistant response found".into()));
            };

            let results = handle_tool_calls(ctx, response.reply.tool_calls.clone()).await?;
            message = UserMessage::ToolCallResults(results);
        }

        // If a query is provided, prepend it to the existing message. This is
        // only relevant for replays, otherwise the existing message is empty,
        // and we replace it with the provided query.
        if let Some(text) = &self.query {
            let text = text.join(" ");
            match &mut message {
                UserMessage::Query(query) if query.is_empty() => text.clone_into(query),
                UserMessage::Query(query) => *query = format!("{text}\n\n{query}"),
                UserMessage::ToolCallResults(_) => {}
            }
        }

        let query_file_path = self.edit_message(&mut message, ctx, conversation_id)?;

        if let UserMessage::Query(query) = &mut message
            && self.template
        {
            let mut env = Environment::empty();
            env.set_undefined_behavior(UndefinedBehavior::SemiStrict);
            env.add_template("query", query)?;

            let tmpl = env.get_template("query")?;
            // TODO: supported nested variables
            for var in tmpl.undeclared_variables(false) {
                if ctx.config().template.values.contains_key(&var) {
                    continue;
                }

                return Err(Error::TemplateUndefinedVariable(var));
            }

            *query = tmpl.render(&ctx.config().template.values)?;
        }

        Ok((message, query_file_path))
    }

    fn update_active_conversation(&self, ctx: &mut Ctx) -> Result<ConversationId> {
        // Store the (old) active conversation ID, so that we can restore to it,
        // if the current conversation is aborted early (e.g. because of an
        // empty query or any other error).
        let last_active_conversation_id = ctx.workspace.active_conversation_id();

        // Set new active conversation if requested.
        if self.new_conversation {
            let id = ctx
                .workspace
                .create_conversation(Conversation::default().with_local(self.local));

            debug!(
                %id,
                local = %self.local,
                "Creating new active conversation due to --new flag."
            );

            ctx.workspace.set_active_conversation_id(id)?;
        }

        Ok(last_active_conversation_id)
    }

    // Open the editor for the query, if requested.
    fn edit_message(
        &self,
        message: &mut UserMessage,
        ctx: &mut Ctx,
        conversation_id: &ConversationId,
    ) -> Result<Option<PathBuf>> {
        // Editing only applies to queries, not tool-call results.
        let UserMessage::Query(query) = message else {
            return Ok(None);
        };

        // If there is no query provided, but the user explicitly requested not
        // to edit the query, we populate the query with a default message,
        // since most LLM providers do not support empty queries.
        if query.is_empty() && self.force_no_edit() {
            "<no content>".clone_into(query);
        }

        // If a query is provided, and editing is not explicitly requested, we
        // omit opening the editor.
        if !query.is_empty() && !self.force_edit() {
            return Ok(None);
        }

        let cmd = ctx.config().editor.command();
        let editor = match cmd {
            None if !query.is_empty() => return Ok(None),
            None => return Err(Error::MissingEditor),
            Some(cmd) => cmd,
        };

        let initial_message = if query.is_empty() {
            None
        } else {
            Some(query.to_owned())
        };

        // If replaying, pass the last query as the text to be edited,
        // otherwise open an empty editor.
        let query_file_path;
        (*query, query_file_path) =
            editor::edit_query(ctx, conversation_id, initial_message, editor)
                .map(|(q, p)| (q, Some(p)))?;

        Ok(query_file_path)
    }

    #[expect(clippy::too_many_lines)]
    async fn handle_stream(
        &self,
        ctx: &mut Ctx,
        mut thread: Thread,
        tool_choice: ToolChoice,
        tools: Vec<ToolDefinition>,
        messages: &mut Vec<MessagePair>,
        mut tries: usize,
    ) -> Result<()> {
        tries += 1;

        let model_id = &ctx
            .config()
            .assistant
            .model
            .id
            .finalize(&ctx.config().providers.llm.aliases)?;

        let parameters = &ctx.config().assistant.model.parameters;
        let provider = provider::get_provider(model_id.provider, &ctx.config().providers.llm)?;
        let message = thread.message.clone();
        let query = ChatQuery {
            thread: thread.clone(),

            // Limit the tools to the ones that are relevant to the tool choice.
            tools: match &tool_choice {
                ToolChoice::None => vec![],
                ToolChoice::Auto | ToolChoice::Required => tools.clone(),
                ToolChoice::Function(name) => tools
                    .clone()
                    .into_iter()
                    .filter(|v| &v.name == name)
                    .collect(),
            },
            tool_choice: tool_choice.clone(),
            ..Default::default()
        };
        let model = provider.model_details(&model_id.name).await?;
        let mut stream = provider
            .chat_completion_stream(&model, parameters, query)
            .await?;

        let mut event_handler = StreamEventHandler {
            content_tokens: String::new(),
            reasoning_tokens: String::new(),
            tool_calls: vec![],
            tool_call_results: vec![],
        };

        let mut printer =
            ResponseHandler::new(self.render_mode(), ctx.config().style.tool_call.show);
        let mut metadata = BTreeMap::new();

        while let Some(event) = stream.next().await {
            let event = match event {
                Err(jp_llm::Error::RateLimit { retry_after }) => {
                    let max_tries = 5;
                    if tries > max_tries {
                        error!(tries, "Failed to get a non-rate-limited response.");
                        return Err(Error::Llm(jp_llm::Error::RateLimit { retry_after: None }));
                    }

                    let retry_after = retry_after.unwrap_or(Duration::from_secs(2));
                    warn!(
                        retry_after_secs = retry_after.as_secs(),
                        tries, max_tries, "Rate limited, retrying..."
                    );
                    tokio::time::sleep(retry_after).await;
                    return Box::pin(self.handle_stream(
                        ctx,
                        thread,
                        tool_choice,
                        tools,
                        messages,
                        tries,
                    ))
                    .await;
                }
                Err(jp_llm::Error::UnknownModel(model)) => {
                    let available = provider
                        .models()
                        .await?
                        .into_iter()
                        .map(|v| v.id.name.to_string())
                        .collect();

                    return Err(Error::UnknownModel { model, available });
                }
                Err(e) => {
                    return Err(e.into());
                }
                Ok(event) => event,
            };

            let data = match event {
                StreamEvent::ChatChunk(chunk) => event_handler.handle_chat_chunk(ctx, chunk),
                StreamEvent::ToolCall(call) => {
                    event_handler
                        .handle_tool_call(ctx, call, &mut printer)
                        .await?
                }
                StreamEvent::Metadata(key, data) => {
                    metadata.insert(key, data);
                    continue;
                }
            };

            let Some(data) = data else {
                continue;
            };

            printer.handle(&data, ctx, false)?;
        }

        // Ensure we handle the last line of the stream.
        if !printer.buffer.is_empty() {
            printer.handle("\n", ctx, false)?;
        }

        let content_tokens = event_handler.content_tokens.trim().to_string();
        let content = if !content_tokens.is_empty() {
            Some(content_tokens)
        } else if content_tokens.is_empty() && event_handler.tool_calls.is_empty() {
            let max_tries = 3;
            if tries <= max_tries {
                warn!(tries, max_tries, "Empty response received, retrying...");

                return Box::pin(self.handle_stream(
                    ctx,
                    thread,
                    tool_choice,
                    tools,
                    messages,
                    tries,
                ))
                .await;
            }

            error!(tries, "Failed to get a non-empty response.");
            Some("<no reply>".to_string())
        } else {
            None
        };

        let reasoning_tokens = event_handler.reasoning_tokens.trim().to_string();
        let reasoning = if reasoning_tokens.is_empty() {
            None
        } else {
            Some(reasoning_tokens)
        };

        if let RenderMode::Buffered = printer.render_mode {
            println!("{}", printer.parsed.join("\n"));
        } else if content.is_some() || reasoning.is_some() {
            // Final newline.
            println!();
        }

        let message = MessagePair::new(message, AssistantMessage {
            provider: model_id.provider,
            metadata,
            content,
            reasoning,
            tool_calls: event_handler.tool_calls.clone(),
        });
        messages.push(message.clone());

        // If the assistant asked for a tool call, we handle it within the same
        // "conversation turn", essentially going into a "loop" until no more
        // tool calls are requested.
        if !event_handler.tool_call_results.is_empty() {
            thread.history.push(message, None);
            thread.message = UserMessage::ToolCallResults(event_handler.tool_call_results);

            Box::pin(self.handle_stream(
                ctx,
                thread,
                // After the first tool call, we revert back to letting the LLM
                // decide if/which tool to use.
                ToolChoice::Auto,
                tools,
                messages,
                0,
            ))
            .await?;
        }

        Ok(())
    }

    fn render_mode(&self) -> RenderMode {
        if self.no_stream {
            return RenderMode::Buffered;
        } else if self.stream {
            return RenderMode::Streamed;
        }

        RenderMode::Auto
    }

    /// Returns `true` if editing is explicitly disabled.
    ///
    /// This signals that even if no query is provided, no editor should be
    /// opened, but instead an empty query should be used.
    ///
    /// This can be used for example when requesting a tool call without needing
    /// additional context to be provided.
    fn force_no_edit(&self) -> bool {
        self.no_edit || matches!(self.edit, Some(Some(Editor::Disabled)))
    }

    /// Returns `true` if editing is explicitly enabled.
    ///
    /// This means the `--edit` flag was provided (but not `--edit=false`),
    /// which means the editor should be opened, regardless of whether a query
    /// is provided as an argument.
    fn force_edit(&self) -> bool {
        !self.force_no_edit() && self.edit.is_some()
    }

    fn store_messages(
        &self,
        ctx: &mut Ctx,
        conversation_id: ConversationId,
        new_messages: Vec<MessagePair>,
    ) -> Result<String> {
        let mut reply = String::new();

        for message in new_messages {
            debug!(
                conversation = %conversation_id,
                content_size_bytes = message.reply.content.as_deref().unwrap_or_default().len(),
                reasoning_size_bytes = message.reply.reasoning.as_deref().unwrap_or_default().len(),
                tool_calls_count = message.reply.tool_calls.len(),
                "Storing response message in conversation."
            );

            if let Some(content) = &message.reply.content {
                reply.push_str(content);
            }
            ctx.workspace.add_message(
                conversation_id,
                message,
                if self.new_conversation {
                    Some(ctx.config().to_partial())
                } else {
                    let global = ctx.term.args.config.clone();
                    let partial = load_cli_cfg_args(
                        PartialAppConfig::empty(),
                        &global,
                        Some(&ctx.workspace),
                    )?;

                    let partial_config = ctx.config().to_partial();
                    let partial = IntoPartialAppConfig::apply_cli_config(
                        self,
                        None,
                        partial,
                        Some(&partial_config),
                    )
                    .map_err(|error| Error::CliConfig(error.to_string()))?;

                    Some(partial)
                },
            );
        }

        Ok(reply)
    }
}

impl IntoPartialAppConfig for Query {
    fn apply_cli_config(
        &self,
        _workspace: Option<&Workspace>,
        mut partial: PartialAppConfig,
        merged_config: Option<&PartialAppConfig>,
    ) -> std::result::Result<PartialAppConfig, Box<dyn std::error::Error + Send + Sync>> {
        let Self {
            model,
            template: _,
            schema: _,
            replay: _,
            new_conversation: _,
            local: _,
            attachments,
            edit,
            no_edit,
            tool_use,
            no_tool_use,
            query: _,
            parameters,
            hide_reasoning,
            hide_tool_calls,
            stream: _,
            no_stream: _,
            tools: raw_tools,
            no_tools: raw_no_tools,
            reasoning,
            no_reasoning,
        } = &self;

        apply_model(&mut partial, model.as_deref(), merged_config);
        apply_editor(&mut partial, edit.as_ref().map(|v| v.as_ref()), *no_edit);
        apply_enable_tools(&mut partial, raw_tools, raw_no_tools, merged_config)?;
        apply_tool_use(
            &mut partial,
            tool_use.as_ref().map(|v| v.as_deref()),
            *no_tool_use,
        )?;
        apply_attachments(&mut partial, attachments);
        apply_reasoning(&mut partial, reasoning.as_ref(), *no_reasoning);

        for kv in parameters.clone() {
            partial.assistant.model.parameters.assign(kv)?;
        }

        if *hide_reasoning {
            partial.style.reasoning.show = Some(false);
        }

        if *hide_tool_calls {
            partial.style.tool_call.show = Some(false);
        }

        Ok(partial)
    }

    fn apply_conversation_config(
        &self,
        workspace: Option<&Workspace>,
        partial: PartialAppConfig,
        _: Option<&PartialAppConfig>,
    ) -> std::result::Result<PartialAppConfig, Box<dyn std::error::Error + Send + Sync>> {
        // New conversations do not apply any existing conversation
        // configurations. This is handled by the other configuration layers
        // (files, environment variables, CLI arguments).
        if self.new_conversation {
            return Ok(partial);
        }

        // If we're not inside a workspace, there is no active conversation to
        // fetch the configuration from.
        let Some(workspace) = workspace else {
            return Ok(partial);
        };

        let id = workspace.active_conversation_id();

        load_partial(partial, workspace.get_messages(&id).config()).map_err(Into::into)
    }
}

fn build_thread(
    user_message: UserMessage,
    history: Messages,
    attachments: Vec<Attachment>,
    assistant: &AssistantConfig,
    tools: &[ToolDefinition],
) -> Result<Thread> {
    let mut thread_builder = ThreadBuilder::default()
        .with_system_prompt(assistant.system_prompt.clone())
        .with_instructions(assistant.instructions.clone())
        .with_attachments(attachments)
        .with_history(history)
        .with_message(user_message);

    if !tools.is_empty() {
        let instruction = InstructionsConfig::default()
            .with_title("Tool Usage")
            .with_description("How to leverage the tools available to you.".to_string())
            .with_item("Use all the tools available to you to give the best possible answer.")
            .with_item("Verify the tool name, description and parameters are correct.")
            .with_item(
                "Even if you've reasoned yourself towards a solution, use any available tool to \
                 verify your answer.",
            );

        thread_builder = thread_builder.with_instruction(instruction);
    }

    Ok(thread_builder.build()?)
}

/// Apply the CLI model configuration to the partial configuration.
fn apply_model(partial: &mut PartialAppConfig, model: Option<&str>, _: Option<&PartialAppConfig>) {
    let Some(id) = model else { return };

    partial.assistant.model.id = id.into();
}

/// Apply the CLI editor configuration to the partial configuration.
fn apply_editor(partial: &mut PartialAppConfig, editor: Option<Option<&Editor>>, no_edit: bool) {
    let Some(Some(editor)) = editor else {
        return;
    };

    match (no_edit, editor) {
        (true, _) | (_, Editor::Disabled) => {
            partial.editor.cmd = None;
            partial.editor.envs = None;
        }
        (_, Editor::Default) => {}
        (_, Editor::Command(cmd)) => partial.editor.cmd = Some(cmd.clone()),
    }
}

fn apply_enable_tools(
    partial: &mut PartialAppConfig,
    raw_tools: &[Option<String>],
    raw_no_tools: &[Option<String>],
    merged_config: Option<&PartialAppConfig>,
) -> BoxedResult<()> {
    let tools = if raw_tools.is_empty() {
        None
    } else if raw_tools.iter().any(Option::is_none) {
        Some(vec![])
    } else {
        Some(raw_tools.iter().filter_map(|v| v.as_deref()).collect())
    };

    let no_tools = if raw_no_tools.is_empty() {
        None
    } else if raw_no_tools.iter().any(Option::is_none) {
        Some(vec![])
    } else {
        Some(raw_no_tools.iter().filter_map(|v| v.as_deref()).collect())
    };

    let enable_all = tools.as_ref().is_some_and(Vec::is_empty);
    let disable_all = no_tools.as_ref().is_some_and(Vec::is_empty);

    if enable_all && disable_all {
        return Err("cannot pass both --no-tools and --tools without arguments".into());
    }

    let existing_tools = merged_config.map_or(&partial.conversation.tools.tools, |v| {
        &v.conversation.tools.tools
    });

    let missing = tools
        .iter()
        .flatten()
        .chain(no_tools.iter().flatten())
        .filter(|name| !existing_tools.contains_key(**name))
        .collect::<HashSet<_>>();

    if missing.len() == 1 {
        return Err(ToolError::NotFound {
            name: missing.iter().next().unwrap().to_string(),
        }
        .into());
    } else if !missing.is_empty() {
        return Err(ToolError::NotFoundN {
            names: missing.into_iter().map(ToString::to_string).collect(),
        }
        .into());
    }

    // Disable tools.
    if let Some(no_tools) = no_tools {
        partial
            .conversation
            .tools
            .tools
            .iter_mut()
            .filter(|(name, _)| disable_all || no_tools.iter().any(|v| v == name))
            .for_each(|(_, v)| v.enable = Some(false));
    }

    // Enable tools.
    if let Some(tools) = tools {
        partial
            .conversation
            .tools
            .tools
            .iter_mut()
            .filter(|(name, _)| enable_all || tools.iter().any(|v| v == *name))
            .for_each(|(_, v)| v.enable = Some(true));
    }

    Ok(())
}

/// Apply the CLI tool use configuration to the partial configuration.
///
/// NOTE: This has to run *after* `apply_enable_tools` because it will return an
/// error if the tool of choice is not enabled.
fn apply_tool_use(
    partial: &mut PartialAppConfig,
    tool_choice: Option<Option<&str>>,
    no_tool_choice: bool,
) -> BoxedResult<()> {
    if no_tool_choice || matches!(tool_choice, Some(Some("false"))) {
        partial.assistant.tool_choice = Some(ToolChoice::None);
        return Ok(());
    }

    let Some(tool) = tool_choice else {
        return Ok(());
    };

    partial.assistant.tool_choice = match tool {
        None | Some("true") => Some(ToolChoice::Required),
        Some(v) => {
            if !partial
                .conversation
                .tools
                .tools
                .iter()
                .filter(|(_, cfg)| cfg.enable.is_some_and(|v| v))
                .any(|(name, _)| name == v)
            {
                return Err(format!("tool choice '{v}' does not match any enabled tools").into());
            }

            Some(ToolChoice::Function(v.to_owned()))
        }
    };

    Ok(())
}

/// Apply the CLI attachments to the partial configuration.
fn apply_attachments(partial: &mut PartialAppConfig, attachments: &[Url]) {
    if attachments.is_empty() {
        return;
    }

    partial
        .conversation
        .attachments
        .extend(attachments.iter().cloned().map(Into::into));
}

/// Apply the CLI reasoning configuration to the partial configuration.
fn apply_reasoning(
    partial: &mut PartialAppConfig,
    reasoning: Option<&ReasoningConfig>,
    no_reasoning: bool,
) {
    if no_reasoning {
        partial.assistant.model.parameters.reasoning = Some(PartialReasoningConfig::Off);
        return;
    }

    let Some(reasoning) = reasoning else {
        return;
    };

    partial.assistant.model.parameters.reasoning = Some(match reasoning {
        ReasoningConfig::Off => PartialReasoningConfig::Off,
        ReasoningConfig::Auto => PartialReasoningConfig::Auto,
        ReasoningConfig::Custom(custom) => PartialCustomReasoningConfig {
            effort: Some(custom.effort),
            exclude: Some(custom.exclude),
        }
        .into(),
    });
}

/// Clean up empty queries.
fn cleanup(
    ctx: &mut Ctx,
    last_active_conversation_id: ConversationId,
    query_file_path: Option<&Path>,
) -> Result<Success> {
    let conversation_id = ctx.workspace.active_conversation_id();

    info!("Query is empty, exiting.");
    if last_active_conversation_id != conversation_id {
        ctx.workspace
            .set_active_conversation_id(last_active_conversation_id)?;
        ctx.workspace.remove_conversation(&conversation_id)?;
    }

    if let Some(path) = query_file_path {
        fs::remove_file(path)?;
    }

    Ok("Query is empty, ignoring.".into())
}

async fn handle_structured_output(
    ctx: &mut Ctx,
    thread: Thread,
    schema: schemars::Schema,
) -> Result<MessagePair> {
    let model_id = &ctx
        .config()
        .assistant
        .model
        .id
        .finalize(&ctx.config().providers.llm.aliases)?;

    let parameters = &ctx.config().assistant.model.parameters;
    let provider = provider::get_provider(model_id.provider, &ctx.config().providers.llm)?;
    let message = thread.message.clone();
    let query = StructuredQuery::new(schema, thread);

    let model = provider.model_details(&model_id.name).await?;
    let value = provider
        .structured_completion(&model, parameters, query)
        .await?;
    let content = if ctx.term.is_tty {
        serde_json::to_string_pretty(&value)?
    } else {
        serde_json::to_string(&value)?
    };

    Ok(MessagePair::new(
        message,
        AssistantMessage::from((model_id.provider, content)),
    ))
}

#[expect(clippy::needless_pass_by_value)]
fn json_schema(s: String) -> Result<schemars::Schema> {
    serde_json::from_str::<serde_json::Value>(&s)?
        .try_into()
        .map_err(Into::into)
}

fn string_or_path(s: &str) -> Result<String> {
    if let Some(s) = s
        .strip_prefix(PATH_STRING_PREFIX)
        .and_then(|s| expand_tilde(s, env::var("HOME").ok()))
    {
        return fs::read_to_string(s).map_err(Into::into);
    }

    Ok(s.to_owned())
}

struct Line {
    content: String,
    variant: LineVariant,
}

#[derive(Debug)]
enum LineVariant {
    Normal,
    Code,
    Raw,
    FencedCodeBlockStart { language: Option<String> },
    FencedCodeBlockEnd { indent: usize },
}

impl Line {
    fn new(content: String, in_fenced_code_block: bool, raw: bool) -> Self {
        let variant = if raw {
            LineVariant::Raw
        } else if in_fenced_code_block && content.trim().ends_with("```") {
            let indent = content.chars().take_while(|c| c.is_whitespace()).count();

            LineVariant::FencedCodeBlockEnd { indent }
        } else if content.trim_start().starts_with("```") {
            let language = content
                .trim_start()
                .chars()
                .skip(3)
                .take_while(|c| c.is_alphanumeric())
                .collect::<String>();
            let language = if language.is_empty() {
                None
            } else {
                Some(language)
            };

            LineVariant::FencedCodeBlockStart { language }
        } else if in_fenced_code_block {
            LineVariant::Code
        } else {
            LineVariant::Normal
        };

        Line { content, variant }
    }
}

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;
    use jp_config::conversation::tool::PartialToolConfig;

    use super::*;

    #[test]
    #[expect(clippy::too_many_lines)]
    fn test_query_tools_and_no_tools() {
        // Create a partial configuration with a few tools.
        let mut partial = PartialAppConfig::default();
        partial.conversation.tools.tools = IndexMap::from_iter([
            ("implicitly_enabled_tool".into(), PartialToolConfig {
                enable: None,
                ..Default::default()
            }),
            ("explicitly_enabled_tool".into(), PartialToolConfig {
                enable: Some(true),
                ..Default::default()
            }),
            ("explicitly_disabled_tool".into(), PartialToolConfig {
                enable: Some(false),
                ..Default::default()
            }),
        ]);

        // Keep all tools as-is.
        partial = IntoPartialAppConfig::apply_cli_config(
            &Query {
                no_tools: vec![],
                ..Default::default()
            },
            None,
            partial,
            None,
        )
        .unwrap();

        assert_eq!(
            partial.conversation.tools.tools["implicitly_enabled_tool"].enable,
            None,
        );
        assert_eq!(
            partial.conversation.tools.tools["explicitly_enabled_tool"].enable,
            Some(true)
        );
        assert_eq!(
            partial.conversation.tools.tools["explicitly_disabled_tool"].enable,
            Some(false)
        );

        // Disable one tool.
        partial = IntoPartialAppConfig::apply_cli_config(
            &Query {
                no_tools: vec![Some("implicitly_enabled_tool".into())],
                ..Default::default()
            },
            None,
            partial,
            None,
        )
        .unwrap();

        assert_eq!(
            partial.conversation.tools.tools["implicitly_enabled_tool"].enable,
            Some(false),
        );
        assert_eq!(
            partial.conversation.tools.tools["explicitly_enabled_tool"].enable,
            Some(true)
        );
        assert_eq!(
            partial.conversation.tools.tools["explicitly_disabled_tool"].enable,
            Some(false)
        );

        // Enable one tool.
        partial = IntoPartialAppConfig::apply_cli_config(
            &Query {
                tools: vec![Some("explicitly_disabled_tool".into())],
                ..Default::default()
            },
            None,
            partial,
            None,
        )
        .unwrap();

        assert_eq!(
            partial.conversation.tools.tools["implicitly_enabled_tool"].enable,
            Some(false),
        );
        assert_eq!(
            partial.conversation.tools.tools["explicitly_enabled_tool"].enable,
            Some(true)
        );
        assert_eq!(
            partial.conversation.tools.tools["explicitly_disabled_tool"].enable,
            Some(true)
        );

        // Enable all tools.
        partial = IntoPartialAppConfig::apply_cli_config(
            &Query {
                tools: vec![None],
                ..Default::default()
            },
            None,
            partial,
            None,
        )
        .unwrap();

        assert_eq!(
            partial.conversation.tools.tools["implicitly_enabled_tool"].enable,
            Some(true),
        );
        assert_eq!(
            partial.conversation.tools.tools["explicitly_enabled_tool"].enable,
            Some(true)
        );
        assert_eq!(
            partial.conversation.tools.tools["explicitly_disabled_tool"].enable,
            Some(true)
        );

        // Disable all tools.
        partial = IntoPartialAppConfig::apply_cli_config(
            &Query {
                no_tools: vec![None],
                ..Default::default()
            },
            None,
            partial,
            None,
        )
        .unwrap();

        assert_eq!(
            partial.conversation.tools.tools["implicitly_enabled_tool"].enable,
            Some(false),
        );
        assert_eq!(
            partial.conversation.tools.tools["explicitly_enabled_tool"].enable,
            Some(false)
        );
        assert_eq!(
            partial.conversation.tools.tools["explicitly_disabled_tool"].enable,
            Some(false)
        );

        // Enable multiple tools.
        partial = IntoPartialAppConfig::apply_cli_config(
            &Query {
                tools: vec![
                    Some("explicitly_disabled_tool".into()),
                    Some("explicitly_enabled_tool".into()),
                ],
                ..Default::default()
            },
            None,
            partial,
            None,
        )
        .unwrap();

        assert_eq!(
            partial.conversation.tools.tools["implicitly_enabled_tool"].enable,
            Some(false),
        );
        assert_eq!(
            partial.conversation.tools.tools["explicitly_enabled_tool"].enable,
            Some(true)
        );
        assert_eq!(
            partial.conversation.tools.tools["explicitly_disabled_tool"].enable,
            Some(true)
        );
    }
}
