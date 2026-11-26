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
//! See `docs/wiki/00dxyb6-query-stream-pipeline-architecture.md` for the full
//! design document.
//!
//! [`ChatRenderer`]: crate::render::ChatRenderer
//! [`ChatRequest`]: jp_conversation::event::ChatRequest
//! [`ChatResponse`]: jp_conversation::event::ChatResponse
//! [`ConversationEvent`]: jp_conversation::event::ConversationEvent
//! [`EventBuilder`]: jp_llm::event_builder::EventBuilder
//! [`InterruptHandler`]: interrupt::handler::InterruptHandler
//! [`StreamRetryState`]: stream::retry::StreamRetryState
//! [`ToolCallRequest`]: jp_conversation::event::ToolCallRequest
//! [`ToolCallResponse`]: jp_conversation::event::ToolCallResponse
//! [`ToolCoordinator`]: tool::ToolCoordinator
//! [`TurnCoordinator`]: turn::coordinator::TurnCoordinator

mod args;
mod compose;
mod config;
mod draft;
mod input;
pub(crate) mod interrupt;
mod mcp_startup;
mod mounts;
mod quote;
mod stream;
mod thread;
pub(crate) mod tool;
mod turn;
mod turn_loop;
mod turn_result;
mod turn_start;

use std::time::Duration;

pub(crate) use args::ToolDirective;
use args::{QueryText, ToolFlags, ToolUse};
pub(crate) use compose::PendingStreamTrim;
use compose::{BuiltConversation, QuerySource};
use config::persist_config_reset;
pub(crate) use config::turn_config_delta;
pub(super) use config::{apply_model, conversation_config};
use draft::{
    DraftRemoval, cleanup_query_message_file, draft_dir, draft_fingerprint, draft_store,
    is_abandoned, preserve_query_message_file,
};
use input::{query_file_path, read_arg_file, read_piped_stdin};
use interrupt::TurnInterrupts;
use jp_config::{
    AppConfig, PartialConfig as _,
    assignment::KvAssignment,
    conversation::ConversationConfig,
    model::parameters::{ReasoningConfig, ServiceTier},
    providers::llm::AuthEntry,
};
use jp_conversation::{
    Conversation, ConversationId, ConversationStream, Labels, stream::ApplyDelta,
};
use jp_inquire::prompt::{PromptBackend, TerminalPromptBackend};
use jp_llm::{event::NoticeSink, provider};
use jp_storage::backend::Projection;
use jp_task::task::{TitleGeneratorTask, TurnSummaryTask};
use jp_workspace::{ConversationHandle, ConversationLock, ConversationMut};
use mounts::create_mount_effects;
use tracing::{debug, trace, warn};
pub(crate) use turn::TurnEnding;
pub(crate) use turn_result::{StructuredDataError, turn_answer, turn_structured_data};
pub(crate) use turn_start::{TurnInputs, last_request_id};

use super::{
    ConversationLoadRequest, Output,
    compact_flag::CompactFlag,
    conversation_id::{ConversationIds, FlagIds},
    lock::LockOutcome,
    target::TargetGrammar,
};
use crate::{
    Ctx,
    cli::flag::{Flag, Settled, Stage},
    cmd::{
        self,
        conversation::fork,
        label::resolve::{Resolver, Trigger},
        lock::{LockRequest, acquire_lock},
    },
    config_pipeline,
    ctx::McpServerScope,
    error::{Error, Result},
    output::{background_notice_sink, print_json},
    parser::AttachmentUrlOrPath,
    render::{RenderFlow, TurnView},
};

/// The arguments of `jp query`, decided as far as `S` says.
///
/// Help text for each argument is declared in `Query::augment`.
#[derive(Debug, Default)]
pub(crate) struct Query<S: Stage = Settled> {
    template: bool,
    schema: Option<schemars::Schema>,
    replay: bool,
    target: FlagIds<false, false>,

