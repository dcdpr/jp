//! Query command implementation using the stream pipeline architecture.
//!
//! # Architecture Overview
//!
//! The query command handles conversational interactions with LLMs.
//! It uses a component-based architecture with clear separation of concerns.
//!
//! # Key Components
//!
//! - [`TurnCoordinator`]: State machine managing the turn lifecycle (Idle →
//!   Streaming → Executing → Complete/Aborted).
//!
//! - [`EventBuilder`]: Accumulates streamed chunks by index and produces
//!   complete [`ConversationEvent`]s on flush.
//!
//! - [`ChatRenderer`]: Renders LLM output (reasoning and messages) to the
//!   terminal with display mode support.
//!
//! - [`StreamRetryState`]: Single source of truth for stream retry logic
//!   (backoff, notification, state flushing).
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
//! [`ChatRenderer`]: crate::render::ChatRenderer
//! [`ConversationEvent`]: jp_conversation::event::ConversationEvent
//! [`EventBuilder`]: jp_llm::event_builder::EventBuilder
//! [`InterruptHandler`]: interrupt::handler::InterruptHandler
//! [`StreamRetryState`]: stream::retry::StreamRetryState
//! [`ToolCallRequest`]: jp_conversation::event::ToolCallRequest
//! [`ToolCallResponse`]: jp_conversation::event::ToolCallResponse
//! [`TurnCoordinator`]: turn::coordinator::TurnCoordinator

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
use clap::{ArgAction, builder::TypedValueParser as _};
use indexmap::IndexMap;
use jp_attachment::Attachment;
use jp_config::{
    AppConfig, PartialAppConfig, PartialConfig as _,
    assignment::{AssignKeyValue as _, KvAssignment},
    assistant::{
        AssistantConfig, instructions::InstructionsConfig, sections::SectionConfig,
        tool_choice::ToolChoice,
    },
    conversation::{
        ConversationConfig,
        tool::{
            Enable, ToolSource,
            access::{AccessConfig, PartialAccessConfig, PartialFsRuleConfig},
        },
    },
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
        InvocationContext, ToolDefinition, ToolDocs,
        builtin::{BuiltinExecutors, describe_tools::DescribeTools},
        tool_definitions,
    },
};
use jp_printer::Printer;
use jp_task::task::TitleGeneratorTask;
use jp_workspace::{ConversationHandle, ConversationLock, Workspace};
use minijinja::{Environment, UndefinedBehavior};
use tool::{TerminalExecutorSource, ToolCoordinator};
use tracing::{debug, trace, warn};
use turn_loop::run_turn_loop;

use super::{
    ConversationLoadRequest, Output, attachment::load_conversation_attachments,
    conversation_id::FlagIds, lock::LockOutcome,
};
use crate::{
    Ctx, PATH_STRING_PREFIX,
    access::{
        approvals::{APPROVALS_FILE, ApprovalLookup, ApprovalStore},
        compile::{ApprovalDecision, compile_policy},
        mount::{MountMode, MountSpec},
    },
    cmd::{
        self,
        conversation::fork,
        lock::{LockRequest, acquire_lock},
    },
    ctx::IntoPartialAppConfig,
    editor::{self, Editor},
    error::{Error, Result},
    output::print_json,
    parser::AttachmentUrlOrPath,
    render::TurnView,
    signals::SignalRx,
};

type BoxedResult<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Default, clap::Args)]
pub(crate) struct Query {
    /// The query to send.
    /// If not provided, uses `$JP_EDITOR`, `$VISUAL` or `$EDITOR` to open edit
    /// the query in an editor.
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
    /// \-s 'summary' → single string field -s 'name, age int, bio' → mixed
    /// types -s 'summary: a brief summary' → field with description
    ///
    /// See: <https://jp.computer/rfd/030-schema-dsl>
    #[arg(short = 's', long, value_parser = string_or_path.try_map(parse_schema))]
    schema: Option<schemars::Schema>,

    /// Replay the last message in the conversation.
    ///
    /// If a query is provided, it will be appended to the end of the previous
    /// message.
    /// If no query is provided, $EDITOR will open with the last message in the
    /// conversation.
    #[arg(long = "replay", conflicts_with = "new")]
    replay: bool,

    #[command(flatten)]
    target: FlagIds<false, false>,

