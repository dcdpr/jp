//! Starting a turn: gathering what it needs, then running it.
//!
//! [`TurnInputs::collect`] reads the CLI context; [`TurnInputs::run`] waits for
//! the MCP servers, assembles the thread, and hands the turn to
//! [`Query::run_turn`].

use std::{sync::Arc, time::Instant};

use camino::Utf8PathBuf;
use indexmap::IndexMap;
use jp_attachment::Attachment;
use jp_config::{
    AppConfig, assistant::tool_choice::ToolChoice,
};
use jp_conversation::{ConversationStream, EventId, event::ChatRequest};
use jp_inquire::prompt::TerminalPromptBackend;
use jp_llm::{DynProvider, provider};
use jp_mcp::{
    StartupSet,
    server::{
        CommandPlugins, PluginInit,
        builtin::{BuiltinExecutors, describe_tools::DescribeTools},
        tool_definitions,
    },
};
use jp_printer::Printer;
use jp_process::SystemProcessRunner;
use jp_tool::{InvocationContext, ToolDefinition, ToolDocs};
use jp_workspace::{ConversationLock, Id as WorkspaceId};
use tracing::{debug, info, warn};
use url::Url;

use super::{
    PendingStreamTrim, Query,
    interrupt::{InterruptAction, TurnInterrupts},
    mcp_startup,
    mounts::load_approval_store,
    thread::build_thread,
    tool::{TerminalExecutorSource, ToolCoordinator},
    turn::TurnEnding,
    turn_loop::run_turn_loop,
};
use crate::{
    Ctx,
    access::approvals::ApprovalStore,
    cmd::{
        attachment::{attachment_url, load_conversation_attachments},
        plugin::{
            dispatch::well_known_paths,
            tool::{admit_turn_plugins, without_unadmitted_plugins},
        },
    },
    error::{Error, Result},
    signals::{SignalRouter, TurnInterrupt},
};

impl Query {
    /// Run one turn of a conversation against the LLM.
    ///
    /// Takes no CLI state: everything it needs is an argument, so a caller that
    /// isn't the `query` command can drive a turn too.
    ///
    /// The `lock` is proof the conversation is held for the duration (RFD 020),
    /// and it owns what it needs, so nothing here borrows the workspace.
    ///
    /// Returns how the turn ended, including a stop or an abort that arrived
    /// before its request was appended.
    #[expect(clippy::too_many_arguments)]
    pub(crate) async fn run_turn(
        cfg: &AppConfig,
        provider: Arc<dyn DynProvider>,
        signals: &SignalRouter,
        mcp_client: &jp_mcp::Client,
        root: Utf8PathBuf,
        interactive: bool,
        attachments: &[Attachment],
        lock: &ConversationLock,
        tool_choice: ToolChoice,
        tools: &[ToolDefinition],
        printer: Arc<Printer>,
        approvals: Arc<ApprovalStore>,
        command_plugins: CommandPlugins,
        chat_request: ChatRequest,
        invocation: InvocationContext,
        pending_trim: PendingStreamTrim,
        mut turn_interrupt: TurnInterrupt,
        mut interrupts: TurnInterrupts,
    ) -> Result<TurnEnding> {
        let model_id = cfg.assistant.model.id.resolved();

        debug!(model = %model_id, "Fetching model details.");

        // A network round trip, and the last await before the turn loop starts
        // reading the same receiver.
        let model = tokio::select! {
            details = provider.model_details(&model_id.name) => details?,

            notified = turn_interrupt.recv() => {
                // Nothing has been appended yet, so there is nothing to commit
                // and nothing to sanitize.
                if let Some(notice) = notified {
                    notice.handled();
                }
                info!("Interrupted during model lookup; the turn did not start.");
                return Ok(TurnEnding::Stopped);
            }

            Some(action) = interrupts.next_stop() => {
                info!(?action, "Stopped by a client during model lookup; the turn did not start.");
                return Ok(ended_before_start(&action));
            }
        };

        debug!(model = model.name(), "Model details resolved.");

        // Build docs map from the resolved definitions for describe_tools.
        let docs_map: IndexMap<String, ToolDocs> = tools
            .iter()
            .map(|t| (t.name.clone(), t.docs.clone()))
            .collect();
        let builtin_executors =
            BuiltinExecutors::new().register("describe_tools", DescribeTools::new(docs_map));
        let (executor_source, execution_owner) = TerminalExecutorSource::start_with_metadata(
            builtin_executors,
            Arc::new(SystemProcessRunner),
            command_plugins,
            tools,
            &cfg.conversation.tools,
            approvals,
            invocation.clone(),
            mcp_client,
            root.clone(),
            provider.mcp_tool_metadata(&model),
        )
        .await?;
        // What the service advertises, which is what the provider is shown: a
        // fan-out tool appears with its envelope.
        let tools = executor_source.advertised();
        let tool_coordinator =
            ToolCoordinator::new(cfg.conversation.tools.clone(), Box::new(executor_source))
                .with_interrupt(cfg.interrupt.tool_call.clone());
        let prompt_backend = Arc::new(TerminalPromptBackend);

        let result = Box::pin(run_turn_loop(
            provider,
            &model,
            cfg,
            signals,
            &root,
            invocation,
            interactive,
            attachments,
            lock,
            tool_choice,
            &tools,
            printer,
            prompt_backend,
            tool_coordinator,
            chat_request,
            pending_trim,
            turn_interrupt,
            interrupts,
        ))
        .await;
        if let Err(error) = execution_owner.shutdown().await {
            if result.is_ok() {
                return Err(error.into());
            }
            warn!(%error, "MCP execution service cleanup failed");
        }
        result
    }
}