    /// `None` without `--fork`, `Some(None)` to keep every turn,
    /// `Some(Some(n))` to keep the last `n`.
    fork: Option<Option<usize>>,
    new_conversation: bool,
    local: bool,
    no_local: bool,
    attachments: Vec<AttachmentUrlOrPath>,
    edit: bool,
    no_edit: bool,

    /// The query words, in the order they were given.
    query: S::Text<QueryText>,

    /// `Some(true)` prefixes the quoted message with `> `, `Some(false)` seeds
    /// it verbatim, `None` means `--quote` was not given.
    quote: Option<bool>,
    model: Option<String>,
    auth: Vec<AuthEntry>,
    parameters: Vec<KvAssignment>,
    reasoning: Option<ReasoningConfig>,
    no_reasoning: bool,
    service_tier: Option<ServiceTier>,
    no_service_tier: bool,
    hide_reasoning: bool,
    hide_tool_calls: bool,

    /// `-t` and `-T`, in the order written.
    tool_directives: S::Config<ToolFlags>,

    /// `None` without `--tmp`, `Some(None)` without a duration.
    expires_in: Option<Option<humantime::Duration>>,
    title: Option<String>,
    no_title: bool,

    /// `None` without `-u`, `Some(None)` for a bare `-u`, `Some(Some(value))`
    /// for a value.
    tool_use: S::Config<Flag<ToolUse>>,
    no_tool_use: bool,
    compact: CompactFlag,
    mount: Vec<String>,
}

impl<S: Stage> Query<S> {
    /// Declare what conversations this command needs.
    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        if self.is_new() {
            return ConversationLoadRequest::none();
        }

        ConversationLoadRequest::explicit_or_session_with_config(&self.target)
    }

    #[must_use]
    fn is_new(&self) -> bool {
        self.new_conversation
    }

    /// Whether the conversation picker may offer a "start a new conversation"
    /// item for this invocation.
    ///
    /// Returns `false` when any flag incompatible with `--new` is present, so
    /// that choosing the item can never produce a combination clap would have
    /// rejected.
    /// `--new` conflicts with `--fork`, `--replay`, and `--id`.
    /// A non-empty `target` means `--id` was given (including bare `--id`,
    /// which parses to an empty value and lands on the picker), so it gates the
    /// item just like the other two.
    pub(crate) fn allows_new_from_picker(&self) -> bool {
        self.fork.is_none() && !self.replay && self.target.ids().is_empty()
    }
}

impl Query {
    pub(crate) async fn run(
        self,
        ctx: &mut Ctx,
        handle: Option<ConversationHandle>,
        start_new: bool,
    ) -> Output {
        debug!("Running `query` command.");
        trace!(args = ?self, "Received arguments.");

        // Resolve the query before any conversation or session state is
        // touched: an unreadable `@path` must not leave a conversation created
        // and recorded as the session's active one.
        let query = self.resolve_query()?;

        // Resolve the target conversation and acquire an exclusive lock.
        //
        // Paths:
        // 1. --new: create a fresh conversation (already locked).
        // 2. picker "start new": `start_new` is set, create a fresh conversation.
        // 3. --fork/--id/session: resolve an existing conversation, lock it.
        // 4. Lock contention: user picks "new" or "fork" from the prompt.
        let AcquiredConversation {
            lock,
            fresh,
            staged,
        } = self.acquire_lock(ctx, handle, start_new).await?;

        // Track whether this run created the conversation (`--new` or `--fork`)
        // so an abandoned new conversation can be removed on exit.
        let created = !lock.is_written();
        let turns_before = lock.with_events(ConversationStream::turn_count);

        let result = Box::pin(self.run_locked(ctx, &lock, query, fresh, staged)).await;

        // Every exit from the locked region lands here, which is what makes
        // this the one reliable drain point: a mutation scope that dropped
        // while dirty recorded its persist failure on the lock, and any `?` in
        // the region above returns past every other candidate site.
        let persist_failure = lock.take_persist_failure();

        // If this run created the conversation and exited without adding a turn
        // or saving a query draft, remove the empty conversation directory
        // while we still hold the lock.
        if created && is_abandoned(draft_store(ctx), &lock, turns_before) {
            ctx.workspace.remove_conversation_with_lock(lock.into_mut());
        }

        cmd::fold_persist_failure(result, persist_failure)
    }

