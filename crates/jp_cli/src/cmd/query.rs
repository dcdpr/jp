mod event;
mod response_handler;
mod turn;

use std::{
    collections::{BTreeMap, HashSet},
    env, fs,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
    time::Duration,
};

use clap::{ArgAction, builder::TypedValueParser as _};
use event::StreamEventHandler;
use futures::StreamExt as _;
use itertools::Itertools as _;
use jp_attachment::Attachment;
use jp_config::{
    AppConfig, PartialAppConfig, PartialConfig as _,
    assignment::{AssignKeyValue as _, KvAssignment},
    assistant::{AssistantConfig, instructions::InstructionsConfig, tool_choice::ToolChoice},
    fs::{expand_tilde, load_partial},
    model::parameters::{PartialCustomReasoningConfig, PartialReasoningConfig, ReasoningConfig},
    style::reasoning::ReasoningDisplayConfig,
};
use jp_conversation::{
    Conversation, ConversationEvent, ConversationId, ConversationStream, EventKind,
    event::{ChatRequest, ChatResponse},
    thread::{Thread, ThreadBuilder},
};
use jp_llm::{
    ToolError,
    event::Event,
    provider,
    query::{ChatQuery, StructuredQuery},
    tool::{ToolDefinition, tool_definitions},
};
use jp_task::task::TitleGeneratorTask;
use jp_term::stdout;
use jp_workspace::Workspace;
use minijinja::{Environment, UndefinedBehavior};
use response_handler::ResponseHandler;
use serde_json::Value;
use tracing::{debug, error, info, trace, warn};
use url::Url;

use super::{Output, attachment::register_attachment};
use crate::{
    Ctx, PATH_STRING_PREFIX,
    cmd::{self, Success, query::turn::TurnState},
    ctx::IntoPartialAppConfig,
    editor::{self, Editor},
    error::{Error, Result},
    parser,
    signals::{SignalRx, SignalTo},
};

const EMPTY_RESPONSE_MESSAGE: &str = " -- The response appears to be empty. Please try again.";

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
    #[arg(short = 'm', long = "model")]
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

impl RenderMode {
    pub fn is_streamed(self) -> bool {
        matches!(self, Self::Streamed)
    }
}

impl Query {
    #[expect(clippy::too_many_lines)]
    pub(crate) async fn run(self, ctx: &mut Ctx) -> Output {
        debug!("Running `query` command.");
        trace!(args = ?self, "Received arguments.");
        let cfg = ctx.config();

        let previous_id = self.update_active_conversation(&mut ctx.workspace, (*cfg).clone())?;
        let conversation_id = ctx.workspace.active_conversation_id();
        if let Some(delta) = get_config_delta_from_cli(&cfg, &ctx.workspace, &conversation_id)? {
            ctx.workspace
                .get_events_mut(&conversation_id)
                .expect(
                    "TODO: add this invariant to the type system. FIXME: This can actually happen \
                     right now if the `events.json` file of the active conversation is corrupt, \
                     and thus not loaded into memory.",
                )
                .add_config_delta(delta);
        }

        ctx.configure_active_mcp_servers().await?;

        let root = ctx
            .workspace
            .storage_path()
            .unwrap_or(&ctx.workspace.root)
            .to_path_buf();

        let conversation = ctx.workspace.get_conversation(&conversation_id);
        let conversation_path = root.join(
            conversation_id.to_dirname(conversation.as_ref().and_then(|v| v.title.as_deref())),
        );

        let (query_file, editor_provided_config) = self.build_conversation(
            ctx.workspace
                .get_events_mut(&conversation_id)
                .expect("TODO: add this invariant to the type system"),
            &cfg,
            &conversation_path,
        )?;

        let has_request = ctx
            .workspace
            .get_events(&conversation_id)
            .and_then(ConversationStream::last)
            .and_then(|v| v.as_chat_request().map(|v| !v.is_empty()))
            .unwrap_or(false);

        if !has_request {
            return cleanup(ctx, previous_id, query_file.as_deref()).map_err(Into::into);
        }

        if !editor_provided_config.is_empty() {
            ctx.workspace
                .get_events_mut(&conversation_id)
                .expect("TODO: add this invariant to the type system")
                .add_config_delta(editor_provided_config);
        }

        let stream = ctx
            .workspace
            .get_events(&conversation_id)
            .cloned()
            .unwrap_or_else(|| ConversationStream::new(cfg.clone()));

        // Generate title for new or empty conversations.
        if (self.new_conversation || stream.is_empty())
            && ctx.term.args.persist
            && cfg.conversation.title.generate.auto
        {
            debug!("Generating title for new conversation");
            ctx.task_handler.spawn(TitleGeneratorTask::new(
                conversation_id,
                stream.clone(),
                &cfg,
            )?);
        }

        let tools = tool_definitions(cfg.conversation.tools.iter(), &ctx.mcp_client).await?;

        let mut attachments = vec![];
        for attachment in &cfg.conversation.attachments {
            register_attachment(ctx, &attachment.to_url()?, &mut attachments).await?;
        }

        // Keep track of the number of events in the stream, so that we can
        // later append new events to the end.
        let current_events = stream.len();

        let mut thread = build_thread(stream, attachments, &cfg.assistant, &tools)?;

        let mut result = Output::Ok(Success::Ok);
        if let Some(schema) = self.schema.clone() {
            result = handle_structured_output(
                &cfg,
                ctx.term.is_tty,
                &mut thread,
                schema,
                self.render_mode(),
            )
            .await;
        } else {
            let mut turn_state = TurnState::default();
            if let Err(error) = self
                .handle_stream(
                    &cfg,
                    &mut ctx.signals.receiver,
                    &ctx.mcp_client,
                    ctx.workspace.root.clone(),
                    ctx.term.is_tty,
                    &mut turn_state,
                    &mut thread,
                    cfg.assistant.tool_choice.clone(),
                    tools,
                    conversation_id,
                )
                .await
            {
                result = Output::Err(cmd::Error::from(error).with_persistence(true));
            }
        }

        let stream = ctx
            .workspace
            .get_events_mut(&conversation_id)
            .expect("TODO: add this invariant to the type system");

        for event in thread.events.into_iter().skip(current_events) {
            stream.push_with_config_delta(event);
        }

        // Clean up the query file, unless we got an error.
        if let Some(path) = query_file
            && result.is_ok()
        {
            fs::remove_file(path)?;
        }

        result
    }