/// Everything a turn needs, gathered in one place.
///
/// Collecting reads the context; running does not.
/// That split is what lets a turn be driven by a caller that is not the `query`
/// command, without handing it the whole CLI context.
pub(crate) struct TurnInputs {
    /// The effective configuration for this turn, already alias-resolved.
    config: Arc<AppConfig>,

    /// Where an interrupt reaches the turn, and where the turn registers its
    /// own handler for the duration.
    signals: SignalRouter,

    /// Resolves MCP tools and reads MCP resources.
    mcp_client: jp_mcp::Client,

    /// The workspace root, which tool commands and attachments resolve against.
    workspace_root: Utf8PathBuf,

    /// Whether a user is there to answer a prompt or approve a tool call.
    interactive: bool,

    /// What the assistant is given alongside the conversation, in declaration
    /// order.
    ///
    /// The order reaches the provider: every attachment is sent as a document
    /// in this order, numbered by its position.
    attachments: Vec<Attachment>,

    /// Where the turn's output goes.
    printer: Arc<Printer>,

    /// Standing decisions about which tool calls may run unattended.
    approvals: Arc<ApprovalStore>,

    /// The command plugins admitted for this turn, sharing `config` with it.
    ///
    /// A tool on any other plugin is left out of the turn.
    command_plugins: CommandPlugins,

    /// The request that starts the turn.
    chat_request: ChatRequest,

    /// Named in the tool invocation context, so a tool can scope state it
    /// persists to the workspace the call came from.
    workspace_id: WorkspaceId,

    /// A `--replay` trim to apply at the turn-start commit point.
    pending_trim: PendingStreamTrim,

    /// MCP servers starting in the background, awaited by [`TurnInputs::run`].
    mcp_servers: StartupSet,

    /// Interrupts aimed at this turn by whoever asked for it.
    ///
    /// A turn typed at a terminal is interrupted through the signal router and
    /// takes [`TurnInterrupts::none`]; a turn asked for from elsewhere is
    /// reached through here.
    interrupts: TurnInterrupts,

    #[cfg(test)]
    stubbed_provider: Option<Arc<dyn DynProvider>>,
}

