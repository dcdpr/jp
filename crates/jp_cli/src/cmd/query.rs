use std::{
    collections::{BTreeMap, HashSet},
    convert::Infallible,
    env, fs,
    path::{Path, PathBuf},
    str::FromStr,
};

use clap::{builder::TypedValueParser as _, ArgAction};
use crossterm::style::{Color, Stylize as _};
use futures::StreamExt as _;
use jp_config::{expand_tilde, style::code::LinkStyle};
use jp_conversation::{
    message::{ToolCallRequest, ToolCallResult},
    persona::Instructions,
    thread::{Thread, ThreadBuilder},
    AssistantMessage, Context, ContextId, Conversation, ConversationId, MessagePair, Model,
    ModelId, PersonaId, UserMessage,
};
use jp_llm::provider::{self, CompletionChunk, StreamEvent};
use jp_mcp::{config::McpServerId, tool::ToolChoice, ResourceContents};
use jp_query::query::{ChatQuery, StructuredQuery};
use jp_task::task::TitleGeneratorTask;
use jp_term::{code, osc::hyperlink, stdout};
use minijinja::{Environment, UndefinedBehavior};
use termimad::FmtText;
use tracing::{debug, info, trace};
use url::Url;

use super::{attachment::register_attachment, Output};
use crate::{
    cmd::Success,
    editor::{self, Editor},
    error::{Error, Result},
    parser, Ctx, KeyValue, PATH_STRING_PREFIX,
};

#[derive(Debug, clap::Args)]
pub struct Query {
    /// The query to send. If not provided, uses `$JP_EDITOR`, `$VISUAL` or
    /// `$EDITOR` to open edit the query in an editor.
    #[arg(value_parser = string_or_path)]
    pub query: Option<String>,

    /// Use the query string as a Jinja2 template.
    ///
    /// You can provide values for template variables using the
    /// `template.values` config key.
    #[arg(long)]
    pub template: bool,

    #[arg(long, value_parser = string_or_path.try_map(json_schema))]
    pub schema: Option<schemars::Schema>,

    /// Replay the last message in the conversation.
    ///
    /// If a query is provided, it will be appended to the end of the previous
    /// message. If no query is provided, $EDITOR will open with the last
    /// message in the conversation.
    #[arg(long = "replay", conflicts_with = "new_conversation")]
    pub replay: bool,

    /// Start a new conversation without any message history.
    ///
    /// If a context named `default` exists, it will be attached to the
    /// conversation.
    #[arg(short = 'n', long = "new")]
    pub new_conversation: bool,

    /// Store the conversation locally, outside of the workspace.
    #[arg(short = 'l', long = "local", requires = "new_conversation")]
    pub local: bool,

    /// Add attachment to the context.
    #[arg(short = 'a', long = "attachment", value_parser = |s: &str| parser::attachment_url(s))]
    pub attachments: Vec<Url>,

    /// Use specific persona.
    #[arg(short = 'p', long = "persona", value_parser = PersonaId::from_str)]
    pub persona: Option<PersonaId>,

    /// Use specific context.
    #[arg(short = 'x', long = "context", value_parser = |s: &str| ContextId::try_from(s))]
    pub context: Option<ContextId>,

    /// Use specific MCP servers exclusively.
    #[arg(short = 'm', long = "mcp", value_parser = |s: &str| Ok::<_, Infallible>(McpServerId::new(s)))]
    pub mcp: Vec<McpServerId>,

    /// Whether and how to edit the query.
    #[arg(short = 'e', long = "edit", conflicts_with = "no_edit")]
    pub edit: Option<Option<Editor>>,

    /// Do not edit the query.
    #[arg(short = 'E', long = "no-edit", conflicts_with = "edit")]
    pub no_edit: bool,

    /// The model to use.
    #[arg(short = 'o', long = "model", value_parser = ModelId::from_str)]
    pub model: Option<ModelId>,

    /// The model parameters to use.
    #[arg(short = 'r', long = "param", value_name = "KEY=VALUE", action = ArgAction::Append, value_parser = KeyValue::from_str)]
    pub parameters: Vec<KeyValue>,