    /// Run the query against an already-locked conversation.
    ///
    /// `fresh` is `true` when this run created the conversation, so no config
    /// state predates its base config.
    /// `staged` carries mutations acquiring the conversation already made
    /// without writing them, which this run either commits or discards along
    /// with its own.
    ///
    /// Errors propagate freely: the caller drains any persist failure the
    /// unwinding left behind.
    #[expect(clippy::too_many_lines)]
    async fn run_locked(
        self,
        ctx: &mut Ctx,
        lock: &ConversationLock,
        query: Option<String>,
        fresh: bool,
        staged: Option<ConversationMut>,
    ) -> Output {
        let now = ctx.now();
        let cfg = ctx.config();

        // One scope for everything this run changes about the conversation
        // before the turn starts — whatever `--tmp`, `--title`, `--fork`,
        // `--compact`, or `--cfg` put on it.
        //
        // It discards on drop, so the `flush` below, once the request is known
        // to be non-empty, is the only thing that writes. A query the user walks
        // away from, and any failure before the turn starts, leave the stored
        // conversation exactly as they found it.
        let mut setup = staged.unwrap_or_else(|| lock.as_mut());
        setup.discard_on_drop();

        // Create symlinks and seed approvals for any `--mount` flags before the
        // turn runs, so tools can reach the mounted paths.
        create_mount_effects(&self.mount, &ctx.workspace, ctx.fs_backend.as_deref(), now)?;

        // Stamp the expiry a fresh conversation was created with. An existing
        // conversation keeps the expiry it has: `--tmp` describes a conversation
        // this run starts, not one it continues, and clap rejects it without
        // `--new`.
        if fresh && let Some(duration) = self.expires_in_duration() {
            let expires_at = chrono::Duration::from_std(duration)
                .ok()
                .and_then(|v| lock.id().timestamp().checked_add_signed(v));

            setup.update_metadata(|m| m.expires_at = expires_at);
        }

        // Fail fast on provider misconfiguration (e.g. a missing API key
        // environment variable, or a credential chain with no usable
        // entry) before any side-effectful work below: pre-query
        // compaction can run a full summary LLM round-trip, MCP servers
        // boot in background tasks, the editor may open to compose the
        // request, and title generation and attachment loading are all
        // wasted — and the title task alone can hold the run open for
        // seconds at teardown — when the request can never be sent.
        // `Query::run_turn` repeats this check implicitly when it constructs
        // the live provider.
        let model_id = cfg.assistant.model.id.resolved();
        provider::preflight(model_id.provider, &cfg.providers.llm)?;

        // Compact the conversation before querying, if requested. Staged on
        // `setup`, so the composed request and the editor's history preview see
        // the compacted stream while nothing is written yet.
        if self.compact.should_compact() {
            let notices = background_notice_sink("pre-query compaction");
            self.apply_pre_query_compaction(&setup, &cfg, &notices)
                .await?;
        }

        // `-u`/`-U` never enter the config, so the turn's choice is resolved
        // from the flags here and carried through to the tool list and the turn
        // loop.
        let tool_choice = self.effective_tool_choice(&cfg);
        let forced_tool = tool_choice.function_name();

        let mcp_servers_handle = ctx
            .configure_active_mcp_servers(forced_tool, McpServerScope::Exclusive)
            .await?;

        // The title this run ends with, resolved here because the terminal
        // title is named after it. Writing it into metadata waits until the
        // request is known to be non-empty, so an abandoned query leaves the
        // conversation as it found it.
        let stored_title = lock.metadata().title.clone();
        let conv_title =
            resolve_title_override(stored_title.clone(), self.title.as_deref(), self.no_title);

        // Show conversation identity in the terminal title.
        if ctx.term.is_tty {
            set_terminal_title(lock.id(), conv_title.as_deref());
        }

        let cid = lock.id();

        // Persist a newly created conversation before opening the editor or
        // writing a recovery draft so `QUERY_MESSAGE.md` never sits in a
        // directory without `metadata.json` and `events.json`.
        if !lock.is_written() {
            setup.store()?;
        }

        // Held until the run ends: under `--no-persist` it is the directory the
        // editor composes in.
        let (conversation_path, _scratch) = draft_dir(
            ctx.workspace.persists(),
            ctx.fs_backend.as_deref(),
            ctx.workspace.root(),
            &cid,
            stored_title.as_deref(),
        )?;

        let piped = read_piped_stdin()?;

        // Build the request read-only: replay resolution, quote seeding, and
        // the editor session all operate on (a view of) the stream without
        // mutating it. The destructive replay trim is described by
        // `pending_trim` and committed at the turn-start commit point,
        // atomically with appending the new request.
        let BuiltConversation {
            query_source,
            mut editor_provided_config,
            pending_trim,
            chat_request,
        } = lock.with_events(|stream| {
            self.build_conversation(
                &piped,
                query.as_deref(),
                stream,
                ctx.term.interactive,
                &cfg,
                &conversation_path,
                &ctx.printer,
            )
        })?;

        let Some(mut chat_request) = chat_request else {
            // Empty query, early exit. `setup` drops unflushed, discarding
            // everything staged on it, and the request was composed against a
            // view of the stream rather than the stream itself — so the stored
            // conversation is untouched, even for `--replay`. One this run
            // created is removed by `run`, which finds no draft and no new turn.
            if query_source == QuerySource::Editor {
                cleanup_query_message_file(draft_store(ctx), &cid, DraftRemoval::Any);
            }
            ctx.printer.println("Query is empty, ignoring.");
            return Ok(());
        };

        // Record this conversation as the session's active conversation.
        //
        // Deferred until the request is known to be non-empty: the metadata
        // bump this performs is the first write a fresh conversation gets, so
        // running it earlier would leave a stored, session-active conversation
        // behind for a query that was ultimately ignored.
        if let Some(session) = &ctx.session
            && let Err(error) = ctx
                .workspace
                .activate_session_conversation(lock, session, now)
        {
            warn!(%error, "Failed to record activation.");
        }

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

        // Preserve the composed request in the conversation's user-local
        // scratch file so a pre-turn interrupt (Ctrl-C during MCP boot,
        // attachment loading, model resolution, ...) never silently discards
        // it: the next `jp q --edit` session seeds its buffer from this file.
        // An editor-composed query is already on disk (the editor session
        // wrote it and the guard was disarmed); only inline, piped, and
        // replayed requests need the explicit write. The file is removed once
        // the turn completes successfully.
        //
        // Only preserve when user-local storage is configured: without it,
        // `conversation_path` has fallen back to the workspace conversation
        // directory, where `cleanup_query_message_file` (which resolves
        // exclusively through the user-local store) could never remove the
        // file again. The scratch file must never land in the committed
        // workspace tree.
        //
        // Skip under `--no-persist`: the conversation itself is not written to
        // disk, and a failed turn would leave `QUERY_MESSAGE.md` in a directory
        // with no persisted conversation.
        if query_source != QuerySource::Editor
            && ctx.workspace.persists()
            && ctx
                .fs_backend
                .as_deref()
                .is_some_and(|fs| fs.user_storage_path().is_some())
        {
            preserve_query_message_file(&conversation_path, &chat_request.content);
        }

        // The draft as it stands now, so the cleanup after the turn can tell it
        // apart from one written while the turn ran: by another process, or by a
        // plugin through `write_draft`.
        //
        // Taken from the file rather than from the request, because the two are
        // not the same text: an editor-composed draft keeps its configuration
        // and history sections, and `--template` renders the request after the
        // file was written.
        let draft_at_send = draft_fingerprint(draft_store(ctx), &cid);

        // Echo the request back through the same role-aware rendering
        // machinery used by replay and live streaming — a labeled user header
        // followed by the request body — so the boundary between user input
        // and the forthcoming assistant response is visually clear. Render
        // this before any post-edit work (MCP init, attachments, tools) so
        // that failures in those stages don't swallow the user's message.
        if self.should_echo_request(query_source) {
            let mut echo = TurnView::new(
                ctx.printer.clone(),
                cfg.style.clone(),
                cfg.assistant.name.clone(),
                Some(cfg.assistant.model.id.resolved().to_string()),
                RenderFlow::Live,
            );
            echo.render_user_request(&chat_request);
        }

        // Store the title resolved before the draft was composed. A run that
        // names the title the conversation already carries changes nothing, and
        // leaves the scope clean rather than rewriting it.
        if conv_title != stored_title {
            setup.update_metadata(|m| m.title.clone_from(&conv_title));
        }

        // Persist config state changes into the conversation stream, now that
        // the query is known to be non-empty. Recording them before the
        // empty-query check would leave config events behind for a query that
        // was ultimately ignored.
        //
        // A fresh conversation needs neither branch: its base config was
        // written from this invocation's resolved config at creation time
        // (that also absorbs any `--cfg` reset keyword, per [RFD 038]).
        //
        // A conversation carrying earlier config state — continuing or forked
        // — records a `--cfg` reset keyword as the reset-then-layer sequence
        // [RFD 038] describes.
        //
        // Without a reset keyword, any divergence between the stream's config
        // and this invocation's resolved config is appended as a single
        // `Apply` diff.
        //
        // [RFD 038]: https://jp.computer/rfd/038
        if let Some(reset_events) = ctx.config_reset.take() {
            if !fresh {
                setup.update_events(|events| persist_config_reset(events, reset_events, now));
            }
        } else if let Some(delta) = turn_config_delta(&cfg, lock)? {
            setup.update_events(|events| events.add_config_delta(delta));
        }

        if !editor_provided_config.is_empty() {
            // Resolve any model aliases before storing in the stream so
            // that per-event configs always contain concrete model IDs.
            editor_provided_config.resolve_model_aliases(&cfg.providers.llm.aliases);

            // The editor hands back the config it was shown, so most of what
            // comes back is what was already in effect. Only the part that
            // changes anything is worth an event.
            let current = setup
                .events()
                .config_partial()
                .map_err(jp_conversation::Error::from)?;

            if let Some((delta, unsets)) =
                config_pipeline::override_to_record(&current, editor_provided_config)?
            {
                let apply = ApplyDelta::with_unsets(now, delta, unsets);
                setup.update_events(|events| events.add_config_delta(apply));
            }
        }

        // Snapshot the stream for title generation and thread assembly. The
        // snapshot reflects what the durable stream will contain once the turn
        // starts: the pending replay trim applied (on the clone — the durable
        // stream is only trimmed at the turn-start commit point), the new
        // request not yet appended.
        let stream = {
            let mut stream = setup.events().clone();
            pending_trim.apply(&mut stream);
            stream
        };

        // Set the title for new or empty conversations (including forks).
        // Skip when `--title` or `--no-title` was provided (the user already
        // expressed an intent for the title).
        //
        // A leading markdown heading in the prompt is used verbatim as the
        // title, short-circuiting the LLM round-trip. Otherwise, fall back to
        // background title generation when enabled.
        if (self.is_new() || self.fork.is_some() || stream.is_empty())
            && ctx.workspace.persists()
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
                    setup.update_metadata(|m| m.title = Some(title.clone()));
                    if ctx.term.is_tty {
                        jp_term::osc::set_title(format!("{cid}: {title}"));
                    }
                }
                NewTitle::Generate => {
                    debug!("Generating title for new conversation");
                    let mut stream = stream.clone();
                    stream.start_turn(chat_request.clone());
                    // A misconfigured title model must not fail the query —
                    // the turn itself runs on the (already preflighted)
                    // assistant model. Skip the title instead of spawning a
                    // task that is doomed to fail after holding teardown
                    // open.
                    let notices = background_notice_sink("title generation");
                    match TitleGeneratorTask::new(cid, stream, &cfg, ctx.term.is_tty, notices) {
                        Ok(task) => ctx.task_handler.spawn(task),
                        Err(error) => warn!(%error, "Skipping title generation."),
                    }
                }
                NewTitle::Skip => {}
            }
        }

        // Spawn background summarization for earlier completed turns that do
        // not yet have a summary. A misconfigured summary model logs a warning
        // rather than failing the query.
        if cfg.conversation.turn_summary.generate.auto && ctx.workspace.persists() {
            match TurnSummaryTask::new(cid, &stream, &cfg, background_notice_sink("turn summary")) {
                Ok(Some(task)) => ctx.task_handler.spawn(task),
                Ok(None) => {}
                Err(error) => warn!(%error, "Skipping turn summaries."),
            }
        }

        // Commit the setup phase before the turn starts. Errors propagate here
        // rather than being swallowed by a drop, and the turn loop's own
        // checkpoints take over from this point.
        setup.flush()?;
        drop(setup);

        // The schema the provider was given, which is what the response is held
        // to.
        let schema = chat_request.schema.clone();

        // Read from the lock, not from `stream`: a `--replay` snapshot already
        // has its last turn trimmed, and a turn stopped before it starts
        // leaves that turn in place.
        let before = lock.with_events(last_request_id);

        let inputs = TurnInputs::collect(
            ctx,
            cfg.clone(),
            chat_request,
            pending_trim,
            mcp_servers_handle,
            ctx.printer.clone(),
            ctx.term.interactive,
            // Ctrl-C is how this turn is interrupted, and that arrives through
            // the signal router.
            TurnInterrupts::none(),
        )
        .await?;

        // Direct CLI queries treat all `TurnEnding` variants as a clean exit.
        let mut turn_result = inputs
            .run(lock, stream, ctx.signals.turn_interrupt())
            .await
            .map(|_ending| ())
            .map_err(|error| cmd::Error::from(error).with_persistence(true));

        // Fold a drop-time persist failure into the turn's result before
        // anything downstream treats the turn as a success. Both gates below
        // key off `is_ok`, and the scratch file one of them deletes is the
        // user's only recovery copy of a request that was never saved.
        //
        // `run` drains again for the exits that never reach this point; the
        // failure is only ever handed out once.
        if turn_result.is_ok()
            && let Some(error) = lock.take_persist_failure()
        {
            turn_result = Err(cmd::Error::from(Error::Workspace(error)));
        }

        if self.schema.is_some() && turn_result.is_ok() {
            let data = lock
                .with_events(|events| {
                    turn_structured_data(events, before.as_ref(), schema.as_ref())
                })
                .map_err(Error::StructuredData)?;

            print_json(&ctx.printer, &data);
        }

        // Clean up the query file only once the request is in the conversation.
        // After an error, or a turn stopped before it started, the file is the
        // only copy of the request. The conversation directory may have been
        // renamed mid-turn (e.g. a heading-derived title), so re-resolve the
        // live directories rather than trusting the path captured before the
        // turn ran.
        if turn_result.is_ok() && lock.with_events(last_request_id) != before {
            cleanup_query_message_file(
                draft_store(ctx),
                &cid,
                DraftRemoval::IfUnchanged(draft_at_send.as_deref()),
            );
        }

        turn_result
    }

    /// Resolve the positional query into the text to send.
    ///
    /// A query of exactly one `@path` value is read from that file; any other
    /// query is its values joined with spaces, leaving an `@` word inside a
    /// multi-word query as ordinary text.
    /// Returns `None` when no query was given, which leaves the request to be
    /// composed from piped stdin, a replayed request, or the editor.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ArgFile`] if the named file cannot be read.
    fn resolve_query(&self) -> Result<Option<String>> {
        let Some(values) = &self.query else {
            return Ok(None);
        };

        let text = match query_file_path(values) {
            Some(path) => read_arg_file(path)?,
            None => values.join(" "),
        };

        Ok(Some(text))
    }

    /// Create a new conversation and return an exclusive lock.
    ///
    /// The conversation exists in memory only: nothing about it reaches storage
    /// until a mutation scope on the returned lock is flushed.
    async fn create_new_conversation(&self, ctx: &mut Ctx) -> Result<ConversationLock> {
        let cfg = ctx.config();

        // Resolved before the conversation exists so a rule that needs
        // confirmation, and finds no terminal to ask on, aborts without leaving
        // a half-labelled conversation behind.
        let prompts = TerminalPromptBackend;
        // A rule that produced no values names no label on a fresh
        // conversation: there is nothing here for it to replace.
        let labels: Labels = label_resolver(ctx, &cfg, &prompts)
            .automatic(Trigger::New)
            .await?
            .into_iter()
            .collect();

        let ws = &mut ctx.workspace;
        let projection = if self.is_local(&cfg.conversation) {
            Projection::LocalOnly
        } else {
            Projection::Projected
        };
        let lock = ws.create_and_lock_conversation_with_projection(
            Conversation {
                labels,
                ..Conversation::default()
            },
            cfg.clone(),
            ctx.session.as_ref(),
            projection,
        )?;
        let id = lock.id();

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

    #[must_use]
    fn is_local(&self, cfg: &ConversationConfig) -> bool {
        (self.local || cfg.start_local) && !self.no_local
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
    /// compaction events to `conv`, which decides when — or whether — they
    /// are written.
    async fn apply_pre_query_compaction(
        &self,
        conv: &ConversationMut,
        cfg: &AppConfig,
        notices: &NoticeSink,
    ) -> Result<()> {
        let events = conv.events().clone();

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
            // `--compact` on a query applies the configured rules to the whole
            // conversation; there are no turn-selection flags to honour.
            &crate::cmd::turn_selection::TurnSelection::default(),
            // `--compact` on a query is a quick adjunct; apply it silently so
            // compaction details don't clutter the query output.
            None,
            notices,
        )
        .await?;

        super::conversation::compact::apply_compactions(conv, compactions);

        Ok(())
    }

    /// Resolve the target conversation and acquire its exclusive lock.
    async fn acquire_lock(
        &self,
        ctx: &mut Ctx,
        handle: Option<ConversationHandle>,
        start_new: bool,
    ) -> Result<AcquiredConversation> {
        // Handle --new: create a fresh conversation.
        if self.is_new() {
            return Ok(AcquiredConversation::created(
                self.create_new_conversation(ctx).await?,
            ));
        }

        // Handle the picker's "start a new conversation" choice. It carries no
        // target, so any flag that needs one is unsatisfiable; refuse loudly
        // rather than silently dropping it. The same predicate gates the offer.
        if start_new {
            if !self.allows_new_from_picker() {
                return Err(Error::NewConflictsWithTarget);
            }
            return Ok(AcquiredConversation::created(
                self.create_new_conversation(ctx).await?,
            ));
        }

        // `--new` is only worth suggesting when it wouldn't conflict with a
        // flag already given; the same predicate gates the picker's offer.
        let grammar = TargetGrammar::from_args(&self.target, self.allows_new_from_picker());
        let handle = handle.ok_or_else(|| Error::from(grammar))?;

        // Handle --fork: fork the conversation before locking.
        if let Some(fork_turns) = self.fork {
            return Ok(AcquiredConversation::forked(
                fork_conversation(ctx, &handle, fork_turns).await?,
            ));
        }

        let req = LockRequest::from_ctx(handle, ctx)
            .allow_new(true)
            .allow_fork(true);

        match acquire_lock(req).await? {
            LockOutcome::Acquired(lock) => Ok(AcquiredConversation::resumed(lock)),
            LockOutcome::NewConversation => Ok(AcquiredConversation::created(
                self.create_new_conversation(ctx).await?,
            )),
            LockOutcome::ForkConversation(handle) => Ok(AcquiredConversation::forked(
                fork_conversation(ctx, &handle, None).await?,
            )),
        }
    }
}