    fn build_conversation(
        &self,
        stream: &mut ConversationStream,
        config: &AppConfig,
        root: &Path,
    ) -> Result<(Option<PathBuf>, PartialAppConfig)> {
        // If replaying, remove all events up-to-and-including the last
        // `ChatRequest` event, which we'll replay.
        //
        // If not replaying (or replaying but no chat request event exists), we
        // create a new `ChatRequest` event, to populate with either the
        // provided query, or the contents of the text editor.
        let mut request = self
            .replay
            .then(|| stream.trim_chat_request())
            .flatten()
            .unwrap_or_default();

        // If a query is provided, prepend it to the chat request. This is only
        // relevant for replays, otherwise the chat request is still empty, so
        // we replace it with the provided query.
        if let Some(text) = &self.query {
            let text = text.join(" ");
            let sep = if request.is_empty() { "" } else { "\n\n" };
            *request = format!("{text}{sep}{request}");
        }

        let editor_details = self.edit_message(&mut request, stream, config, root)?;

        if self.template {
            let mut env = Environment::empty();
            env.set_undefined_behavior(UndefinedBehavior::SemiStrict);
            env.add_template("query", &request.content)?;

            let tmpl = env.get_template("query")?;
            // TODO: supported nested variables
            for var in tmpl.undeclared_variables(false) {
                if config.template.values.contains_key(&var) {
                    continue;
                }

                return Err(Error::TemplateUndefinedVariable(var));
            }

            *request = tmpl.render(&config.template.values)?;
        }

        stream.add_chat_request(request);

        Ok(editor_details)
    }

    fn update_active_conversation(
        &self,
        ws: &mut Workspace,
        cfg: AppConfig,
    ) -> Result<ConversationId> {
        // Store the (old) active conversation ID, so that we can restore to it,
        // if the current conversation is aborted early (e.g. because of an
        // empty query or any other error).
        let last_active_conversation_id = ws.active_conversation_id();

        // Set new active conversation if requested.
        if self.new_conversation {
            let id = ws.create_conversation(Conversation::default().with_local(self.local), cfg);

            debug!(
                %id,
                local = %self.local,
                "Creating new active conversation due to --new flag."
            );

            ws.set_active_conversation_id(id)?;
        }

        Ok(last_active_conversation_id)
    }

