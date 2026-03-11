//! Query command implementation using the stream pipeline architecture.
//!
//! # Architecture Overview
//!
//! The query command handles conversational interactions with LLMs. It uses a
//! component-based architecture with clear separation of concerns.
//!
//! # Key Components
//!
//! - [`TurnCoordinator`]: State machine managing the turn lifecycle (Idle →
//!   Streaming → Executing → Complete/Aborted).
//!
//! - [`EventBuilder`]: Accumulates streamed chunks by index and produces
//!   complete [`ConversationEvent`]s on flush.
//!
//! - [`ChatResponseRenderer`]: Renders LLM output (reasoning and messages) to
//!   the terminal with display mode support.
//!
//! - [`StreamRetryState`]: Single source of
//!   truth for stream retry logic (backoff, notification, state flushing).
//!
//! - [`ToolCoordinator`]: Manages parallel tool execution.
//!
//! - [`InterruptHandler`]: Handles Ctrl+C with context-aware menus (streaming
//!   vs tool execution).
//!
//! # Turn Lifecycle
//!
//! A "turn" is the complete interaction from user query to final response:
//!
//! 1. User sends a query ([`ChatRequest`]).
//! 2. LLM streams response chunks ([`ChatResponse`]).
//! 3. If [`ToolCallRequest`] present: execute tools, send [`ToolCallResponse`],
//!    goto 2 (new cycle, same turn).
//! 4. If no tool calls: turn complete, persist and exit.
//!
//! See `docs/architecture/query-stream-pipeline.md` for the full design
//! document.
//!
//! [`TurnCoordinator`]: turn::coordinator::TurnCoordinator
//! [`EventBuilder`]: jp_conversation::event_builder::EventBuilder
//! [`ConversationEvent`]: jp_conversation::event::ConversationEvent
//! [`ChatResponseRenderer`]: stream::renderer::ChatResponseRenderer
//! [`StreamRetryState`]: stream::retry::StreamRetryState
//! [`InterruptHandler`]: interrupt::handler::InterruptHandler
//! [`ToolCallRequest`]: jp_conversation::event::ToolCallRequest
//! [`ToolCallResponse`]: jp_conversation::event::ToolCallResponse

mod interrupt;
mod stream;
pub(crate) mod tool;
mod turn;
mod turn_loop;

use std::{
    collections::HashSet,
    env, fs,
    io::{self, BufRead as _, IsTerminal},
    sync::Arc,
    time::Duration,
};

use camino::{Utf8Path, Utf8PathBuf};
use chrono::{DateTime, Utc};
use clap::{ArgAction, builder::TypedValueParser as _};
use indexmap::IndexMap;
use jp_attachment::Attachment;
use jp_config::{
    AppConfig, PartialAppConfig, PartialConfig as _,
    assignment::{AssignKeyValue as _, KvAssignment},
    assistant::{AssistantConfig, instructions::InstructionsConfig, tool_choice::ToolChoice},
    conversation::tool::Enable,
    fs::{expand_tilde, load_partial},
    model::parameters::{PartialCustomReasoningConfig, PartialReasoningConfig, ReasoningConfig},
    style::reasoning::ReasoningDisplayConfig,
};
use jp_conversation::{
    Conversation, ConversationEvent, ConversationId, ConversationStream,
    event::{ChatRequest, ChatResponse},
    thread::{Thread, ThreadBuilder},
};
use jp_inquire::prompt::TerminalPromptBackend;
use jp_llm::{
    ToolError, provider,
    tool::{
        ToolDefinition, ToolDocs,
        builtin::{BuiltinExecutors, describe_tools::DescribeTools},
        tool_definitions,
    },
};
use jp_md::format::Formatter;
use jp_printer::Printer;
use jp_storage::CONVERSATIONS_DIR;
use jp_task::task::TitleGeneratorTask;
use jp_workspace::Workspace;
use minijinja::{Environment, UndefinedBehavior};
use tool::{TerminalExecutorSource, ToolCoordinator};
use tracing::{debug, info, trace};
use turn_loop::run_turn_loop;