    /// Do not display the reasoning content.
    ///
    /// This does not stop the assistant from generating reasoning tokens to
    /// help with its accuracy, but it does not display them in the output.
    #[arg(long = "hide-reasoning")]
    pub hide_reasoning: bool,

    /// Stream the assistant's response as it is generated.
    ///
    /// This is the default behaviour for TTY sessions, but can be forced for
    /// non-TTY sessions by setting this flag.
    #[arg(short = 's', long = "stream", conflicts_with = "no_stream")]
    pub stream: bool,

    /// Disable streaming the assistant's response.
    ///
    /// This is the default behaviour for non-TTY sessions, or for structured
    /// responses, but can be forced by setting this flag.
    #[arg(short = 'S', long = "no-stream", conflicts_with = "stream")]
    pub no_stream: bool,

    /// The tool to use.
    ///
    /// If a value is provided, the tool matching the value will be used.
    ///
    /// Note that this setting is *not* persisted across queries. To persist
    /// tool choice behavior, use a named context with the `tool_choice` field,
    /// or set `llm.tool_choice` in the config file.
    #[arg(short = 't', long = "tool")]
    pub tool_choice: Option<Option<String>>,

    /// Disable tool use by the assistant.
    #[arg(short = 'T', long = "no-tool")]
    pub no_tool_choice: bool,
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
    pub async fn run(self, ctx: &mut Ctx) -> Output {
        debug!("Running `query` command.");
        trace!(args = ?self, "Received arguments.");

        let previous_id = self.update_active_conversation(ctx)?;

        self.update_config(&mut ctx.config)?;
        self.update_context(ctx).await?;
        ctx.configure_active_mcp_servers().await?;
        let model = self.get_model(ctx)?;

        let (message, query_file) = self.build_message(ctx, &model).await?;

        if let UserMessage::Query(query) = &message {
            if query.is_empty() {
                return cleanup(ctx, previous_id, query_file.as_deref()).map_err(Into::into);
            }

            // Generate title for new conversations.
            if ctx.term.args.persist
                && self.new_conversation
                && ctx.config.conversation.title.generate.auto
            {
                debug!("Generating title for new conversation");
                ctx.task_handler.spawn(TitleGeneratorTask::new(
                    ctx.workspace.active_conversation_id(),
                    &ctx.config,
                    &ctx.workspace,
                    Some(query.clone()),
                ));
            }
        }

        let conversation = ctx.workspace.get_active_conversation();
        let thread = self.build_thread(ctx, message.clone()).await?;

        let context = conversation.context.clone();
        let mut messages = vec![];
        if let Some(schema) = self.schema.clone() {
            messages.push(handle_structured_output(ctx, context, thread, &model, schema).await?);
        } else {
            self.handle_stream(
                ctx,
                context,
                thread,
                &model,
                self.tool_choice(ctx),
                &mut messages,
            )
            .await?;
        }

        let mut reply = String::new();
        for message in messages {
            let conversation_id = ctx.workspace.active_conversation_id();
            trace!(
                conversation = %conversation_id,
                content_size = message.reply.content.as_deref().unwrap_or_default().len(),
                reasoning_size = message
                    .reply
                    .reasoning
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default()
                    .len(),
                "Storing response message in conversation."
            );

            if let Some(content) = &message.reply.content {
                reply.push_str(content);
            }
            ctx.workspace.add_message(conversation_id, message.clone());
        }

        // Clean up the query file.
        if let Some(path) = query_file {
            fs::remove_file(path)?;
        }

        if self.schema.is_some() && !reply.is_empty() {
            if let RenderMode::Streamed = self.render_mode() {
                stdout::typewriter(&reply, ctx.config.style.typewriter.code_delay)?;
            } else {
                return Ok(Success::Json(serde_json::from_str(&reply)?));
            }
        }

        Ok(Success::Ok)
    }