    // Open the editor for the query, if requested.
    fn edit_message(
        &self,
        request: &mut ChatRequest,
        stream: &mut ConversationStream,
        config: &AppConfig,
        root: &Path,
    ) -> Result<(Option<PathBuf>, PartialAppConfig)> {
        // If there is no query provided, but the user explicitly requested not
        // to edit the query, we populate the query with a default message,
        // since most LLM providers do not support empty queries.
        //
        // See `force_no_edit` why this can be useful.
        if request.is_empty() && self.force_no_edit() {
            "<no additional context provided>".clone_into(request);
        }

        // If a query is provided, and editing is not explicitly requested, we
        // omit opening the editor.
        if !request.is_empty() && !self.force_edit() {
            return Ok((None, PartialAppConfig::empty()));
        }

        let editor = match config.editor.command() {
            None if !request.is_empty() => return Ok((None, PartialAppConfig::empty())),
            None => return Err(Error::MissingEditor),
            Some(cmd) => cmd,
        };

        let (content, query_file, editor_provided_config) =
            editor::edit_query(config, root, stream, request.as_str(), editor, None)?;
        request.content = content;

        Ok((Some(query_file), editor_provided_config))
    }

    #[expect(clippy::too_many_lines, clippy::too_many_arguments)]
    async fn handle_stream(
        &self,
        cfg: &AppConfig,
        signals: &mut SignalRx,
        mcp_client: &jp_mcp::Client,
        root: PathBuf,
        is_tty: bool,
        turn_state: &mut TurnState,
        thread: &mut Thread,
        tool_choice: ToolChoice,
        tools: Vec<ToolDefinition>,
        conversation_id: ConversationId,
    ) -> Result<()> {
        let mut result = Ok(());
        let mut cancelled = false;
        turn_state.request_count += 1;

        let model_id = cfg
            .assistant
            .model
            .id
            .finalize(&cfg.providers.llm.aliases)?;

        let provider = provider::get_provider(model_id.provider, &cfg.providers.llm)?;
        let query = ChatQuery {
            thread: thread.clone(),

            // Limit the tools to the ones that are relevant to the tool choice.
            //
            // FIXME: This should be done in the individual `Provider`
            // implementations. This is because some providers support tool
            // caching, but the cache is busted if the list of tools changes.
            // Since tools can have elaborate descriptions, this can result in a
            // significant amount of uncached tokens. For those providers, we
            // should just trust that `ToolChoice::Function` is handled
            // correctly by the provider and the correct tool is used, even if
            // others are available.
            // tools: match &tool_choice {
            //     ToolChoice::None => vec![],
            //     ToolChoice::Auto | ToolChoice::Required => tools.clone(),
            //     ToolChoice::Function(name) => tools
            //         .clone()
            //         .into_iter()
            //         .filter(|v| &v.name == name)
            //         .collect(),
            // },
            tools: tools.clone(),
            tool_choice: tool_choice.clone(),
            tool_call_strict_mode: false,
        };
        let model = provider.model_details(&model_id.name).await?;

        info!(
            model = model
                .display_name
                .as_deref()
                .unwrap_or(&model.id.to_string()),
            tools = query.tools.iter().map(|v| &v.name).sorted().join(", "),
            attachments = query
                .thread
                .attachments
                .iter()
                .map(|v| &v.source)
                .sorted()
                .join(", "),
            "Chat query created."
        );

        let mut stream = provider.chat_completion_stream(&model, query).await?;

        let mut event_handler = StreamEventHandler::default();

        let mut printer = ResponseHandler::new(self.render_mode(), cfg.style.tool_call.show);
        let mut metadata = BTreeMap::new();

        loop {
            jp_macro::select!(
                biased,
                signals.recv(),
                |signal| {
                    debug!(?signal, "Received signal.");
                    match signal {
                        // Stop processing events, but gracefully store the
                        // conversation state.
                        Ok(SignalTo::Shutdown) => {
                            cancelled = true;
                            break;
                        }
                        // Immediately stop processing events, and exit, without
                        // storing the new conversation state.
                        Ok(SignalTo::Quit) => return Ok(()),
                        Ok(SignalTo::ReloadFromDisk) => {}
                        Err(error) => error!(?error, "Failed to receive signal."),
                    }
                },
                stream.next(),
                |event| {
                    let Some(event) = event else {
                        break;
                    };

                    if let Err(error) = self
                        .handle_event(
                            event,
                            cfg,
                            mcp_client,
                            root.clone(),
                            is_tty,
                            signals,
                            turn_state,
                            provider.as_ref(),
                            thread,
                            &tool_choice,
                            &tools,
                            &mut printer,
                            &mut event_handler,
                            &mut metadata,
                            conversation_id,
                        )
                        .await
                    {
                        error!(?error, "Received error while handling conversation event.");
                        cancelled = true;
                        result = Err(error);
                        break;
                    }
                },
            );
        }

        // Ensure we handle the last line of the stream.
        printer.drain(&cfg.style, false)?;

        let content_tokens = event_handler.content_tokens.trim().to_string();
        let content = if !content_tokens.is_empty() {
            Some(content_tokens)
        } else if !cancelled && content_tokens.is_empty() && event_handler.tool_calls.is_empty() {
            let max_tries = 3;
            if turn_state.request_count <= max_tries {
                warn!(
                    turn_state.request_count,
                    max_tries, "Empty response received, retrying..."
                );

                // Append retry message to the last ChatRequest
                if let Some(mut request) = thread.events.last_mut()
                    && let Some(request) = request.as_chat_request_mut()
                    && !request.ends_with(EMPTY_RESPONSE_MESSAGE)
                {
                    request.push_str(EMPTY_RESPONSE_MESSAGE);
                }

                return Box::pin(self.handle_stream(
                    cfg,
                    signals,
                    mcp_client,
                    root,
                    is_tty,
                    turn_state,
                    thread,
                    tool_choice,
                    tools,
                    conversation_id,
                ))
                .await;
            }

            error!(
                turn_state.request_count,
                "Failed to get a non-empty response."
            );
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

        // Emit reasoning response if present
        if let Some(v) = reasoning {
            thread.events.add_chat_response(ChatResponse::reasoning(v));

            if let Some(mut event) = thread.events.last_mut() {
                event.metadata.extend(metadata);
            }
        }

        // Emit message response if present
        if let Some(v) = content {
            thread.events.add_chat_response(ChatResponse::message(v));
        }

        // Emit tool call request events.
        for tool_call in event_handler.tool_calls {
            thread.events.add_tool_call_request(tool_call);
        }

        let has_tool_call_responses = !event_handler.tool_call_responses.is_empty();
        for response in event_handler.tool_call_responses {
            thread.events.add_tool_call_response(response);
        }

        // If a cancellation was requested, we DO NOT deliver the responses
        // to the assistant. We DO store the responses to disk, such that a
        // new invocation of the CLI picks up where we left off.
        //
        // If not cancelled, we deliver the tool call results to the
        // assistant in a loop. Rebuild thread with all events so far.
        if !cancelled && has_tool_call_responses {
            turn_state.request_count = 0;

            Box::pin(self.handle_stream(
                cfg,
                signals,
                mcp_client,
                root,
                is_tty,
                turn_state,
                thread,
                // After the first tool call, we revert back to letting the LLM
                // decide if/which tool to use.
                ToolChoice::Auto,
                tools,
                conversation_id,
            ))
            .await?;
        }

        result
    }

    #[expect(clippy::too_many_arguments)]
    async fn handle_event(
        &self,
        event: std::result::Result<Event, jp_llm::Error>,
        cfg: &AppConfig,
        mcp_client: &jp_mcp::Client,
        root: PathBuf,
        is_tty: bool,
        signals: &mut SignalRx,
        turn_state: &mut TurnState,
        provider: &dyn provider::Provider,
        thread: &mut Thread,
        tool_choice: &ToolChoice,
        tools: &[ToolDefinition],
        printer: &mut ResponseHandler,
        event_handler: &mut StreamEventHandler,
        metadata: &mut BTreeMap<String, Value>,
        conversation_id: ConversationId,
    ) -> Result<()> {
        let tries = turn_state.request_count;
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
                    cfg,
                    signals,
                    mcp_client,
                    root,
                    is_tty,
                    turn_state,
                    thread,
                    tool_choice.clone(),
                    tools.to_vec(),
                    conversation_id,
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
            Event::Part { event, .. } => {
                let ConversationEvent {
                    kind, metadata: m, ..
                } = event;
                metadata.extend(m);

                match kind {
                    EventKind::ChatResponse(response) => {
                        event_handler.handle_chat_chunk(cfg.style.reasoning.display, response)
                    }
                    EventKind::ToolCallRequest(request) => {
                        event_handler
                            .handle_tool_call(
                                cfg, mcp_client, root, is_tty, turn_state, request, printer,
                            )
                            .await?
                    }
                    EventKind::ChatRequest(_) => panic!("invalid part `ChatRequest` received"),
                    EventKind::ToolCallResponse(_) => {
                        panic!("invalid part `ToolCallResponse` received")
                    }
                    _ => todo!("handle `inquery` events"),
                }
            }
            Event::Flush { .. } => None,
            Event::Finished(_) => return Ok(()),
        };

        let Some(data) = data else {
            return Ok(());
        };

        printer.handle(&data, &cfg.style, false)?;

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
}

fn get_config_delta_from_cli(
    cfg: &AppConfig,
    ws: &Workspace,
    conversation_id: &ConversationId,
) -> Result<Option<PartialAppConfig>> {
    let partial = ws
        .get_events(conversation_id)
        .map_or_else(
            || Ok(PartialAppConfig::empty()),
            |stream| stream.config().map(|c| c.to_partial()),
        )
        .map_err(jp_conversation::Error::from)?;

    let partial = partial.delta(cfg.to_partial());
    if partial.is_empty() {
        return Ok(None);
    }

    Ok(Some(partial))
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
            tools,
            no_tools,
            reasoning,
            no_reasoning,
        } = &self;