use super::{Output, attachment::register_attachment};
use crate::{
    Ctx, PATH_STRING_PREFIX, cmd,
    ctx::IntoPartialAppConfig,
    editor::{self, Editor},
    error::{Error, Result},
    output::print_json,
    parser::AttachmentUrlOrPath,
    signals::SignalRx,
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
    #[arg(short = '%', long)]
    template: bool,

    /// Constrain the assistant's response to match a JSON schema.
    ///
    /// Accepts either a full JSON Schema object or a concise DSL:
    ///
    ///   -s 'summary'                   → single string field
    ///   -s 'name, age int, bio'        → mixed types
    ///   -s 'summary: a brief summary'  → field with description
    ///
    /// See: <https://jp.computer/rfd/030-schema-dsl>
    #[arg(short = 's', long, value_parser = string_or_path.try_map(parse_schema))]
    schema: Option<schemars::Schema>,

    /// Replay the last message in the conversation.
    ///
    /// If a query is provided, it will be appended to the end of the previous
    /// message. If no query is provided, $EDITOR will open with the last
    /// message in the conversation.
    #[arg(long = "replay", conflicts_with = "new")]
    replay: bool,

    /// Start a new conversation without any message history.
    #[arg(
        short = 'n',
        long = "new",
        group = "new",
        conflicts_with = "new_local_conversation"
    )]
    new_conversation: bool,

    /// Start a new local conversation without any message history.
    ///
    /// This is the same as `--new --local`.
    #[arg(
        short = 'N',
        long = "new-local",
        group = "new",
        conflicts_with_all = ["new_conversation", "local"]
    )]
    new_local_conversation: bool,

    /// Store the conversation locally, outside of the workspace.
    #[arg(
        short = 'l',
        long = "local",
        requires = "new_conversation",
        conflicts_with = "new_local_conversation"
    )]
    local: bool,

    /// Add attachment to the configuration.
    #[arg(short = 'a', long = "attachment", alias = "attach")]
    attachments: Vec<AttachmentUrlOrPath>,

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
    #[arg(short = 'p', long = "param", value_name = "KEY=VALUE", action = ArgAction::Append)]
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

    /// Set the expiration date of the conversation.
    ///
    /// The conversation is persisted, but only until the conversation is no
    /// longer marked as active (e.g. when a new conversation is started), and
    /// when the expiration date is reached.
    ///
    /// This differs from `--no-persist` in that the conversation can contain
    /// multiple turns, as long as it remains active and not expired.
    #[arg(long = "tmp", requires = "new")]
    expires_in: Option<Option<humantime::Duration>>,

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