impl TurnInputs {
    /// Collect what a turn needs from the context.
    ///
    /// This is the half that reads [`Ctx`]; [`Self::run`] reads none of it, so
    /// the turn can be driven by a caller that has no CLI context to hand over.
    /// The split is about what each half needs, not about how long it takes: an
    /// attachment here can fetch over HTTP, call the GitHub API, or shell out,
    /// and this waits for all of them.
    ///
    /// `printer` is where the turn's output goes: the terminal's printer for a
    /// turn typed there, or a sink printer, which writes nothing, for a turn
    /// started from somewhere with no terminal attached.
    ///
    /// `interactive` says whether a person can answer a prompt for this turn.
    /// It is the caller's to state rather than the context's to report: a turn
    /// asked for over a protocol has nobody at the terminal the host happens to
    /// be running in, and a tool that stopped to ask would block on a read that
    /// no answer is coming for.
    pub(crate) async fn collect(
        ctx: &Ctx,
        config: Arc<AppConfig>,
        chat_request: ChatRequest,
        pending_trim: PendingStreamTrim,
        mcp_servers: StartupSet,
        printer: Arc<Printer>,
        interactive: bool,
        interrupts: TurnInterrupts,
    ) -> Result<Self> {
        let urls: Vec<Url> = config
            .conversation
            .attachments
            .iter()
            .map(attachment_url)
            .collect::<Result<Vec<_>>>()?;

        // One group per URL, in declaration order, flattened into the order
        // the provider receives them in.
        let attachments: Vec<Attachment> = load_conversation_attachments(ctx, urls)
            .await?
            .into_iter()
            .flatten()
            .collect();

        debug!(count = attachments.len(), "Attachments loaded.");

        // Admitted here, before the turn starts, so a prompt asking whether to
        // trust a plugin binary never opens inside a tool call.
        //
        // The turn's config, not the context's: a plugin serving a tool sees
        // the options this query resolved, including conversation config and
        // `--cfg`, rather than resolving its own from the root.
        let command_plugins = admit_turn_plugins(&config, interactive, &printer, PluginInit {
            workspace_id: ctx.workspace.id().to_string(),
            storage: ctx.storage_path().map(ToOwned::to_owned),
            paths: well_known_paths(ctx.user_storage_path()),
            config: config.clone(),
            log_level: ctx.term.args.verbose,
        })?;

        Ok(Self {
            workspace_root: ctx.workspace.root().to_path_buf(),
            approvals: Arc::new(load_approval_store(ctx.fs_backend.as_deref())),
            command_plugins,
            workspace_id: ctx.workspace.id().clone(),
            signals: ctx.signals.clone(),
            mcp_client: ctx.mcp_client.clone(),
            printer,
            interactive,
            attachments,
            mcp_servers,
            chat_request,
            pending_trim,
            config,
            interrupts,
            #[cfg(test)]
            stubbed_provider: ctx.stubbed_provider.clone(),
        })
    }