        apply_model(&mut partial, model.as_deref(), merged_config);
        apply_editor(&mut partial, edit.as_ref().map(|v| v.as_ref()), *no_edit);
        apply_enable_tools(&mut partial, tools, no_tools, merged_config)?;
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
            partial.style.reasoning.display = Some(ReasoningDisplayConfig::Hidden);
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
        let config = workspace.get_events(&id).map_or_else(
            || Ok(PartialAppConfig::empty()),
            |stream| stream.config().map(|c| c.to_partial()),
        )?;

        load_partial(partial, config).map_err(Into::into)
    }
}

fn build_thread(
    events: ConversationStream,
    attachments: Vec<Attachment>,
    assistant: &AssistantConfig,
    tools: &[ToolDefinition],
) -> Result<Thread> {
    let mut thread_builder = ThreadBuilder::default()
        .with_instructions(assistant.instructions.to_vec())
        .with_attachments(attachments)
        .with_events(events);

    if let Some(system_prompt) = assistant.system_prompt.clone() {
        thread_builder = thread_builder.with_system_prompt(system_prompt);
    }

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

        thread_builder = thread_builder.add_instruction(instruction);
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
    tools: &[Option<String>],
    no_tools: &[Option<String>],
    merged_config: Option<&PartialAppConfig>,
) -> BoxedResult<()> {
    let tools = if tools.is_empty() {
        None
    } else if tools.iter().any(Option::is_none) {
        Some(vec![])
    } else {
        Some(tools.iter().filter_map(|v| v.as_deref()).collect())
    };

    let no_tools = if no_tools.is_empty() {
        None
    } else if no_tools.iter().any(Option::is_none) {
        Some(vec![])
    } else {
        Some(no_tools.iter().filter_map(|v| v.as_deref()).collect())
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

    // Disable all first, if all tools are to be disabled.
    if disable_all {
        partial
            .conversation
            .tools
            .tools
            .iter_mut()
            .for_each(|(_, v)| v.enable = Some(false));
    // Enable all tools first if all tools are to be enabled.
    } else if enable_all {
        partial
            .conversation
            .tools
            .tools
            .iter_mut()
            .for_each(|(_, v)| v.enable = Some(true));
    }

    // Then enable individual tools.
    if let Some(tools) = tools {
        partial
            .conversation
            .tools
            .tools
            .iter_mut()
            .filter(|(name, _)| tools.iter().any(|v| v == *name))
            .for_each(|(_, v)| v.enable = Some(true));
    }

    // And finally disable individual tools.
    if let Some(no_tools) = no_tools {
        partial
            .conversation
            .tools
            .tools
            .iter_mut()
            .filter(|(name, _)| no_tools.iter().any(|v| v == name))
            .for_each(|(_, v)| v.enable = Some(false));
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
    cfg: &AppConfig,
    is_tty: bool,
    thread: &mut Thread,
    schema: schemars::Schema,
    render_mode: RenderMode,
) -> Output {
    let model_id = cfg
        .assistant
        .model
        .id
        .finalize(&cfg.providers.llm.aliases)?;
    let provider = provider::get_provider(model_id.provider, &cfg.providers.llm)?;
    let query = StructuredQuery::new(schema, thread.clone());
    let model = provider.model_details(&model_id.name).await?;

    let result = provider.structured_completion(&model, query).await?;

    let content = serde_json::to_string(&result)?;
    thread
        .events
        .add_chat_response(ChatResponse::message(&content));

    let content = if is_tty {
        serde_json::to_string_pretty(&result)?
    } else {
        content
    };

    if render_mode.is_streamed() {
        stdout::typewriter(&content, cfg.style.typewriter.code_delay.into())?;
        return Ok(Success::Ok);
    }

    Ok(Success::Json(result))
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