impl Query {
    #[expect(clippy::too_many_lines)]
    pub(crate) async fn run(self, ctx: &mut Ctx) -> Output {
        debug!("Running `query` command.");
        trace!(args = ?self, "Received arguments.");
        let now = ctx.now();
        let cfg = ctx.config();

        let previous_id = self.update_active_conversation(&mut ctx.workspace, cfg.clone(), now)?;
        let cid = ctx.workspace.active_conversation_id();
        if let Some(delta) = get_config_delta_from_cli(&cfg, &ctx.workspace, &cid)? {
            ctx.workspace
                .try_get_events_mut(&cid)?
                .add_config_delta(delta);
        }

        let mut mcp_servers_handle = ctx.configure_active_mcp_servers().await?;

        let root = ctx
            .workspace
            .storage_path()
            .unwrap_or(ctx.workspace.root())
            .to_path_buf();

        let conversation = ctx.workspace.get_conversation(&cid);
        let conversation_path = root
            .join(CONVERSATIONS_DIR)
            .join(cid.to_dirname(conversation.as_ref().and_then(|v| v.title.as_deref())));

        let (query_file, editor_provided_config, chat_request) = self.build_conversation(
            ctx.workspace.try_get_events_mut(&cid)?,
            &cfg,
            &conversation_path,
        )?;

        let Some(chat_request) = chat_request else {
            // Empty query, early exit.
            return cleanup(ctx, previous_id, query_file.as_deref()).map_err(Into::into);
        };

        // If we have a query, and it was built from the editor, we print it
        // to the terminal for convenience, formatted as markdown.
        if query_file.is_some() {
            let pretty = ctx.printer.pretty_printing();
            let formatter = Formatter::with_width(cfg.style.markdown.wrap_width)
                .table_max_column_width(cfg.style.markdown.table_max_column_width)
                .theme(if pretty {
                    cfg.style.markdown.theme.as_deref()
                } else {
                    None
                })
                .pretty_hr(pretty && cfg.style.markdown.hr_style.is_line())
                .inline_code_bg(
                    cfg.style
                        .inline_code
                        .background
                        .map(crate::format::color_to_bg_param),
                );

            let formatted =
                formatter.format_terminal(&format!("{}\n\n---\n\n", chat_request.content))?;
            ctx.printer.println(formatted);
        }

        if !editor_provided_config.is_empty() {
            ctx.workspace
                .try_get_events_mut(&cid)?
                .add_config_delta(editor_provided_config);
        }

        let stream = ctx
            .workspace
            .get_events(&cid)
            .cloned()
            .unwrap_or_else(|| ConversationStream::new(cfg.clone()));

        // Generate title for new or empty conversations.
        if (self.is_new() || stream.is_empty())
            && ctx.term.args.persist
            && cfg.conversation.title.generate.auto
        {
            debug!("Generating title for new conversation");
            let mut stream = stream.clone();
            stream.start_turn(chat_request.clone());
            ctx.task_handler
                .spawn(TitleGeneratorTask::new(cid, stream, &cfg)?);
        }

        // Wait for all MCP servers to finish loading.
        while let Some(result) = mcp_servers_handle.join_next().await {
            result??;
        }

        let tools = tool_definitions(cfg.conversation.tools.iter(), &ctx.mcp_client).await?;

        let mut attachments = vec![];
        for attachment in &cfg.conversation.attachments {
            register_attachment(ctx, &attachment.to_url()?, &mut attachments).await?;
        }

        let thread = build_thread(stream, attachments, &cfg.assistant, !tools.is_empty())?;
        let root = ctx.workspace.root().to_path_buf();
        let workspace = &mut ctx.workspace;

        // Sanitize any structural issues (orphaned tool calls, missing
        // user messages, etc.) before sending the stream to the provider.
        workspace.try_get_events_mut(&cid)?.sanitize();

        // If a schema is provided, set it on the ChatRequest so the
        // provider uses its native structured output API.
        let mut chat_request = chat_request;
        if let Some(schema) = &self.schema {
            chat_request.schema = schema.as_object().cloned();
        }

        let turn_result = self
            .handle_turn(
                &cfg,
                &ctx.signals.receiver,
                &ctx.mcp_client,
                root,
                ctx.term.is_tty,
                &thread.attachments,
                workspace,
                cfg.assistant.tool_choice.clone(),
                &tools,
                cid,
                ctx.printer.clone(),
                chat_request,
            )
            .await
            .map_err(|error| cmd::Error::from(error).with_persistence(true));

        // Extract structured data from the conversation after the turn.
        if self.schema.is_some() && turn_result.is_ok() {
            let data = workspace.get_events(&cid).and_then(|events| {
                events.iter().rev().find_map(|e| {
                    e.as_chat_response()
                        .and_then(ChatResponse::as_structured_data)
                        .cloned()
                })
            });

            match data {
                Some(data) => print_json(&ctx.printer, &data),
                None => return Err(Error::MissingStructuredData.into()),
            }
        }

        // Clean up the query file, unless we got an error.
        if let Some(path) = query_file
            && turn_result.is_ok()
        {
            fs::remove_file(path)?;
        }

        turn_result
    }

    /// Build the chat request for this query.
    ///
    /// Returns the editor details and the [`ChatRequest`], if non-empty.
    /// The request is **not** added to the stream — that is the
    /// responsibility of [`TurnCoordinator::start_turn`].
    ///
    /// [`TurnCoordinator::start_turn`]: turn::TurnCoordinator::start_turn
    fn build_conversation(
        &self,
        stream: &mut ConversationStream,
        config: &AppConfig,
        conversation_root: &Utf8Path,
    ) -> Result<(Option<Utf8PathBuf>, PartialAppConfig, Option<ChatRequest>)> {
        // If replaying, remove all events up-to-and-including the last
        // `ChatRequest` event, which we'll replay.
        //
        // If not replaying (or replaying but no chat request event exists), we
        // create a new `ChatRequest` event, to populate with either the
        // provided query, or the contents of the text editor.
        let mut chat_request = self
            .replay
            .then(|| stream.trim_chat_request())
            .flatten()
            .unwrap_or_default();

        // If stdin contains data, we prepend it to the chat request.
        let stdin = io::stdin();
        let piped = if stdin.is_terminal() {
            String::new()
        } else {
            stdin
                .lock()
                .lines()
                .map_while(std::result::Result::ok)
                .collect::<String>()
        };

        if !piped.is_empty() {
            let sep = if chat_request.is_empty() { "" } else { "\n\n" };
            *chat_request = format!("{piped}{sep}{chat_request}");
        }

        // If a query is provided, prepend it to the chat request. This is only
        // relevant for replays, otherwise the chat request is still empty, so
        // we replace it with the provided query.
        if let Some(text) = &self.query {
            let text = text.join(" ");
            let sep = if chat_request.is_empty() { "" } else { "\n\n" };
            *chat_request = format!("{text}{sep}{chat_request}");
        }

        let (query_file, editor_provided_config) = self.edit_message(
            &mut chat_request,
            stream,
            !piped.is_empty(),
            config,
            conversation_root,
        )?;

        if self.template {
            let mut env = Environment::empty();
            env.set_undefined_behavior(UndefinedBehavior::SemiStrict);
            env.add_template("query", &chat_request.content)?;

            let tmpl = env.get_template("query")?;
            // TODO: supported nested variables
            for var in tmpl.undeclared_variables(false) {
                if config.template.values.contains_key(&var) {
                    continue;
                }

                return Err(Error::TemplateUndefinedVariable(var));
            }

            *chat_request = tmpl.render(&config.template.values)?;
        }

        Ok((
            query_file,
            editor_provided_config,
            (!chat_request.is_empty()).then_some(chat_request),
        ))
    }