    /// Fork the session's active conversation (or the one specified by --id)
    /// and start a new turn on the fork.
    ///
    /// If N is given, the fork keeps only the last N turns.
    #[arg(
        long = "fork",
        num_args = 0..=1,
        default_missing_value = "",
        value_parser = parse_fork_turns,
        conflicts_with = "new",
    )]
    fork: Option<Option<usize>>,

    /// Start a new conversation without any message history.
    #[arg(short = 'n', long = "new", group = "new", conflicts_with = "id")]
    new_conversation: bool,

    /// Store the conversation locally, outside of the workspace.
    #[arg(
        short = 'l',
        long = "local",
        requires = "new_conversation",
        conflicts_with = "no_local"
    )]
    local: bool,

    /// Store the conversation in the current workspace.
    #[arg(
        short = 'L',
        long = "no-local",
        requires = "new_conversation",
        conflicts_with = "local"
    )]
    no_local: bool,

    /// Add attachment to the configuration.
    #[arg(short = 'a', long = "attachment", alias = "attach")]
    attachments: Vec<AttachmentUrlOrPath>,

    /// Whether and how to edit the query.
    ///
    /// Setting this flag to `true`, omitting it, or using it as a boolean flag
    /// (e.g.
    /// `--edit`) will use the default editor configured elsewhere, or return an
    /// error if no editor is configured and one is required.
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

    /// Pre-fill the editor with the last assistant message quoted as a markdown
    /// blockquote (each line prefixed with ` >  `).
    ///
    /// Useful for inline replies: open `$EDITOR` with the assistant's last
    /// response pre-quoted, then intersperse your replies between the quoted
    /// lines (mutt/email style).
    /// The complete buffer — quotes plus your replies — becomes your next
    /// message.
    ///
    /// Forces the editor open by default; respects `--no-edit` / `--edit=false`
    /// if explicitly suppressed, in which case the quoted text is sent as-is.
    /// Composes with `--replay`: the quote is taken from the stream *after* the
    /// replayed turn has been trimmed, i.e. the assistant message preceding the
    /// turn being replayed.
    ///
    /// If no prior assistant message exists in this conversation, a warning is
    /// emitted and the editor opens with whatever other content was seeded
    /// (query, stdin, or empty).
    #[arg(long = "quote")]
    quote: bool,

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

    #[command(flatten)]
    tool_directives: ToolDirectives,

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

    /// Set a custom title for the conversation.
    ///
    /// Applied to the resolved conversation (new, forked, or resumed) before
    /// the turn runs.
    /// Skips title auto-generation for new conversations — your title wins.
    /// Mutually exclusive with `--no-title`.
    #[arg(long = "title", conflicts_with = "no_title")]
    title: Option<String>,

    /// Disable the title for the conversation.
    ///
    /// Clears any existing title on the resolved conversation (new, forked, or
    /// resumed) and skips auto-generation for this run.
    /// Mutually exclusive with `--title`.
    #[arg(long = "no-title", conflicts_with = "title")]
    no_title: bool,

    /// The tool to use.
    ///
    /// If a value is provided, the tool matching the value will be used.
    ///
    /// Note that this setting is *not* persisted across queries.
    /// To persist tool choice behavior, set the `assistant.tool_choice` field
    /// in a configuration file.
    #[arg(short = 'u', long = "tool-use")]
    tool_use: Option<Option<String>>,

    /// Disable tool use by the assistant.
    #[arg(short = 'U', long = "no-tool-use")]
    no_tool_use: bool,

    /// Compact the conversation before querying.
    #[command(flatten)]
    compact: crate::cmd::compact_flag::CompactFlag,

    /// Mount an external path into the workspace as a symlink and grant the
    /// assistant access to it.
    ///
    /// Form: `[TOOL:]NAME=PATH[:MODE]`.
    /// `NAME` is the workspace-relative location for the symlink, `PATH` is the
    /// external target, and `MODE` is `ro` (default) or `rw`.
    /// `rw` requires a `TOOL:` prefix; without a `TOOL:` prefix the grant
    /// applies to all enabled local tools.
    /// Repeat the flag to mount several paths.
    #[arg(long = "mount", value_name = "[TOOL:]NAME=PATH[:MODE]", action = ArgAction::Append)]
    mount: Vec<String>,
}