/// The conversation a run targets, exclusively locked, and how it got there.
struct AcquiredConversation {
    lock: ConversationLock,

    /// Whether this run created the conversation, so no config state predates
    /// its base config.
    ///
    /// A fork is `false`: it copies the source's base config and events, and
    /// therefore carries config state from before this invocation.
    fresh: bool,

    /// Mutations made while acquiring the conversation, held unwritten.
    ///
    /// A fork arrives with its inherited stream staged here.
    /// The scope discards on drop, so whoever takes it decides whether the
    /// conversation is ever written by flushing it.
    staged: Option<ConversationMut>,
}

impl AcquiredConversation {
    /// A conversation that already existed, with nothing staged on it.
    fn resumed(lock: ConversationLock) -> Self {
        Self {
            lock,
            fresh: false,
            staged: None,
        }
    }

    /// A conversation this run created, whose base config is this invocation's
    /// resolved config.
    fn created(lock: ConversationLock) -> Self {
        Self {
            lock,
            fresh: true,
            staged: None,
        }
    }

    /// A fork, whose inherited stream is staged but not yet written.
    fn forked((lock, staged): (ConversationLock, ConversationMut)) -> Self {
        Self {
            lock,
            fresh: false,
            staged: Some(staged),
        }
    }
}

/// Fork a conversation and return the new conversation's lock.
async fn fork_conversation(
    ctx: &mut Ctx,
    source: &ConversationHandle,
    fork_turns: Option<usize>,
) -> Result<(ConversationLock, ConversationMut)> {
    fork::fork_conversation(ctx, source, |events| {
        if let Some(n) = fork_turns {
            events.retain_last_turns(n);
        }
    })
    .await
}