    fn update_active_conversation(
        &self,
        ws: &mut Workspace,
        cfg: Arc<AppConfig>,
        now: DateTime<Utc>,
    ) -> Result<ConversationId> {
        // Store the (old) active conversation ID, so that we can restore to it,
        // if the current conversation is aborted early (e.g. because of an
        // empty query or any other error).
        let last_active_conversation_id = ws.active_conversation_id();

        // Set new active conversation if requested.
        if self.is_new() {
            let conversation = Conversation::default().with_local(self.is_local());

            let id = ws.create_conversation(conversation, cfg);
            if let Some(duration) = self.expires_in_duration()
                && let Some(mut conversation) = ws.get_conversation_mut(&id)
            {
                conversation.expires_at = chrono::Duration::from_std(duration)
                    .ok()
                    .and_then(|v| id.timestamp().checked_add_signed(v));
            }

            debug!(
                id = id.to_string(),
                local = self.is_local(),
                expires_in = self.expires_in_duration().map_or_else(
                    || "when inactive".to_owned(),
                    |v| humantime::format_duration(v).to_string()
                ),
                "Creating new active conversation due to --new flag."
            );

            ws.set_active_conversation_id(id, now)?;
        }

        Ok(last_active_conversation_id)
    }

    // Open the editor for the query, if requested.
    fn edit_message(
        &self,
        request: &mut ChatRequest,
        stream: &mut ConversationStream,
        piped: bool,
        config: &AppConfig,
        conversation_root: &Utf8Path,
    ) -> Result<(Option<Utf8PathBuf>, PartialAppConfig)> {
        // If there is no query provided, but the user explicitly requested not
        // to open the editor, we populate the query with a default message,
        // since most LLM providers do not support empty queries.
        //
        // See `force_no_edit` why this can be useful.
        if request.is_empty() && self.force_no_edit() {
            // If the last event in the stream is a `ChatRequest`, we don't add
            // anything, and simply "replay" the last message in the
            // conversation.
            //
            // Otherwise we add a default "continue" message.
            if let Some(last) = stream.pop_if(ConversationEvent::is_chat_request)
                && let Some(req) = last.into_inner().into_chat_request()
            {
                *request = req;
            } else {
                "continue".clone_into(request);
            }
        }

        // If a query is provided, and editing is not explicitly requested, or
        // in addition to the query, stdin contains data, we omit opening the
        // editor.
        if (self.query.as_ref().is_some_and(|v| !v.is_empty()) || !piped)
            && !self.force_edit()
            && !request.is_empty()
        {
            return Ok((None, PartialAppConfig::empty()));
        }

        let editor = match config.editor.command() {
            None if !request.is_empty() => return Ok((None, PartialAppConfig::empty())),
            None => return Err(Error::MissingEditor),
            Some(cmd) => cmd,
        };

        let (content, query_file, editor_provided_config) = editor::edit_query(
            config,
            conversation_root,
            stream,
            request.as_str(),
            editor,
            None,
        )?;
        request.content = content;

        Ok((Some(query_file), editor_provided_config))
    }