    async fn build_message(
        &self,
        ctx: &mut Ctx,
        model: &Model,
    ) -> Result<(UserMessage, Option<PathBuf>)> {
        let conversation_id = ctx.workspace.active_conversation_id();

        // If replaying, remove the last message from the conversation, and use
        // its query message to build the new query.
        let mut message = self
            .replay
            .then(|| ctx.workspace.pop_message(&conversation_id))
            .flatten()
            .map_or(UserMessage::Query(String::new()), |m| m.message);

        // If replaying a tool call, re-run the requested tool(s) and return the
        // new results.
        if let UserMessage::ToolCallResults(_) = &mut message {
            let Some(response) = ctx.workspace.get_messages(&conversation_id).last() else {
                return Err(Error::Replay("No assistant response found".into()));
            };

            let results = handle_tool_calls(ctx, response.reply.tool_calls.clone()).await?;
            message = UserMessage::ToolCallResults(results);
        }

        // If a query is provided, prepend it to the existing message. This is
        // only relevant for replays, otherwise the existing message is empty,
        // and we replace it with the provided query.
        if let Some(text) = &self.query {
            match &mut message {
                UserMessage::Query(query) if query.is_empty() => text.clone_into(query),
                UserMessage::Query(query) => *query = format!("{text}\n\n{query}"),
                UserMessage::ToolCallResults(_) => {}
            }
        }

        let query_file_path = self.edit_message(&mut message, ctx, conversation_id, model)?;

        if let UserMessage::Query(query) = &mut message
            && self.template
        {
            let mut env = Environment::empty();
            env.set_undefined_behavior(UndefinedBehavior::SemiStrict);
            env.add_template("query", query)?;

            let tmpl = env.get_template("query")?;
            // TODO: supported nested variables
            for var in tmpl.undeclared_variables(false) {
                if ctx.config.template.values.contains_key(&var) {
                    continue;
                }

                return Err(Error::TemplateUndefinedVariable(var));
            }

            *query = tmpl.render(&ctx.config.template.values)?;
        }

        Ok((message, query_file_path))
    }

    /// Update the conversation context based on the contextual information
    /// passed in through the CLI, configuration, and environment variables.
    async fn update_context(&self, ctx: &mut Ctx) -> Result<()> {
        // Update context if specified
        if let Some(id) = ctx.config.conversation.context.clone() {
            debug!(
                %id,
                "Using named context in conversation due to conversation.context config."
            );

            // Get context.
            let context = ctx
                .workspace
                .get_named_context(&id)
                .ok_or(Error::NotFound("Context", id.to_string()))?
                .clone();

            // Update conversation context.
            ctx.workspace.get_active_conversation_mut().context = context;
        }

        // Update persona if specified
        if let Some(id) = ctx.config.conversation.persona.clone() {
            debug!(
                %id,
                "Changing persona in conversation context due to conversation.persona config."
            );

            // Ensure persona exists.
            ctx.workspace
                .get_persona(&id)
                .ok_or(Error::NotFound("Persona", id.to_string()))?;

            // Update context with new persona.
            ctx.workspace
                .get_active_conversation_mut()
                .context
                .persona_id = id;
        }

        // Add any new attachments specified in arguments
        for attachment in &self.attachments {
            let context = &mut ctx.workspace.get_active_conversation_mut().context;
            register_attachment(attachment, context).await?;
        }

        // Set exclusive MCP servers
        let mut servers = HashSet::new();
        for id in &self.mcp {
            // Ensure MCP server exists.
            ctx.workspace
                .get_mcp_server(id)
                .ok_or(Error::NotFound("MCP server", id.to_string()))?;

            servers.insert(id.clone());
        }

        if !servers.is_empty() {
            debug!(
                servers = servers
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", "),
                "Overriding MCP server in conversation context due to --mcp flag."
            );

            ctx.workspace
                .get_active_conversation_mut()
                .context
                .mcp_server_ids = servers;
        }