/// Build a label resolver for this run.
///
/// Commands run at the workspace root so a rule produces the same value
/// wherever in the tree the user invoked JP from.
fn label_resolver<'a>(
    ctx: &'a Ctx,
    cfg: &'a AppConfig,
    prompts: &'a dyn PromptBackend,
) -> Resolver<'a> {
    Resolver::new(
        &cfg.conversation.labels,
        ctx.workspace.root(),
        ctx.term.interactive,
        &ctx.printer,
        prompts,
    )
}

/// How a new conversation's title is set from its first prompt, before the turn
/// runs.
#[derive(Debug, PartialEq)]
pub(crate) enum NewTitle {
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
pub(crate) fn resolve_new_title(
    from_heading: bool,
    generate_auto: bool,
    content: &str,
) -> NewTitle {
    if from_heading && let Some(title) = jp_md::heading::leading_heading(content) {
        return NewTitle::FromHeading(title);
    }

    if generate_auto {
        return NewTitle::Generate;
    }

    NewTitle::Skip
}

/// The title the conversation ends the run with, given the title it carries now
/// and the `--title` / `--no-title` flags.
///
/// The two flags are mutually exclusive (enforced by clap):
///
/// - `--title T` names the title.
/// - `--no-title` clears it.
/// - Neither keeps `current`, whether that came from a fork inheriting the
///   source's title, a resumed conversation, or a new one with none.
fn resolve_title_override(
    current: Option<String>,
    title: Option<&str>,
    no_title: bool,
) -> Option<String> {
    if let Some(title) = title {
        return Some(title.to_owned());
    }

    if no_title {
        return None;
    }

    current
}

/// Set the terminal title to show the active conversation.
///
/// `set_title` removes any control character from the title it writes.
fn set_terminal_title(id: ConversationId, title: Option<&str>) {
    let display = match title {
        Some(t) => format!("{id}: {t}"),
        None => id.to_string(),
    };
    jp_term::osc::set_title(display);
}

#[cfg(test)]
#[path = "query_tests.rs"]
mod tests;