    /// Handle a single turn of conversation with the LLM.
    #[expect(clippy::too_many_arguments)]
    async fn handle_turn(
        &self,
        cfg: &AppConfig,
        signals: &SignalRx,
        mcp_client: &jp_mcp::Client,
        root: Utf8PathBuf,
        is_tty: bool,
        attachments: &[Attachment],
        workspace: &mut Workspace,
        tool_choice: ToolChoice,
        tools: &[ToolDefinition],
        conversation_id: ConversationId,
        printer: Arc<Printer>,
        chat_request: ChatRequest,
    ) -> Result<()> {
        let model_id = cfg
            .assistant
            .model
            .id
            .finalize(&cfg.providers.llm.aliases)?;
        let provider: Arc<dyn jp_llm::Provider> = Arc::from(provider::get_provider(
            model_id.provider,
            &cfg.providers.llm,
        )?);
        let model = provider.model_details(&model_id.name).await?;

        // Build docs map from the resolved definitions for describe_tools.
        let docs_map: IndexMap<String, ToolDocs> = tools
            .iter()
            .map(|t| (t.name.clone(), t.docs.clone()))
            .collect();
        let builtin_executors =
            BuiltinExecutors::new().register("describe_tools", DescribeTools::new(docs_map));
        let executor_source = TerminalExecutorSource::new(builtin_executors, tools);
        let tool_coordinator =
            ToolCoordinator::new(cfg.conversation.tools.clone(), Box::new(executor_source));
        let prompt_backend = Arc::new(TerminalPromptBackend);

        run_turn_loop(
            provider,
            &model,
            cfg,
            signals,
            mcp_client,
            &root,
            is_tty,
            attachments,
            workspace,
            tool_choice,
            tools,
            conversation_id,
            printer,
            prompt_backend,
            tool_coordinator,
            chat_request,
        )
        .await
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

    #[must_use]
    fn is_local(&self) -> bool {
        self.local || self.new_local_conversation
    }

    #[must_use]
    fn is_new(&self) -> bool {
        self.new_conversation || self.new_local_conversation
    }

    #[must_use]
    fn expires_in_duration(&self) -> Option<Duration> {
        self.expires_in?
            .map(Duration::from)
            .or_else(|| Some(Duration::new(0, 0)))
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
        workspace: Option<&Workspace>,
        mut partial: PartialAppConfig,
        merged_config: Option<&PartialAppConfig>,
    ) -> std::result::Result<PartialAppConfig, Box<dyn std::error::Error + Send + Sync>> {
        let Self {
            model,
            template: _,
            schema: _,
            replay: _,
            new_conversation: _,
            new_local_conversation: _,
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
            tools,
            no_tools,
            reasoning,
            no_reasoning,
            expires_in: _,
        } = &self;

        apply_model(&mut partial, model.as_deref(), merged_config);
        apply_editor(&mut partial, edit.as_ref().map(|v| v.as_ref()), *no_edit);

        // Inject builtin tool configs before tool-enable processing.
        for (name, config) in tool::builtins::all() {
            partial
                .conversation
                .tools
                .tools
                .entry(name)
                .or_insert(config);
        }

        apply_enable_tools(&mut partial, tools, no_tools, merged_config)?;
        apply_tool_use(
            &mut partial,
            tool_use.as_ref().map(|v| v.as_deref()),
            *no_tool_use,
        )?;
        apply_attachments(&mut partial, attachments, workspace)?;
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
        if self.is_new() {
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

/// Build the sorted list of system prompt sections from assistant config.
///
/// Used by both [`build_thread`] and [`LlmInquiryBackend`] construction
/// to ensure the inquiry backend sees the same sections as the main thread.
///
/// [`LlmInquiryBackend`]: crate::cmd::query::tool::inquiry::LlmInquiryBackend
pub(super) fn build_sections(
    assistant: &AssistantConfig,
    has_tools: bool,
) -> Vec<jp_config::assistant::sections::SectionConfig> {
    let mut sections: Vec<_> = assistant.system_prompt_sections.to_vec();
    sections.extend(
        assistant
            .instructions
            .iter()
            .map(jp_config::assistant::instructions::InstructionsConfig::to_section),
    );

    if has_tools {
        let tool_section = InstructionsConfig::default()
            .with_title("Tool Usage")
            .with_description("How to leverage the tools available to you.".to_string())
            .with_item("Use all the tools available to you to give the best possible answer.")
            .with_item("Verify the tool name, description and parameters are correct.")
            .with_item(
                "Even if you've reasoned yourself towards a solution, use any available tool to \
                 verify your answer.",
            )
            .to_section();

        sections.push(tool_section);
    }

    sections.sort_by_key(|s| s.position);
    sections
}

fn build_thread(
    events: ConversationStream,
    attachments: Vec<Attachment>,
    assistant: &AssistantConfig,
    has_tools: bool,
) -> Result<Thread> {
    let sections = build_sections(assistant, has_tools);

    let mut thread_builder = ThreadBuilder::default()
        .with_sections(sections)
        .with_attachments(attachments)
        .with_events(events);

    if let Some(system_prompt) = assistant.system_prompt.clone() {
        thread_builder = thread_builder.with_system_prompt(system_prompt);
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
    // A bare `--tools` (None) means "enable all". Named `--tools foo`
    // entries are collected separately so they can override `Explicit`
    // tools even when `enable_all` is active.
    let enable_all = !tools.is_empty() && tools.iter().any(Option::is_none);
    let named_tools: Vec<&str> = tools.iter().filter_map(|v| v.as_deref()).collect();

    let disable_all = !no_tools.is_empty() && no_tools.iter().any(Option::is_none);
    let named_no_tools: Vec<&str> = no_tools.iter().filter_map(|v| v.as_deref()).collect();

    let has_tools = enable_all || !named_tools.is_empty();
    let has_no_tools = disable_all || !named_no_tools.is_empty();

    if enable_all && disable_all {
        return Err("cannot pass both --no-tools and --tools without arguments".into());
    }

    let existing_tools = merged_config.map_or(&partial.conversation.tools.tools, |v| {
        &v.conversation.tools.tools
    });

    let missing = named_tools
        .iter()
        .chain(named_no_tools.iter())
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
    // Skip tools with Enable::Always (core tools cannot be disabled).
    if disable_all {
        partial
            .conversation
            .tools
            .tools
            .iter_mut()
            .filter(|(_, v)| !v.enable.is_some_and(Enable::is_always))
            .for_each(|(_, v)| v.enable = Some(Enable::Off));

    // Enable all tools first if all tools are to be enabled, but skip
    // tools that require explicit activation.
    } else if enable_all {
        partial
            .conversation
            .tools
            .tools
            .iter_mut()
            .filter(|(_, v)| !v.enable.is_some_and(Enable::is_explicit))
            .for_each(|(_, v)| v.enable = Some(Enable::On));
    }

    // Then enable individually named tools. This activates even `Explicit`
    // tools, since the user is naming them specifically.
    if has_tools {
        partial
            .conversation
            .tools
            .tools
            .iter_mut()
            .filter(|(name, _)| named_tools.iter().any(|v| v == name))
            .for_each(|(_, v)| v.enable = Some(Enable::On));
    }

    // And finally disable individually named tools.
    // Error if trying to disable a core tool.
    if has_no_tools {
        for name in &named_no_tools {
            if let Some(tool) = partial.conversation.tools.tools.get(*name)
                && tool.enable.is_some_and(Enable::is_always)
            {
                return Err(
                    format!("Tool '{name}' is a system tool and cannot be disabled").into(),
                );
            }
        }

        partial
            .conversation
            .tools
            .tools
            .iter_mut()
            .filter(|(name, _)| named_no_tools.iter().any(|v| v == name))
            .for_each(|(_, v)| v.enable = Some(Enable::Off));
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
                .filter(|(_, cfg)| cfg.enable.is_some_and(Enable::is_on))
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
fn apply_attachments(
    partial: &mut PartialAppConfig,
    attachments: &[AttachmentUrlOrPath],
    workspace: Option<&Workspace>,
) -> Result<()> {
    let root = workspace.map(Workspace::root);
    let attachments = attachments
        .iter()
        .map(|v| v.parse(root))
        .collect::<Result<Vec<_>>>()?;

    partial
        .conversation
        .attachments
        .extend(attachments.into_iter().map(Into::into));

    Ok(())
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
    query_file_path: Option<&Utf8Path>,
) -> Result<()> {
    let conversation_id = ctx.workspace.active_conversation_id();

    info!("Query is empty, exiting.");
    if last_active_conversation_id != conversation_id {
        ctx.workspace
            .set_active_conversation_id(last_active_conversation_id, ctx.now())?;
        ctx.workspace.remove_conversation(&conversation_id)?;
    }

    if let Some(path) = query_file_path {
        fs::remove_file(path)?;
    }

    ctx.printer.println("Query is empty, ignoring.");
    Ok(())
}

/// Parse a schema string as either a concise DSL or raw JSON Schema.
#[expect(clippy::needless_pass_by_value)]
fn parse_schema(s: String) -> Result<schemars::Schema> {
    crate::schema::parse_schema_dsl(&s)
        .map_err(|e| Error::Schema(e.to_string()))?
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

#[cfg(test)]
#[path = "query_tests.rs"]
mod tests;