        Ok(())
    }

    /// Update the config based on overrides from the CLI.
    ///
    /// The `--cfg` global flag is handled separately, this is specifically for
    /// "convenience" flags such as `--persona` or `--context`, which are
    /// equivalent to `--cfg conversation.persona` and `--cfg
    /// conversation.context`.
    fn update_config(&self, config: &mut jp_config::Config) -> Result<()> {
        // Hide reasoning.
        if self.hide_reasoning {
            config.style.reasoning.show = false;
        }

        // Update the conversation context.
        if let Some(context) = self.context.as_ref() {
            config.conversation.context = Some(context.clone());
        }

        // Update the persona.
        if let Some(persona) = self.persona.as_ref() {
            config.conversation.persona = Some(persona.clone());
        }

        // Update the model parameters.
        for KeyValue(key, value) in &self.parameters {
            config
                .llm
                .model
                .parameters
                .get_or_insert_default()
                .set(key, value.to_owned())?;
        }

        Ok(())
    }

    fn update_active_conversation(&self, ctx: &mut Ctx) -> Result<ConversationId> {
        let last_active_conversation_id = ctx.workspace.active_conversation_id();

        if !self.new_conversation {
            return Ok(last_active_conversation_id);
        }

        let id = ctx.workspace.create_conversation(Conversation {
            local: self.local,
            ..Default::default()
        });

        debug!(
            %id,
            local = %self.local,
            "Creating new active conversation due to --new flag."
        );

        ctx.workspace.set_active_conversation_id(id)?;
        Ok(last_active_conversation_id)
    }

    // Open the editor for the query, if requested.
    fn edit_message(
        &self,
        message: &mut UserMessage,
        ctx: &mut Ctx,
        conversation_id: ConversationId,
        model: &Model,
    ) -> Result<Option<PathBuf>> {
        let UserMessage::Query(query) = message else {
            return Ok(None);
        };

        let mut editor = Editor::from_cli_or_config(self.edit.clone(), ctx.config.editor.clone());

        // Explicitly disable editing if the `--no-edit` flag is set.
        if self.no_edit || self.query.as_ref().is_some_and(|_| self.edit.is_none()) {
            editor = Some(Editor::Disabled);
        }

        let editor = match editor {
            None => return Ok(None),
            Some(Editor::Default) => unreachable!("handled in `from_cli_or_config`"),
            // If editing is disabled, we set the query as a single whitespace,
            // which allows the query to pass through to the assistant.
            Some(Editor::Disabled) => {
                if query.is_empty() {
                    " ".clone_into(query);
                }
                return Ok(None);
            }
            Some(cmd @ Editor::Command(_)) => match cmd.command() {
                Some(cmd) => cmd,
                None => return Ok(None),
            },
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
            editor::edit_query(ctx, conversation_id, model, initial_message, editor)
                .map(|(q, p)| (q, Some(p)))?;

        Ok(query_file_path)
    }

    /// Get the model to use for the current query.
    ///
    /// 1. If the `model` CLI flag is set, use that.
    /// 2. If the current persona has a model, use that.
    /// 3. If a model is configured in a configuration file or environment
    ///    variable, use that.
    /// 4. Otherwise return an error.
    fn get_model(&self, ctx: &Ctx) -> Result<Model> {
        let persona_id = &ctx.workspace.get_active_conversation().context.persona_id;
        let persona = ctx
            .workspace
            .get_persona(persona_id)
            .ok_or(Error::NotFound("Persona", persona_id.to_string()))?;

        let Some(id) = self
            .model
            .clone()
            .or_else(|| persona.model.clone())
            .or_else(|| ctx.config.llm.model.id.clone())
        else {
            return Err(Error::UndefinedModel);
        };

        let mut parameters = ctx.config.llm.model.parameters.clone().unwrap_or_default();
        if persona.inherit_parameters {
            parameters.merge(persona.parameters.clone());
        } else {
            parameters = persona.parameters.clone();
        }

        let model = Model { id, parameters };

        trace!(provider = %model.id.provider(), slug = %model.id.slug(), "Loaded LLM model.");

        Ok(model)
    }

    fn tool_choice(&self, ctx: &Ctx) -> ToolChoice {
        self.no_tool_choice
            .then_some(ToolChoice::None)
            .or_else(|| {
                self.tool_choice.as_ref().map(|v| match v.as_deref() {
                    None | Some("true") => ToolChoice::Required,
                    Some(v) => match v {
                        "false" => ToolChoice::None,
                        _ => ToolChoice::Function(v.to_owned()),
                    },
                })
            })
            .or_else(|| {
                ctx.workspace
                    .get_active_conversation()
                    .context
                    .tool_choice
                    .clone()
            })
            .or_else(|| ctx.config.llm.tool_choice.clone())
            .unwrap_or(ToolChoice::Auto)
    }

    async fn build_thread(&self, ctx: &Ctx, message: UserMessage) -> Result<Thread> {
        let conversation_id = ctx.workspace.active_conversation_id();
        let conversation = ctx.workspace.get_active_conversation();
        let persona_id = &conversation.context.persona_id;
        let persona = ctx
            .workspace
            .get_persona(persona_id)
            .ok_or(Error::persona_not_found(persona_id))?;

        let tools = ctx.mcp_client.list_tools().await?;
        let mut attachments = vec![];
        for handler in conversation.context.attachment_handlers.values() {
            attachments.extend(
                handler
                    .get(&ctx.workspace.root, ctx.mcp_client.clone())
                    .await
                    .map_err(|e| Error::Attachment(e.to_string()))?,
            );
        }

        let mut thread_builder = ThreadBuilder::default()
            .with_system_prompt(persona.system_prompt.clone())
            .with_instructions(persona.instructions.clone())
            .with_attachments(attachments)
            .with_history(ctx.workspace.get_messages(&conversation_id).to_vec())
            .with_message(message);

        if !tools.is_empty() {
            let instruction = Instructions::default()
                .with_title("Tool Usage")
                .with_description("How to leverage the tools available to you.".to_string())
                .with_item("Use all the tools available to you to give the best possible answer.")
                .with_item("Verify the tool name, description and parameters are correct.")
                .with_item(
                    "Even if you've reasoned yourself towards a solution, use any available tool \
                     to verify your answer.",
                );

            thread_builder = thread_builder.with_instruction(instruction);
        }

        Ok(thread_builder.build()?)
    }

    #[expect(clippy::too_many_lines)]
    async fn handle_stream(
        &self,
        ctx: &mut Ctx,
        context: Context,
        mut thread: Thread,
        model: &Model,
        tool_choice: ToolChoice,
        messages: &mut Vec<MessagePair>,
    ) -> Result<()> {
        let tools = ctx.mcp_client.list_tools().await?;
        let provider = provider::get_provider(model.id.provider(), &ctx.config.llm.provider)?;
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
            tool_choice,
            ..Default::default()
        };
        let mut stream = provider.chat_completion_stream(model, query).await?;

        let mut content_tokens = String::new();
        let mut reasoning_tokens = String::new();
        let mut handler = ResponseHandler::new(self.render_mode());
        let mut metadata = BTreeMap::new();
        let mut tool_calls = Vec::new();
        let mut tool_call_results = Vec::new();

        while let Some(event) = stream.next().await {
            let data = match event? {
                StreamEvent::ChatChunk(chunk) => match chunk {
                    CompletionChunk::Reasoning(data) if !data.is_empty() => {
                        reasoning_tokens.push_str(&data);

                        if !ctx.config.style.reasoning.show {
                            continue;
                        }

                        data
                    }
                    CompletionChunk::Content(mut data) if !data.is_empty() => {
                        let reasoning_ended = !reasoning_tokens.is_empty()
                            && ctx.config.style.reasoning.show
                            && content_tokens.is_empty();

                        content_tokens.push_str(&data);

                        // If the response includes reasoning, we add two newlines
                        // after the reasoning, but before the content.
                        if reasoning_ended {
                            data = format!("\n\n{data}");
                        }

                        data
                    }
                    _ => continue,
                },
                // Tool calls are handled after the stream is finished.
                //
                // We do add a history of the call to the content tokens for the
                // LLMs understanding, but we do not print it to the terminal.
                StreamEvent::ToolCall(call) => {
                    tool_calls.push(call.clone());

                    let data = indoc::formatdoc!(
                        "
                    ---
                    executing tool: **{}**

                    arguments:
                    ```json
                    {:#}
                    ```

                ",
                        call.name,
                        call.arguments
                    );

                    handler.handle(&data, ctx)?;
                    let result = handle_tool_call(ctx, call.clone()).await?;
                    tool_call_results.push(result.clone());

                    let content = if result.content.starts_with("```") {
                        result.content
                    } else {
                        format!("```\n{}\n```", result.content)
                    };

                    indoc::formatdoc! {"
                    result:

                    {content}
                    ---
                    "
                    }
                }
                StreamEvent::Metadata(key, data) => {
                    metadata.insert(key, data);
                    continue;
                }
            };

            handler.handle(&data, ctx)?;
        }

        // Ensure we handle the last line of the stream.
        if !handler.buffer.is_empty() {
            handler.handle("\n", ctx)?;
        }

        let content_tokens = content_tokens.trim().to_string();
        let content = if !content_tokens.is_empty() {
            Some(content_tokens)
        } else if content_tokens.is_empty() && tool_calls.is_empty() {
            Some("<no reply>".to_string())
        } else {
            None
        };

        let reasoning_tokens = reasoning_tokens.trim().to_string();
        let reasoning = if reasoning_tokens.is_empty() {
            None
        } else {
            Some(reasoning_tokens)
        };

        if let RenderMode::Buffered = handler.render_mode {
            println!("{}", handler.parsed.join("\n"));
        } else if content.is_some() || reasoning.is_some() {
            // Final newline.
            println!();
        }

        let message = MessagePair::new(message, AssistantMessage {
            metadata,
            content,
            reasoning,
            tool_calls: tool_calls.clone(),
        })
        .with_context(context.clone());
        messages.push(message.clone());

        // If the assistant asked for a tool call, we handle it automatically,
        // essentially going into a "loop" until no more tool calls are
        // requested.
        //
        // TODO:
        //
        // This should be handled differently, asking for permission to run a
        // tool (unless whitelisted per conversation/globally), it should log
        // the fact that a tool call is triggered, and it should guard against
        // infinite loops.
        if !tool_call_results.is_empty() {
            thread.history.push(message);
            thread.message = UserMessage::ToolCallResults(tool_call_results);

            Box::pin(self.handle_stream(
                ctx,
                context,
                thread,
                model,
                // After the first tool call, we revert back to letting the LLM
                // decide if/which tool to use.
                ToolChoice::Auto,
                messages,
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
    context: Context,
    thread: Thread,
    model: &Model,
    schema: schemars::Schema,
) -> Result<MessagePair> {
    let provider = provider::get_provider(model.id.provider(), &ctx.config.llm.provider)?;
    let message = thread.message.clone();
    let query =
        StructuredQuery::new(schema, thread).map_err(|err| Error::Schema(err.to_string()))?;

    let value = provider.structured_completion(model, query).await?;
    let content = if ctx.term.is_tty {
        serde_json::to_string_pretty(&value)?
    } else {
        serde_json::to_string(&value)?
    };

    Ok(MessagePair::new(message, AssistantMessage::from(content)).with_context(context))
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

async fn handle_tool_calls(
    ctx: &Ctx,
    tool_calls: Vec<ToolCallRequest>,
) -> Result<Vec<ToolCallResult>> {
    let mut results = vec![];
    for call in tool_calls {
        results.push(handle_tool_call(ctx, call).await?);
    }

    Ok(results)
}

async fn handle_tool_call(ctx: &Ctx, call: ToolCallRequest) -> Result<ToolCallResult> {
    info!(tool = %call.name, arguments = %call.arguments, "Calling tool.");

    let result = ctx.mcp_client.call_tool(&call.name, call.arguments).await?;
    trace!(result = ?result, "Tool call completed.");

    Ok(ToolCallResult {
        id: call.id,
        error: result.is_error.unwrap_or(false),
        content: result
            .content
            .into_iter()
            .filter_map(|c| match c.raw {
                jp_mcp::RawContent::Text(text_content) => Some(text_content.text),
                jp_mcp::RawContent::Resource(embedded_resource) => {
                    match embedded_resource.resource {
                        ResourceContents::TextResourceContents { text, .. } => Some(text),
                        ResourceContents::BlobResourceContents { .. } => None,
                    }
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n\n"),
    })
}

struct Line {
    content: String,
    variant: LineVariant,
}

#[derive(Debug)]
enum LineVariant {
    Normal,
    Code,
    FencedCodeBlockStart { language: Option<String> },
    FencedCodeBlockEnd { indent: usize },
}

impl Line {
    fn new(content: String, in_fenced_code_block: bool) -> Self {
        let variant = if in_fenced_code_block && content.trim().ends_with("```") {
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

#[derive(Debug, Default)]
struct ResponseHandler {
    /// How to render the response.
    render_mode: RenderMode,

    /// The streamed, unprocessed lines received from the LLM.
    received: Vec<String>,

    /// The lines that have been parsed so far.
    ///
    /// If `should_stream` is `true`, these lines have been printed to the
    /// terminal. Otherwise they will be printed when the response handler is
    /// finished.
    parsed: Vec<String>,

    /// A temporary buffer of data received from the LLM.
    buffer: String,

    in_fenced_code_block: bool,
    // (language, code)
    code_buffer: (Option<String>, Vec<String>),
    code_line: usize,

    // The last index of the line that ends a code block.
    // (streamed, printed)
    last_fenced_code_block_end: (usize, usize),
}

impl ResponseHandler {
    fn new(render_mode: RenderMode) -> Self {
        Self {
            render_mode,
            ..Default::default()
        }
    }

    fn handle(&mut self, data: &str, ctx: &Ctx) -> Result<()> {
        self.buffer.push_str(data);

        while let Some(Line { content, variant }) = self.get_line() {
            self.received.push(content);

            let delay = match variant {
                LineVariant::Code => ctx.config.style.typewriter.code_delay,
                _ => ctx.config.style.typewriter.text_delay,
            };

            let lines = self.handle_line(&variant, ctx)?;

            if !matches!(self.render_mode, RenderMode::Buffered) {
                stdout::typewriter(&lines.join("\n"), delay)?;
            }

            self.parsed.extend(lines);
        }

        Ok(())
    }

    #[expect(clippy::too_many_lines)]
    fn handle_line(&mut self, variant: &LineVariant, ctx: &Ctx) -> Result<Vec<String>> {
        let Some(content) = self.received.last().map(String::as_str) else {
            return Ok(vec![]);
        };

        match variant {
            LineVariant::Code => {
                self.code_line += 1;
                self.code_buffer.1.push(content.to_owned());

                let mut buf = String::new();
                let config = code::Config {
                    language: self.code_buffer.0.clone(),
                    theme: ctx
                        .config
                        .style
                        .code
                        .color
                        .then(|| ctx.config.style.code.theme.clone()),
                };

                if !code::format(content, &mut buf, &config)? {
                    let config = code::Config {
                        language: None,
                        theme: config.theme,
                    };

                    code::format(content, &mut buf, &config)?;
                }

                if ctx.config.style.code.line_numbers {
                    buf.insert_str(
                        0,
                        &format!("{:2} │ ", self.code_line)
                            .with(Color::AnsiValue(238))
                            .to_string(),
                    );
                }

                Ok(vec![buf])
            }
            LineVariant::FencedCodeBlockStart { language } => {
                self.code_buffer.0.clone_from(language);
                self.code_buffer.1.clear();
                self.code_line = 0;
                self.in_fenced_code_block = true;

                Ok(vec![content.with(Color::AnsiValue(238)).to_string()])
            }
            LineVariant::FencedCodeBlockEnd { indent } => {
                self.last_fenced_code_block_end = (self.received.len(), self.parsed.len() + 2);

                let path = self.persist_code_block()?;
                let mut links = vec![];

                match ctx.config.style.code.file_link {
                    LinkStyle::Off => {}
                    LinkStyle::Full => {
                        links.push(format!(
                            "{}see: file://{}",
                            " ".repeat(*indent),
                            path.display()
                        ));
                    }
                    LinkStyle::Osc8 => {
                        links.push(format!(
                            "{}[{}]",
                            " ".repeat(*indent),
                            hyperlink(
                                format!("file://{}", path.display()),
                                "open in editor".red().to_string()
                            )
                        ));
                    }
                }

                match ctx.config.style.code.copy_link {
                    LinkStyle::Off => {}
                    LinkStyle::Full => {
                        links.push(format!(
                            "{}copy: copy://{}",
                            " ".repeat(*indent),
                            path.display()
                        ));
                    }
                    LinkStyle::Osc8 => {
                        links.push(format!(
                            "{}[{}]",
                            " ".repeat(*indent),
                            hyperlink(
                                format!("copy://{}", path.display()),
                                "copy to clipboard".red().to_string()
                            )
                        ));
                    }
                }

                self.in_fenced_code_block = false;

                let mut lines = vec![content.with(Color::AnsiValue(238)).to_string()];
                if !links.is_empty() {
                    lines.push(links.join(" "));
                }

                Ok(lines)
            }
            LineVariant::Normal => {
                // We feed all the lines for markdown formatting, but only
                // print the last one, as the others are already printed.
                //
                // This helps the parser to use previous context to apply
                // the correct formatting to the current line.
                //
                // We only care about the lines after the last code block
                // end, because a) formatting context is reset after a code
                // block, and b) we dot not limit the line length of code, makes
                // it impossible to correctly find the non-printed lines based
                // on wrapped vs non-wrapped lines.
                let lines = self
                    .received
                    .iter()
                    .skip(self.last_fenced_code_block_end.0)
                    .cloned()
                    .collect::<Vec<_>>();

                // `termimad` removes empty lines at the start or end, but we
                // want to keep them as we will have more lines to print.
                let empty_lines_start_count = lines.iter().take_while(|s| s.is_empty()).count();
                let empty_lines_end_count = lines.iter().rev().take_while(|s| s.is_empty()).count();

                let options = comrak::Options {
                    render: comrak::RenderOptions {
                        unsafe_: true,
                        prefer_fenced: true,
                        experimental_minimize_commonmark: true,
                        ..Default::default()
                    },
                    ..Default::default()
                };

                let formatted = comrak::markdown_to_commonmark(&lines.join("\n"), &options);

                let mut formatted =
                    FmtText::from(&termimad::MadSkin::default(), &formatted, Some(100)).to_string();

                for _ in 0..empty_lines_start_count {
                    formatted.insert(0, '\n');
                }

                // Only add an extra newline if we have more than one line,
                // otherwise a single empty line will be interpreted as both a
                // missing start and end newline.
                if lines.iter().any(|s| !s.is_empty()) {
                    for _ in 0..empty_lines_end_count {
                        formatted.push('\n');
                    }
                }

                let lines = formatted
                    .lines()
                    .skip(self.parsed.len() - self.last_fenced_code_block_end.1)
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>();

                Ok(lines)
            }
        }
    }

    fn get_line(&mut self) -> Option<Line> {
        let s = &mut self.buffer;
        let idx = s.find('\n')?;

        // Determine the end index of the actual line *content*.
        // Check if the character before '\n' is '\r'.
        let end_idx = if idx > 0 && s.as_bytes().get(idx - 1) == Some(&b'\r') {
            idx - 1
        } else {
            idx
        };

        // Extract the line content *before* draining.
        // Creating a slice and then converting to owned String.
        let extracted_line = s[..end_idx].to_string();

        // Calculate the index *after* the newline sequence to drain up to.
        // This ensures we remove the '\n' and potentially the preceding '\r'.
        let drain_end_idx = idx + 1;
        s.drain(..drain_end_idx);

        Some(Line::new(extracted_line, self.in_fenced_code_block))
    }

    fn persist_code_block(&self) -> Result<PathBuf> {
        let code = self.code_buffer.1.clone();
        let language = self.code_buffer.0.as_deref().unwrap_or("txt");
        let ext = match language {
            "c++" => "cpp",
            "javascript" => "js",
            "python" => "py",
            "ruby" => "rb",
            "rust" => "rs",
            "typescript" => "ts",
            lang => lang,
        };

        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_millis();
        let path = std::env::temp_dir().join(format!("code_{millis}.{ext}"));

        fs::write(&path, code.join("\n"))?;

        Ok(path)
    }
}