    /// Finish preparing, then run the turn.
    ///
    /// Borrows only the lock, which owns itself, so this can run on a task of
    /// its own, which is where the waiting belongs.
    /// `stream` is the snapshot the thread is assembled from.
    pub(crate) async fn run(
        self,
        lock: &ConversationLock,
        stream: ConversationStream,
        mut turn_interrupt: TurnInterrupt,
    ) -> Result<TurnEnding> {
        let cfg = &self.config;
        let mut interrupts = self.interrupts;

        let tools = tokio::select! {
            result = async {
                // Wait for all MCP servers to finish loading, showing a timer line
                // when the wait takes long enough to be noticeable.
                let waited = Instant::now();
                let skipped = mcp_startup::await_mcp_servers(
                    self.mcp_servers,
                    cfg.style.mcp_startup.clone(),
                    self.printer.clone(),
                )
                .await?;
                mcp_startup::report_skipped_servers(&self.printer, cfg, &skipped);
                debug!(
                    elapsed_ms = waited.elapsed().as_millis(),
                    "MCP servers ready."
                );

                let forced_tool = cfg.assistant.tool_choice.function_name();
                let offered =
                    without_unadmitted_plugins(cfg.conversation.tools.iter(), &self.command_plugins);
                let tools = tool_definitions(
                    offered.into_iter(),
                    &self.mcp_client,
                    forced_tool,
                )
                .await?;
                debug!(count = tools.len(), forced_tool, "Tools resolved.");

                Ok::<_, Error>(tools)
            } => result?,

            notified = turn_interrupt.recv() => {
                // Nothing has been appended to the conversation yet: the request
                // is added at the turn-start commit point inside the loop below.
                // So there is nothing to commit and nothing to sanitize, and
                // dropping the guards releases the lock.
                if let Some(notice) = notified {
                    notice.handled();
                }
                info!("Interrupted while preparing; the turn did not start.");
                return Ok(TurnEnding::Stopped);
            }

            // A client's stop, for the same span: nothing has been appended, so
            // ending here leaves the conversation as it was. A reply is held for
            // the turn to take once its own request is in place.
            Some(action) = interrupts.next_stop() => {
                info!(?action, "Stopped by a client while preparing; the turn did not start.");
                return Ok(ended_before_start(&action));
            }
        };

        let thread = build_thread(stream, self.attachments, &cfg.assistant, !tools.is_empty())?;
        debug!(
            events = thread.events.len(),
            attachments = thread.attachments.len(),
            "Thread assembled."
        );

        // Sanitize any structural issues (orphaned tool calls, missing user
        // messages, etc.) before sending the stream to the provider.
        lock.as_mut().update_events(ConversationStream::sanitize);

        #[cfg(test)]
        let stubbed = self.stubbed_provider;
        #[cfg(not(test))]
        let stubbed = None;
        let provider = if let Some(provider) = stubbed {
            provider
        } else {
            let provider_id = cfg.assistant.model.id.resolved().provider;
            Arc::from(provider::get_provider(provider_id, &cfg.providers.llm)?)
        };

        Query::run_turn(
            cfg,
            provider,
            &self.signals,
            &self.mcp_client,
            self.workspace_root,
            self.interactive,
            &thread.attachments,
            lock,
            cfg.assistant.tool_choice.clone(),
            &tools,
            self.printer,
            self.approvals,
            self.command_plugins,
            self.chat_request,
            // Built from the lock the turn actually runs against, so it cannot
            // name one conversation while the events land in another.
            InvocationContext {
                workspace_id: self.workspace_id.into_string(),
                conversation_id: lock.id().to_string(),
            },
            self.pending_trim,
            turn_interrupt,
            interrupts,
        )
        .await
    }
}

/// The id of the conversation's last [`ChatRequest`], if it has one.
///
/// Taken before a turn runs and compared after it, this says whether the run
/// appended a request: a turn stopped before it started leaves the stream's
/// requests as they were, and one that started appends a request with an id the
/// stream has never handed out.
/// Sanitizing the stream before the turn can add or remove a `TurnStart`, but
/// never a request, so the comparison holds across it.
pub(crate) fn last_request_id(events: &ConversationStream) -> Option<EventId> {
    events
        .iter_events_by_turn()
        .filter(|entry| entry.is_chat_request())
        .last()
        .map(|entry| entry.event_id.clone())
}

/// Map a pre-start client interrupt to its [`TurnEnding`].
///
/// [`TurnInterrupts::next_stop`] only yields `Stop` or `Abort`; replies stay
/// queued until the turn starts.
fn ended_before_start(action: &InterruptAction) -> TurnEnding {
    match action {
        InterruptAction::Abort => TurnEnding::Aborted,
        _ => TurnEnding::Stopped,
    }
}

#[cfg(test)]
#[path = "turn_start_tests.rs"]
mod tests;