impl Query {
    #[expect(clippy::too_many_lines)]
    pub(crate) async fn run(self, ctx: &mut Ctx, handle: Option<ConversationHandle>) -> Output {
        debug!("Running `query` command.");
        trace!(args = ?self, "Received arguments.");
        let now = ctx.now();
        let cfg = ctx.config();

        // Resolve the target conversation and acquire an exclusive lock.
        //
        // Three paths:
        // 1. --new: create a fresh conversation (already locked).
        // 2. --fork/--id/session: resolve an existing conversation, lock it.
        // 3. Lock contention: user picks "new" or "fork" from the prompt.
        let lock = self.acquire_lock(ctx, handle).await?;

        // Create symlinks and seed approvals for any `--mount` flags before the
        // turn runs, so tools can reach the mounted paths.
        create_mount_effects(&self.mount, &ctx.workspace, ctx.fs_backend.as_deref(), now)?;

        // The two flags are mutually exclusive (enforced by clap), and the
        // resolved conversation may be new, freshly forked (which clones the
        // source's metadata, including any title), or resumed.
        apply_title_override(&lock, self.title.as_deref(), self.no_title);

        // Record this conversation as the session's active conversation.
        if let Some(session) = &ctx.session
            && let Err(error) = ctx
                .workspace
                .activate_session_conversation(&lock, session, now)
        {
            warn!(%error, "Failed to record activation.");
        }

        if let Some(delta) = get_config_delta_from_cli(&cfg, &lock)? {
            lock.as_mut()
                .update_events(|events| events.add_config_delta(delta));
        }

        // Compact the conversation before querying, if requested.
        if self.compact.should_compact() {
            self.apply_pre_query_compaction(&lock, &cfg).await?;
        }

        let mut mcp_servers_handle = ctx.configure_active_mcp_servers().await?;

        let (conv_title, is_local) = {
            let m = lock.metadata();
            (m.title.clone(), m.user)
        };

        // Show conversation identity in the terminal title.
        if ctx.term.is_tty {
            set_terminal_title(lock.id(), conv_title.as_deref());
        }

        let cid = lock.id();
        let conversation_path = ctx.fs_backend.as_deref().map_or_else(
            || {
                ctx.workspace
                    .root()
                    .join(cid.to_dirname(conv_title.as_deref()))
            },
            |fs| fs.build_conversation_dir(&cid, conv_title.as_deref(), is_local),
        );

        let (query_file, mut editor_provided_config, chat_request) = lock
            .as_mut()
            .update_events(|stream| self.build_conversation(stream, &cfg, &conversation_path))?;

        let Some(mut chat_request) = chat_request else {
            // Empty query, early exit. Auto-persist happens on lock drop.
            if let Some(path) = query_file.as_deref() {
                fs::remove_file(path)?;
            }
            ctx.printer.println("Query is empty, ignoring.");
            return Ok(());
        };

        // Stamp the request with the configured user name so transcripts
        // attribute each turn correctly even when teammates with different
        // local configs continue the conversation. `None` falls back to a
        // generic label at render time.
        chat_request.author = cfg.user.name.clone();

        // If a schema is provided, set it on the ChatRequest so the
        // provider uses its native structured output API.
        if let Some(schema) = &self.schema {
            chat_request.schema = schema.as_object().cloned();
        }

        // If the query was composed in an editor, the user has lost sight
        // of what they wrote by the time the editor closes. Echo it back
        // through the same role-aware rendering machinery used by replay
        // and live streaming — a labeled user header followed by the
        // request body — so the boundary between user input and the
        // forthcoming assistant response is visually clear. Render this
        // before any post-edit work (MCP init, attachments, tools) so that
        // failures in those stages don't swallow the user's message.
        if query_file.is_some() {
            let mut echo = TurnView::new(
                ctx.printer.clone(),
                cfg.style.clone(),
                cfg.assistant.name.clone(),
                Some(cfg.assistant.model.id.resolved().to_string()),
            );
            echo.render_user_request(&chat_request);
        }

        if !editor_provided_config.is_empty() {
            // Resolve any model aliases before storing in the stream so
            // that per-event configs always contain concrete model IDs.
            editor_provided_config.resolve_model_aliases(&cfg.providers.llm.aliases);
            lock.as_mut()
                .update_events(|events| events.add_config_delta(editor_provided_config));
        }

        let stream = lock.events().clone();

        // Set the title for new or empty conversations (including forks).
        // Skip when `--title` or `--no-title` was provided (the user already
        // expressed an intent for the title).
        //
        // A leading markdown heading in the prompt is used verbatim as the
        // title, short-circuiting the LLM round-trip. Otherwise, fall back to
        // background title generation when enabled.
        if (self.is_new() || self.fork.is_some() || stream.is_empty())
            && ctx.term.args.persist
            && self.title.is_none()
            && !self.no_title
        {
            match resolve_new_title(
                cfg.conversation.title.from_heading,
                cfg.conversation.title.generate.auto,
                &chat_request.content,
            ) {
                NewTitle::FromHeading(title) => {
                    debug!("Using leading markdown heading as conversation title");
                    lock.as_mut()
                        .update_metadata(|m| m.title = Some(title.clone()));
                    if ctx.term.is_tty {
                        jp_term::osc::set_title(format!("{cid}: {title}"));
                    }
                }
                NewTitle::Generate => {
                    debug!("Generating title for new conversation");
                    let mut stream = stream.clone();
                    stream.start_turn(chat_request.clone());
                    ctx.task_handler.spawn(TitleGeneratorTask::new(
                        cid,
                        stream,
                        &cfg,
                        ctx.term.is_tty,
                    )?);
                }
                NewTitle::Skip => {}
            }
        }

        // Wait for all MCP servers to finish loading.
        while let Some(result) = mcp_servers_handle.join_next().await {
            result??;
        }

        let forced_tool = cfg.assistant.tool_choice.function_name();
        let tools =
            tool_definitions(cfg.conversation.tools.iter(), &ctx.mcp_client, forced_tool).await?;

        let attachment_urls: Vec<_> = cfg
            .conversation
            .attachments
            .iter()
            .map(jp_config::conversation::attachment::AttachmentConfig::to_url)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let attachments = load_conversation_attachments(ctx, attachment_urls).await?;

        debug!(count = attachments.len(), "Attachments loaded.");

        let thread = build_thread(stream, attachments, &cfg.assistant, !tools.is_empty())?;
        let root = ctx.workspace.root().to_path_buf();
        let approvals = Arc::new(load_approval_store(ctx.fs_backend.as_deref()));

        // Sanitize any structural issues (orphaned tool calls, missing
        // user messages, etc.) before sending the stream to the provider.
        lock.as_mut().update_events(ConversationStream::sanitize);

        let invocation = InvocationContext {
            workspace_id: ctx.workspace.id().to_string(),
            conversation_id: lock.id().to_string(),
        };

        let turn_result = self
            .handle_turn(
                &cfg,
                &ctx.signals.receiver,
                &ctx.mcp_client,
                root,
                ctx.term.is_tty,
                &thread.attachments,
                &lock,
                cfg.assistant.tool_choice.clone(),
                &tools,
                ctx.printer.clone(),
                approvals,
                chat_request,
                invocation,
            )
            .await
            .map_err(|error| cmd::Error::from(error).with_persistence(true));

        // Extract structured data from the conversation after the turn.
        if self.schema.is_some() && turn_result.is_ok() {
            let data = lock.events().iter().rev().find_map(|e| {
                e.as_chat_response()
                    .and_then(ChatResponse::as_structured_data)
                    .cloned()
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

    /// Declare what conversations this command needs.
    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        if self.is_new() {
            return ConversationLoadRequest::none();
        }

        ConversationLoadRequest::explicit_or_session_with_config(&self.target)
    }

    /// Build the chat request for this query.
    ///
    /// Returns the editor details and the [`ChatRequest`], if non-empty.
    /// The request is **not** added to the stream — that is the responsibility
    /// of [`TurnCoordinator::start_turn`].
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

        // If --quote is set, prepend the last assistant message as a markdown
        // blockquote so it sits at the top of the editor buffer. The user can
        // then intersperse replies between the quoted lines (mutt-style inline
        // reply). Missing message (e.g. brand new conversation) degrades to a
        // warning and the editor opens with whatever else was seeded.
        if self.quote {
            if let Some(message) = last_assistant_message(stream) {
                let quoted = blockquote(message);
                *chat_request = format!("{quoted}\n\n{chat_request}");
            } else {
                warn!("--quote: no prior assistant message in this conversation");
            }
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

    /// Create a new conversation and return an exclusive lock.
    fn create_new_conversation(&self, ctx: &mut Ctx) -> Result<ConversationLock> {
        let cfg = ctx.config();
        let ws = &mut ctx.workspace;

        let conversation = Conversation::default().with_local(self.is_local(&cfg.conversation));
        let lock =
            ws.create_and_lock_conversation(conversation, cfg.clone(), ctx.session.as_ref())?;
        let id = lock.id();

        if let Some(duration) = self.expires_in_duration() {
            let mut conv = lock.as_mut();
            conv.update_metadata(|m| {
                m.expires_at = chrono::Duration::from_std(duration)
                    .ok()
                    .and_then(|v| id.timestamp().checked_add_signed(v));
            });
            conv.flush()?;
        }

        debug!(
            id = id.to_string(),
            local = self.is_local(&cfg.conversation),
            expires_in = self.expires_in_duration().map_or_else(
                || "when inactive".to_owned(),
                |v| humantime::format_duration(v).to_string()
            ),
            "Creating new conversation."
        );

        Ok(lock)
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
        lock: &ConversationLock,
        tool_choice: ToolChoice,
        tools: &[ToolDefinition],
        printer: Arc<Printer>,
        approvals: Arc<ApprovalStore>,
        chat_request: ChatRequest,
        invocation: InvocationContext,
    ) -> Result<()> {
        let model_id = cfg.assistant.model.id.resolved();
        let provider: Arc<dyn jp_llm::Provider> = Arc::from(provider::get_provider(
            model_id.provider,
            &cfg.providers.llm,
        )?);
        debug!(model = %model_id, "Fetching model details.");
        let model = provider.model_details(&model_id.name).await?;
        debug!(model = model.name(), "Model details resolved.");

        // Build docs map from the resolved definitions for describe_tools.
        let docs_map: IndexMap<String, ToolDocs> = tools
            .iter()
            .map(|t| (t.name.clone(), t.docs.clone()))
            .collect();
        let builtin_executors =
            BuiltinExecutors::new().register("describe_tools", DescribeTools::new(docs_map));
        let executor_source =
            TerminalExecutorSource::new(builtin_executors, tools, approvals, invocation.clone());
        let tool_coordinator =
            ToolCoordinator::new(cfg.conversation.tools.clone(), Box::new(executor_source))
                .with_interrupt(cfg.interrupt.tool_call.clone());
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
            lock,
            tool_choice,
            tools,
            printer,
            prompt_backend,
            tool_coordinator,
            chat_request,
            invocation,
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
    /// This means the `--edit` flag was provided (but not `--edit=false`), or
    /// `--quote` was provided (which implies editing).
    /// In either case the editor should be opened, regardless of whether a
    /// query is provided as an argument.
    fn force_edit(&self) -> bool {
        !self.force_no_edit() && (self.edit.is_some() || self.quote)
    }

    #[must_use]
    fn is_local(&self, cfg: &ConversationConfig) -> bool {
        (self.local || cfg.start_local) && !self.no_local
    }

    #[must_use]
    fn is_new(&self) -> bool {
        self.new_conversation
    }

    #[must_use]
    fn expires_in_duration(&self) -> Option<Duration> {
        self.expires_in?
            .map(Duration::from)
            .or_else(|| Some(Duration::new(0, 0)))
    }

    /// Apply compaction before the query turn starts.
    ///
    /// Applies all compaction rules from the resolved config and appends the
    /// compaction events to the conversation.
    async fn apply_pre_query_compaction(
        &self,
        lock: &ConversationLock,
        cfg: &AppConfig,
    ) -> Result<()> {
        let events = lock.events().clone();

        // The inline DSL plan never enters the config; assemble the effective
        // rules from the resolved config rules plus any `-k SPEC` here.
        let rules = self
            .compact
            .effective_rules(&cfg.conversation.compaction.rules)
            .map_err(|e| Error::Compaction(e.to_string()))?;

        let compactions = super::conversation::compact::build_compaction_events(
            &events,
            cfg,
            &rules,
            super::conversation::compact::Bound::Default,
            super::conversation::compact::Bound::Default,
            // `--compact` on a query is a quick adjunct; apply it silently so
            // compaction details don't clutter the query output.
            None,
        )
        .await?;

        super::conversation::compact::apply_compactions(&lock.as_mut(), compactions);

        Ok(())
    }

    async fn acquire_lock(
        &self,
        ctx: &mut Ctx,
        handle: Option<ConversationHandle>,
    ) -> Result<ConversationLock> {
        // Handle --new: create a fresh conversation.
        if self.is_new() {
            return self.create_new_conversation(ctx);
        }

        let handle = handle.ok_or(Error::NoConversationTarget)?;

        // Handle --fork: fork the conversation before locking.
        if let Some(fork_turns) = &self.fork {
            return fork_conversation(ctx, &handle, *fork_turns);
        }

        let req = LockRequest::from_ctx(handle, ctx)
            .allow_new(true)
            .allow_fork(true);

        match acquire_lock(req).await? {
            LockOutcome::Acquired(lock) => Ok(lock),
            LockOutcome::NewConversation => self.create_new_conversation(ctx),
            LockOutcome::ForkConversation(handle) => fork_conversation(ctx, &handle, None),
        }
    }
}

/// Return the most recent assistant message text in the stream.
///
/// Walks the stream in reverse and returns the first `ChatResponse::Message` it
/// encounters.
/// Reasoning, structured-data responses, and tool calls are skipped.
fn last_assistant_message(stream: &ConversationStream) -> Option<&str> {
    stream
        .iter()
        .rev()
        .filter_map(|e| e.event.as_chat_response())
        .find_map(|r| r.as_message())
}

/// Prefix each line of `text` with ` >  ` for use as a markdown blockquote.
///
/// Empty lines are emitted as just `>` (no trailing space) so the blockquote
/// stays visually continuous across paragraph breaks while avoiding
/// trailing-whitespace warnings in editors.
fn blockquote(text: &str) -> String {
    text.lines()
        .map(|line| {
            if line.is_empty() {
                ">".to_owned()
            } else {
                format!("> {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A single tool selection directive from the CLI.
///
/// Directives are evaluated left-to-right, allowing users to compose tool sets
/// precisely (e.g.
/// `--no-tools --tool=write --no-tools=fs_modify_file`).
#[derive(Debug, Clone, PartialEq, Eq)]
enum ToolDirective {
    EnableAll,
    DisableAll,
    Enable(String),
    Disable(String),
}

impl ToolDirective {
    /// Returns the single-tool directive as a string slice.
    #[must_use]
    fn as_single(&self) -> Option<&str> {
        match self {
            Self::Enable(name) | Self::Disable(name) => Some(name.as_str()),
            _ => None,
        }
    }
}

/// Ordered sequence of tool directives parsed from `--tool` and `--no-tools`.
///
/// Implements manual [`clap::Args`] and [`clap::FromArgMatches`] to recover the
/// position of each flag value using [`ArgMatches::indices_of`], then merges
/// and sorts them by index into a single ordered list.
///
/// [`ArgMatches::indices_of`]: clap::ArgMatches::indices_of
#[derive(Debug, Clone, Default)]
struct ToolDirectives(Vec<ToolDirective>);

impl std::ops::Deref for ToolDirectives {
    type Target = [ToolDirective];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl clap::FromArgMatches for ToolDirectives {
    fn from_arg_matches(matches: &clap::ArgMatches) -> std::result::Result<Self, clap::Error> {
        let tool_values: Vec<String> = matches
            .get_many("tools")
            .map(|v| v.cloned().collect())
            .unwrap_or_default();
        let tool_indices: Vec<_> = matches
            .indices_of("tools")
            .map(Iterator::collect)
            .unwrap_or_default();

        let no_tool_values: Vec<String> = matches
            .get_many("no_tools")
            .map(|v| v.cloned().collect())
            .unwrap_or_default();
        let no_tool_indices: Vec<_> = matches
            .indices_of("no_tools")
            .map(Iterator::collect)
            .unwrap_or_default();

        let mut indexed = vec![];
        for (val, idx) in tool_values.into_iter().zip(tool_indices) {
            let directive = if val.is_empty() {
                ToolDirective::EnableAll
            } else {
                ToolDirective::Enable(val)
            };
            indexed.push((idx, directive));
        }

        for (val, idx) in no_tool_values.into_iter().zip(no_tool_indices) {
            let directive = if val.is_empty() {
                ToolDirective::DisableAll
            } else {
                ToolDirective::Disable(val)
            };
            indexed.push((idx, directive));
        }

        indexed.sort_by_key(|(idx, _)| *idx);
        Ok(Self(indexed.into_iter().map(|(_, d)| d).collect()))
    }

    fn update_from_arg_matches(
        &mut self,
        matches: &clap::ArgMatches,
    ) -> std::result::Result<(), clap::Error> {
        *self = Self::from_arg_matches(matches)?;
        Ok(())
    }
}

impl clap::Args for ToolDirectives {
    fn augment_args(cmd: clap::Command) -> clap::Command {
        cmd.arg(
            clap::Arg::new("tools")
                .short('t')
                .long("tool")
                .alias("tools")
                .help("The tool(s) to enable")
                .long_help(
                    "The tool(s) to enable.\n\nIf an existing tool is configured with a matching \
                     name, it will be enabled for the duration of the query.\n\nIf no arguments \
                     are provided, all configured tools will be enabled.\n\nYou can provide this \
                     flag multiple times to enable multiple tools. Flags are evaluated \
                     left-to-right, so `--no-tools --tool=write` first disables everything, then \
                     re-enables only 'write'.",
                )
                .action(ArgAction::Append)
                .num_args(0..=1)
                .default_missing_value(""),
        )
        .arg(
            clap::Arg::new("no_tools")
                .short('T')
                .long("no-tool")
                .alias("no-tools")
                .help("Disable tool(s)")
                .long_help(
                    "Disable tool(s).\n\nIf provided without a value, all enabled tools will be \
                     disabled, otherwise pass the argument multiple times to disable one or more \
                     tools.\n\nFlags are evaluated left-to-right together with `--tool`.",
                )
                .action(ArgAction::Append)
                .num_args(0..=1)
                .default_missing_value(""),
        )
    }

    fn augment_args_for_update(cmd: clap::Command) -> clap::Command {
        Self::augment_args(cmd)
    }
}

/// Fork a conversation and return the new conversation's lock.
fn fork_conversation(
    ctx: &mut Ctx,
    source: &ConversationHandle,
    fork_turns: Option<usize>,
) -> Result<ConversationLock> {
    fork::fork_conversation(ctx, source, |events| {
        if let Some(n) = fork_turns {
            events.retain_last_turns(n);
        }
    })
}

/// How a new conversation's title is set from its first prompt, before the turn
/// runs.
#[derive(Debug, PartialEq)]
enum NewTitle {
    /// Use this text, taken verbatim from a leading markdown heading.
    FromHeading(String),

    /// Generate a title in the background via the LLM.
    Generate,

    /// Leave the title unset.
    Skip,
}

/// Decide how to title a new conversation from its first prompt `content`.
///
/// A leading markdown heading wins when `from_heading` is enabled; otherwise
/// background generation is chosen when `generate_auto` is enabled.
/// The two flags are independent: disabling generation does not disable
/// heading-derived titles.
fn resolve_new_title(from_heading: bool, generate_auto: bool, content: &str) -> NewTitle {
    if from_heading && let Some(title) = jp_md::heading::leading_heading(content) {
        return NewTitle::FromHeading(title);
    }

    if generate_auto {
        return NewTitle::Generate;
    }

    NewTitle::Skip
}

/// Apply `--title` / `--no-title` to the resolved conversation.
///
/// Both flags act on `metadata.title` directly so the run ends with the title
/// the user asked for, regardless of whether the conversation is new, freshly
/// forked (which inherits the source's title), or resumed:
///
/// - `--title T` sets the title to `Some(T)`.
/// - `--no-title` clears any existing title.
/// - Neither flag is a no-op.
fn apply_title_override(lock: &ConversationLock, title: Option<&str>, no_title: bool) {
    if let Some(title) = title {
        lock.as_mut().update_metadata(|m| {
            m.title = Some(title.to_owned());
        });
    } else if no_title {
        lock.as_mut().update_metadata(|m| {
            m.title = None;
        });
    }
}

fn get_config_delta_from_cli(
    cfg: &AppConfig,
    lock: &ConversationLock,
) -> Result<Option<PartialAppConfig>> {
    let partial = lock
        .events()
        .config()
        .map(|c| c.to_partial())
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
            local: _,
            no_local: _,
            attachments,
            edit,
            no_edit,
            quote: _,
            tool_use,
            no_tool_use,
            query: _,
            parameters,
            hide_reasoning,
            hide_tool_calls,
            tool_directives,
            reasoning,
            no_reasoning,
            expires_in: _,
            target: _,
            fork: _,
            compact: _,
            title: _,
            no_title: _,
            mount,
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

        apply_enable_tools(&mut partial, tool_directives, merged_config)?;
        apply_tool_use(
            &mut partial,
            tool_use.as_ref().map(|v| v.as_deref()),
            *no_tool_use,
        )?;
        apply_attachments(&mut partial, attachments, workspace)?;
        apply_mounts(&mut partial, mount, workspace, merged_config)?;
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
        workspace: &Workspace,
        partial: PartialAppConfig,
        _: Option<&PartialAppConfig>,
        handle: &ConversationHandle,
    ) -> std::result::Result<PartialAppConfig, Box<dyn std::error::Error + Send + Sync>> {
        let config = workspace.events(handle)?.config().map(|c| c.to_partial())?;

        load_partial(partial, config).map_err(Into::into)
    }
}

/// Build the sorted list of system prompt sections from assistant config.
///
/// Used by both [`build_thread`] and [`LlmInquiryBackend`] construction to
/// ensure the inquiry backend sees the same sections as the main thread.
///
/// [`LlmInquiryBackend`]: crate::cmd::query::tool::inquiry::LlmInquiryBackend
pub(super) fn build_sections(assistant: &AssistantConfig, has_tools: bool) -> Vec<SectionConfig> {
    let mut sections: Vec<_> = assistant.system_prompt_sections.clone();
    sections.extend(
        assistant
            .instructions
            .iter()
            .map(InstructionsConfig::to_section),
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
    directives: &ToolDirectives,
    merged_config: Option<&PartialAppConfig>,
) -> BoxedResult<()> {
    if directives.is_empty() {
        return Ok(());
    }

    let existing_tools = merged_config.map_or(&partial.conversation.tools.tools, |v| {
        &v.conversation.tools.tools
    });

    // Validate all named tools exist.
    let missing: HashSet<_> = directives
        .iter()
        .filter_map(ToolDirective::as_single)
        .filter(|name| !existing_tools.contains_key(*name))
        .collect();

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

    // Validate that core tools are not disabled by name.
    for d in directives.iter() {
        if let ToolDirective::Disable(name) = d
            && let Some(tool) = partial.conversation.tools.tools.get(name.as_str())
            && tool.enable.is_some_and(Enable::is_always)
        {
            return Err(format!("Tool '{name}' is a system tool and cannot be disabled").into());
        }
    }

    // Apply directives left-to-right.
    for d in directives.iter() {
        match d {
            ToolDirective::EnableAll => {
                partial
                    .conversation
                    .tools
                    .tools
                    .iter_mut()
                    .filter(|(_, v)| !v.enable.is_some_and(Enable::is_explicit))
                    .for_each(|(_, v)| v.enable = Some(Enable::On));
            }
            ToolDirective::DisableAll => {
                partial
                    .conversation
                    .tools
                    .tools
                    .iter_mut()
                    .filter(|(_, v)| !v.enable.is_some_and(Enable::is_always))
                    .for_each(|(_, v)| v.enable = Some(Enable::Off));
            }
            ToolDirective::Enable(name) => {
                if let Some(tool) = partial.conversation.tools.tools.get_mut(name.as_str()) {
                    tool.enable = Some(Enable::On);
                }
            }
            ToolDirective::Disable(name) => {
                if let Some(tool) = partial.conversation.tools.tools.get_mut(name.as_str()) {
                    tool.enable = Some(Enable::Off);
                }
            }
        }
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

/// A resolved mount and the tools it grants access to (stage 1 planning).
struct MountPlan {
    rule_path: String,
    write: bool,
    /// (tool name, whether its `access.fs` is empty across all layers)
    targets: Vec<(String, bool)>,
}

/// Inject `--mount` access grants into the partial config (stage 1).
///
/// Pure config mutation: one `access.fs` rule per in-scope tool.
/// The symlink is not required to exist yet; it is created later in
/// [`Query::run`].
/// When a tool had no filesystem rules from any layer, a workspace-default rule
/// is also injected so the mount doesn't silently switch the tool to deny-all.
fn apply_mounts(
    partial: &mut PartialAppConfig,
    mounts: &[String],
    workspace: Option<&Workspace>,
    merged_config: Option<&PartialAppConfig>,
) -> BoxedResult<()> {
    if mounts.is_empty() {
        return Ok(());
    }

    let workspace = workspace.ok_or("`--mount` requires a workspace")?;
    let root = workspace.root().to_owned();
    let cwd = current_dir_utf8()?;

    // Resolve the tool set and the global enable default from the merged
    // config (the fully-layered view) so a bare mount expands over the tools
    // actually enabled in the resolved config, honoring `*` defaults.
    let tools_config = merged_config.map_or(&partial.conversation.tools, |v| &v.conversation.tools);
    let default_enable = tools_config.defaults.enable;
    let existing = &tools_config.tools;

    let mut plans = Vec::new();
    for spec in mounts {
        let spec = MountSpec::parse(spec)?;
        let rule_path = spec.resolve_name(&cwd, &root)?.as_str().to_owned();

        let targets = match &spec.tool {
            Some(tool) => vec![(tool.clone(), tool_access_empty(existing, tool))],
            None => existing
                .iter()
                .filter(|(_, cfg)| is_enabled_local(cfg, default_enable))
                .map(|(name, _)| (name.clone(), tool_access_empty(existing, name)))
                .collect(),
        };

        plans.push(MountPlan {
            rule_path,
            write: spec.mode == MountMode::Rw,
            targets,
        });
    }

    for plan in plans {
        for (tool, access_empty) in plan.targets {
            let cfg = partial.conversation.tools.tools.entry(tool).or_default();
            let access = cfg.access.get_or_insert_with(PartialAccessConfig::default);

            let already_present = access
                .fs
                .iter()
                .any(|rule| rule.path.as_deref() == Some(plan.rule_path.as_str()));
            if already_present {
                continue;
            }

            if access_empty && access.fs.is_empty() {
                access.fs.push(workspace_default_partial_rule());
            }

            access
                .fs
                .push(mount_partial_rule(&plan.rule_path, plan.write));
        }
    }

    Ok(())
}

/// Create the symlinks and seed the approval store for `--mount` flags (stage
/// 2).
fn create_mount_effects(
    mounts: &[String],
    workspace: &Workspace,
    fs_backend: Option<&jp_storage::backend::FsStorageBackend>,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<()> {
    if mounts.is_empty() {
        return Ok(());
    }

    let root = workspace.root().to_owned();
    let root_canonical = root.canonicalize_utf8().unwrap_or_else(|_| root.clone());
    let cwd = current_dir_utf8().map_err(|e| Error::CliConfig(e.to_string()))?;

    let approvals_path = approval_store_path(fs_backend);
    let mut store = approvals_path
        .as_deref()
        .map(ApprovalStore::load)
        .unwrap_or_default();

    let mut rules = Vec::new();
    for spec in mounts {
        let spec = MountSpec::parse(spec).map_err(|e| Error::CliConfig(e.to_string()))?;
        let rule_path = spec
            .resolve_name(&cwd, &root)
            .map_err(|e| Error::CliConfig(e.to_string()))?;
        let link = root.join(&rule_path);

        let target = expand_tilde(&spec.path, env::var("HOME").ok())
            .unwrap_or_else(|| Utf8PathBuf::from(&spec.path));

        // Resolve the target before creating the link so a missing target
        // fails cleanly instead of leaving a broken symlink behind.
        let canonical = target.canonicalize_utf8().map_err(|e| {
            Error::CliConfig(format!("mount target '{target}' cannot be resolved: {e}"))
        })?;

        // An external mount must point outside the workspace. Reject an
        // in-workspace target before any side effect, so a rejected mount
        // leaves no symlink or approval entry behind.
        if canonical.starts_with(&root_canonical) {
            return Err(Error::CliConfig(format!(
                "mount target '{target}' is inside the workspace; mounts are for external paths"
            )));
        }

        // Link to the canonical absolute target so the symlink resolves the
        // same regardless of where it sits and matches the recorded approval
        // (a relative target would resolve against the link's parent instead).
        create_workspace_symlink(&link, &canonical)?;
        store.record(rule_path.as_str(), canonical, now);
        rules.push(spec.rule(rule_path.as_str()));
    }

    if let Some(path) = approvals_path {
        store.save(&path)?;
    }

    // Compile the just-created mounts against the seeded approvals to confirm
    // they resolve to a usable policy, surfacing broken or unapproved targets.
    let access = AccessConfig { fs: rules };
    let (_, warnings) = compile_policy(&access, &root, |rule_path, candidate| {
        match store.lookup(rule_path, candidate) {
            ApprovalLookup::Approved => ApprovalDecision::Approved,
            ApprovalLookup::Retargeted { .. } | ApprovalLookup::Unknown => {
                ApprovalDecision::Rejected
            }
        }
    })
    .map_err(|e| Error::CliConfig(e.to_string()))?;

    for warning in warnings {
        warn!("{warning}");
    }

    Ok(())
}

/// Create a workspace symlink at `link` pointing to `target`.
///
/// A symlink that already resolves to the same target is a no-op; one resolving
/// elsewhere, or a non-symlink at `link`, is an error.
fn create_workspace_symlink(link: &Utf8Path, target: &Utf8Path) -> Result<()> {
    if link.is_symlink() {
        // Compare resolved targets rather than the raw link text, so a relative
        // and an absolute link to the same place are treated as identical.
        let same = link
            .canonicalize_utf8()
            .ok()
            .zip(target.canonicalize_utf8().ok())
            .is_some_and(|(existing, wanted)| existing == wanted);
        if same {
            return Ok(());
        }
        return Err(Error::CliConfig(format!(
            "mount '{link}' already exists as a symlink pointing elsewhere"
        )));
    }

    if link.exists() {
        return Err(Error::CliConfig(format!(
            "cannot create mount: '{link}' already exists and is not a symlink"
        )));
    }

    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent)?;
    }

    #[cfg(unix)]
    std::os::unix::fs::symlink(target.as_std_path(), link.as_std_path())?;

    #[cfg(windows)]
    if target.is_dir() {
        std::os::windows::fs::symlink_dir(target.as_std_path(), link.as_std_path())?;
    } else {
        std::os::windows::fs::symlink_file(target.as_std_path(), link.as_std_path())?;
    }

    Ok(())
}

/// Resolve the path to the user-local approval store, if user storage exists.
fn approval_store_path(
    fs_backend: Option<&jp_storage::backend::FsStorageBackend>,
) -> Option<Utf8PathBuf> {
    fs_backend
        .and_then(|fs| fs.user_storage_with_path(relative_path::RelativePath::new(APPROVALS_FILE)))
}

/// Load the approval store, treating missing/in-memory storage as empty.
fn load_approval_store(
    fs_backend: Option<&jp_storage::backend::FsStorageBackend>,
) -> ApprovalStore {
    approval_store_path(fs_backend)
        .as_deref()
        .map(ApprovalStore::load)
        .unwrap_or_default()
}

fn current_dir_utf8() -> BoxedResult<Utf8PathBuf> {
    let cwd = env::current_dir()?;
    Utf8PathBuf::from_path_buf(cwd)
        .map_err(|path| format!("current directory is not valid UTF-8: {}", path.display()).into())
}

/// Whether a tool's `access.fs` is empty across all merged layers.
fn tool_access_empty(
    tools: &IndexMap<String, jp_config::conversation::tool::PartialToolConfig>,
    name: &str,
) -> bool {
    tools
        .get(name)
        .and_then(|cfg| cfg.access.as_ref())
        .is_none_or(|access| access.fs.is_empty())
}

/// Whether a partial tool config is an enabled local tool.
///
/// The tool's own `enable` takes precedence over the global `*` default; a tool
/// that is `off` or `explicit` after that resolution is not part of a bare
/// mount's scope.
fn is_enabled_local(
    cfg: &jp_config::conversation::tool::PartialToolConfig,
    default_enable: Option<Enable>,
) -> bool {
    matches!(cfg.source, Some(ToolSource::Local { .. }))
        && !matches!(
            cfg.enable.or(default_enable),
            Some(Enable::Off | Enable::Explicit)
        )
}

/// The workspace-default rule injected to preserve a tool's prior implicit
/// workspace access.
fn workspace_default_partial_rule() -> PartialFsRuleConfig {
    PartialFsRuleConfig {
        path: Some(".".to_owned()),
        read: Some(true),
        write: Some(true),
        ..PartialFsRuleConfig::default()
    }
}

/// The `access.fs` rule a mount injects.
fn mount_partial_rule(rule_path: &str, write: bool) -> PartialFsRuleConfig {
    PartialFsRuleConfig {
        path: Some(rule_path.to_owned()),
        external: Some(true),
        read: Some(true),
        write: Some(write),
        ..PartialFsRuleConfig::default()
    }
}

/// Set the terminal title to show the active conversation.
fn set_terminal_title(id: ConversationId, title: Option<&str>) {
    let display = match title {
        Some(t) => format!("{id}: {t}"),
        None => id.to_string(),
    };
    jp_term::osc::set_title(display);
}

/// Parse a schema string as either a concise DSL or raw JSON Schema.
#[expect(clippy::needless_pass_by_value)]
fn parse_schema(s: String) -> Result<schemars::Schema> {
    crate::schema::parse_schema_dsl(&s)
        .map_err(|e| Error::Schema(e.to_string()))?
        .try_into()
        .map_err(Into::into)
}

/// Parse the `--fork` value.
/// Empty string means "all turns", a number means "keep last N turns".
fn parse_fork_turns(s: &str) -> std::result::Result<Option<usize>, String> {
    if s.is_empty() {
        return Ok(None);
    }
    s.parse::<usize>()
        .map(Some)
        .map_err(|_| format!("expected a positive integer, got '{s}'"))
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
