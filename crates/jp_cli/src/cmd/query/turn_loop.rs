//! Extracted turn loop for testability.
//!
//! This module contains the core turn loop logic, extracted from `handle_turn`
//! to enable integration testing with mock providers.

use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use camino::Utf8Path;
use futures::{
    Stream, StreamExt as _, future,
    stream::{self, SelectAll},
};
use indexmap::IndexMap;
use jp_attachment::Attachment;
use jp_config::{
    AppConfig, PartialConfig, assistant::tool_choice::ToolChoice,
    conversation::tool::QuestionTarget, model::id::ProviderId, style::streaming::StreamingConfig,
};
use jp_conversation::{
    ConversationStream,
    event::{ChatRequest, ToolCallRequest, ToolCallResponse},
};
use jp_inquire::prompt::PromptBackend;
use jp_llm::{
    Provider,
    error::StreamError,
    event::{Event, EventPart, ToolCallPart},
    model::ModelDetails,
    provider::get_provider,
    query::ChatQuery,
    tool::{InvocationContext, ToolDefinition, executor::Executor},
    with_idle_timeout,
};
use jp_printer::{ErrChannel, Printer};
use jp_workspace::{ConversationLock, ConversationMut};
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_stream::wrappers::{BroadcastStream, ReceiverStream, errors::BroadcastStreamRecvError};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::{
    build_sections, build_thread,
    interrupt::{LoopAction, handle_llm_event, handle_streaming_signal, reply_edit_mode},
    stream::{StreamRetryState, handle_stream_error},
    tool::{
        PendingEntry, PendingTools, ToolCallDecision, ToolCallState, ToolCoordinator, ToolPrompter,
        ToolRenderer, build_execution_plan,
        inquiry::{InquiryBackend, InquiryConfig, LlmInquiryBackend},
        spawn_line_timer,
    },
    turn::{Action, CommittedEvent, TurnCoordinator, TurnPhase, TurnState},
};
use crate::{
    cmd::query::tool::coordinator::ExecutionResult,
    editor::build_editor_backend,
    error::Error,
    render::metadata::set_rendered_arguments,
    signals::{SignalRx, SignalTo},
};

/// Events produced by the merged streaming loop sources.
enum StreamingLoopEvent {
    /// A signal from the signal handler (e.g. Ctrl+C).
    Signal(SignalTo),
    /// An event from the LLM provider stream.
    Llm(Box<Result<Event, StreamError>>),
    /// A tick from the preparing indicator timer, carrying the elapsed time
    /// since the timer started.
    PreparingTick(Duration),
}

/// Wrapper enum that unifies heterogeneous stream sources for [`SelectAll`].
///
/// Each variant holds a different concrete stream type, but they all yield
/// [`StreamingLoopEvent`].
/// This avoids boxing while allowing `select_all` to poll them as a single
/// merged stream.
enum StreamSource<S, L, T> {
    Signal(S),
    Llm(L),
    Tick(T),
}

impl<S, L, T> Stream for StreamSource<S, L, T>
where
    S: Stream<Item = StreamingLoopEvent> + Unpin,
    L: Stream<Item = StreamingLoopEvent> + Unpin,
    T: Stream<Item = StreamingLoopEvent> + Unpin,
{
    type Item = StreamingLoopEvent;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.get_mut() {
            Self::Signal(s) => Pin::new(s).poll_next(cx),
            Self::Llm(s) => Pin::new(s).poll_next(cx),
            Self::Tick(s) => Pin::new(s).poll_next(cx),
        }
    }
}

/// Spawns a waiting indicator task that prints elapsed time to the terminal.
///
/// Returns `None` if the indicator is disabled (not a TTY or config says no).
fn spawn_waiting_indicator(
    printer: Arc<Printer>,
    config: &StreamingConfig,
    is_tty: bool,
) -> Option<(CancellationToken, JoinHandle<()>)> {
    if !is_tty {
        return None;
    }

    spawn_line_timer(
        printer,
        config.progress.show,
        Duration::from_secs(u64::from(config.progress.delay_secs)),
        Duration::from_millis(u64::from(config.progress.interval_ms)),
        |secs| format!("\r\x1b[K⏱ Waiting… {secs:.1}s"),
    )
}

/// Runs the turn loop: streaming from LLM, handling signals, executing tools.
///
/// This is extracted from `handle_turn` to enable integration testing without
/// requiring a real LLM provider.
/// The function handles the complete turn lifecycle:
///
/// 1. Streaming LLM responses
/// 2. Handling user interrupts (Ctrl+C)
/// 3. Executing tool calls
/// 4. Persisting conversation state
///
/// # Errors
///
/// Returns an error if:
///
/// - LLM streaming fails with a non-retryable error
/// - Tool execution fails critically
/// - Workspace persistence fails
#[expect(clippy::too_many_lines, clippy::too_many_arguments)]
pub(super) async fn run_turn_loop(
    provider: Arc<dyn Provider>,
    model: &ModelDetails,
    cfg: &AppConfig,
    signals: &SignalRx,
    mcp_client: &jp_mcp::Client,
    root: &Utf8Path,
    is_tty: bool,
    attachments: &[Attachment],
    lock: &ConversationLock,
    mut tool_choice: ToolChoice,
    tools: &[ToolDefinition],
    printer: Arc<Printer>,
    prompt_backend: Arc<dyn PromptBackend>,
    mut tool_coordinator: ToolCoordinator,
    chat_request: ChatRequest,
    invocation: InvocationContext,
) -> Result<(), Error> {
    let mut turn_state = TurnState::default();
    let mut stream_retry = StreamRetryState::new(cfg.assistant.request, is_tty);
    let idle_timeout = match cfg.assistant.request.stream_idle_timeout_secs {
        0 => None,
        secs => Some(Duration::from_secs(u64::from(secs))),
    };
    let mut turn_coordinator = TurnCoordinator::new(
        printer.clone(),
        cfg.style.clone(),
        cfg.user.name.clone(),
        cfg.assistant.name.clone(),
        Some(cfg.assistant.model.id.resolved().to_string()),
    );
    let mut tool_renderer = ToolRenderer::new(
        ErrChannel::new(if cfg.style.tool_call.show && !printer.format().is_json() {
            printer.clone()
        } else {
            Printer::sink().into()
        }),
        cfg.style.clone(),
        root.to_path_buf(),
        is_tty,
        invocation,
    );
    // Share the owed-separator flag so visible assistant content rendered by
    // the coordinator can cancel a blank line owed by a preceding tool result.
    turn_coordinator.set_tool_separator(tool_renderer.separator_flag());

    let inquiry_backend: Arc<dyn InquiryBackend> = build_inquiry_backend(
        cfg,
        tools.to_vec(),
        model.clone(),
        provider.clone(),
        attachments.to_vec(),
    )
    .await?;

    info!(model = model.name(), "Starting conversation turn.");

    // Set when an executing phase aborts via user-initiated restart.
    // The next executing phase walks the stream for unresponded requests
    // and re-prepares them.
    let mut restart_requested = false;

    // Id-keyed scratchpad for tool work produced during the streaming
    // phase. The executing phase derives an ordered execution plan from
    // the conversation stream + this scratchpad via `build_execution_plan`.
    // Crucially: there's no public way to enumerate this directly — the
    // stream is the source of truth for "what needs to run."
    let mut pending_tools = PendingTools::new();

    // Prompter shared between streaming (permission prompts) and
    // executing (tool question prompts) phases.
    let prompter = Arc::new(ToolPrompter::with_prompt_backend(
        printer.clone(),
        build_editor_backend(&cfg.editor),
        prompt_backend.clone(),
        reply_edit_mode(cfg.editor.inline.edit_mode),
    ));

    loop {
        match turn_coordinator.current_phase() {
            TurnPhase::Idle => {
                lock.as_mut().update_events(|stream| {
                    turn_coordinator.start_turn(stream, chat_request.clone());
                });
            }

            TurnPhase::Complete | TurnPhase::Aborted => return Ok(()),

            TurnPhase::Streaming => {
                // Restore structural invariants before each provider request.
                // Specifically: any `ToolCallRequest` without a matching
                // `ToolCallResponse` gets a synthetic error response. Without
                // this, providers like Anthropic reject the request with
                // `tool_use ids were found without tool_result blocks`. The
                // top-level `query.rs` sanitize covers the first cycle; this
                // call covers every subsequent cycle within the turn so a
                // mid-turn corruption never reaches the wire.
                lock.as_mut().update_events(ConversationStream::sanitize);

                // Rebuild thread from workspace events to ensure latest context.
                let events_stream = lock.events().clone();

                let thread = build_thread(
                    events_stream,
                    attachments.to_vec(),
                    &cfg.assistant,
                    !tools.is_empty(),
                )?;

                let query = ChatQuery {
                    thread,
                    tools: tools.to_vec(),
                    tool_choice: tool_choice.clone(),
                };

                // Start waiting indicator BEFORE the HTTP request. The drop
                // guard ensures the indicator is cancelled if we exit early
                // (error from run_cycle, break, return).
                let waiting =
                    spawn_waiting_indicator(printer.clone(), &cfg.style.streaming, is_tty);
                let (waiting_token, mut waiting_handle) = match waiting {
                    Some((token, handle)) => (Some(token), Some(handle)),
                    None => (None, None),
                };
                let _waiting_guard = waiting_token
                    .as_ref()
                    .map(CancellationToken::drop_guard_ref);

                // Build the three event sources for the streaming loop.
                let sig_stream = StreamSource::Signal(
                    BroadcastStream::new(signals.resubscribe()).filter_map(|result| {
                        future::ready(match result {
                            Ok(signal) => Some(StreamingLoopEvent::Signal(signal)),
                            Err(BroadcastStreamRecvError::Lagged(n)) => {
                                warn!("Missed {n} signals due to receiver lag");
                                None
                            }
                        })
                    }),
                );

                let raw_stream = provider
                    .chat_completion_stream(model, query)
                    .await
                    .map_err(|e| map_llm_error(e, vec![]))?;
                let raw_stream = match idle_timeout {
                    Some(idle) => with_idle_timeout(raw_stream, idle),
                    None => raw_stream,
                };
                let llm_stream = StreamSource::Llm(
                    raw_stream
                        .fuse()
                        .map(|result| StreamingLoopEvent::Llm(Box::new(result)))
                        // Backstop: if the provider stream ends without a
                        // terminal `Finished` event (a dropped or stalled
                        // connection), the loop would otherwise pend forever —
                        // `SelectAll` only completes once the signal and tick
                        // sources are also exhausted, and those never end.
                        // Append a synthetic transient error so a premature end
                        // routes through the same retry path as any other
                        // stream failure. A normal `Finished` breaks the loop
                        // before this sentinel is ever polled.
                        .chain(stream::once(future::ready(StreamingLoopEvent::Llm(
                            Box::new(Err(StreamError::transient(
                                "provider stream ended without a terminal event",
                            ))),
                        )))),
                );
                turn_state.request_count += 1;

                // Reset preparing display for this streaming cycle.
                tool_renderer.reset();

                // Channel for preparing ticks. The sender is passed to
                // PreparingDisplay which spawns a timer task. The receiver is
                // merged into the event loop via SelectAll.
                let (tick_tx, tick_rx) = mpsc::channel::<Duration>(1);
                let tick_stream = StreamSource::Tick(
                    ReceiverStream::new(tick_rx).map(StreamingLoopEvent::PreparingTick),
                );

                // Whether we've seen at least one provider event this cycle
                // — used to reset the retry budget on the first successful
                // event.
                let mut received_provider_event = false;

                let mut streams: SelectAll<_> =
                    SelectAll::from_iter([sig_stream, llm_stream, tick_stream]);

                let mut conv = lock.as_mut();

                while let Some(event) = streams.next().await {
                    // Cancel and await the waiting indicator on the first
                    // event, ensuring its cleanup (line clear) completes before
                    // we render any content.
                    if let Some(handle) = waiting_handle.take() {
                        if let Some(token) = &waiting_token {
                            token.cancel();
                        }
                        drop(handle.await);
                    }

                    match event {
                        StreamingLoopEvent::Signal(signal) => {
                            // Clear the preparing display before showing the
                            // interrupt menu to avoid visual conflicts.
                            tool_renderer.clear_temp_line();

                            let llm_alive =
                                streams.iter().any(|s| matches!(s, StreamSource::Llm(_)));

                            let action = conv.update_events(|stream| {
                                handle_streaming_signal(
                                    signal,
                                    &mut turn_coordinator,
                                    stream,
                                    &printer,
                                    prompt_backend.as_ref(),
                                    build_editor_backend(&cfg.editor),
                                    reply_edit_mode(cfg.editor.inline.edit_mode),
                                    &cfg.interrupt.streaming,
                                    !llm_alive,
                                )
                            });
                            match action {
                                LoopAction::Continue => {}
                                LoopAction::Break => break,
                                LoopAction::Return(()) => return Ok(()),
                            }
                        }

                        StreamingLoopEvent::Llm(event) => {
                            let event = *event;

                            // Stream errors are handled by the unified retry
                            // logic — the single source of truth for retries.
                            let event = match event {
                                Ok(event) => event,
                                Err(e) => {
                                    tool_renderer.cancel_all();

                                    match handle_stream_error(
                                        e,
                                        &mut stream_retry,
                                        &mut turn_coordinator,
                                        &conv,
                                        &printer,
                                    )
                                    .await
                                    {
                                        LoopAction::Break => break,
                                        LoopAction::Return(result) => {
                                            // Persist any partial content
                                            // flushed before aborting, so a
                                            // fatal stream error doesn't
                                            // discard streamed work.
                                            if let Err(err) = conv.flush() {
                                                warn!("Failed to persist before abort: {err}");
                                            }
                                            return result;
                                        }
                                        LoopAction::Continue => continue,
                                    }
                                }
                            };

                            // Reset the retry counter on the first successful
                            // event in this cycle. This ensures that partially
                            // successful streams (rate-limited mid-response)
                            // don't permanently consume the retry budget.
                            if !received_provider_event {
                                received_provider_event = true;
                                stream_retry.clear_line(&printer);
                                stream_retry.reset();
                            }

                            // Register preparing tool calls. Flush the markdown
                            // buffer first so buffered text appears before the
                            // "Calling tool" line (fixes Issue 1).
                            if let Event::Part {
                                part: EventPart::ToolCall(ToolCallPart::Start { id, name }),
                                ..
                            } = &event
                            {
                                turn_coordinator.flush_renderer();
                                turn_coordinator.transition_to_tool_call();

                                tool_renderer.register(id, name, &tick_tx);
                                tool_coordinator
                                    .set_tool_state(id, ToolCallState::ReceivingArguments {
                                        name: name.clone(),
                                    });
                            }

                            let is_finished = matches!(event, Event::Finished(_));

                            // `handle_llm_event` returns turn control plus
                            // any newly committed event that needs immediate
                            // shell handling. We dispatch on the committed
                            // event, not on the stream tail: a duplicate flush
                            // from a misbehaving provider commits nothing and
                            // so cannot drive a double dispatch.
                            let (action, committed) = conv.update_events(|stream| {
                                handle_llm_event(event, &mut turn_coordinator, stream)
                            });
                            match action {
                                LoopAction::Continue => {}
                                LoopAction::Break => break,
                                LoopAction::Return(()) => return Ok(()),
                            }

                            // On a flushed tool-call request: clear the temp
                            // line, prepare the executor, decide permission,
                            // then render the tool call header + arguments.
                            // For attended tools the permission prompt comes
                            // first, so the user approves before seeing the
                            // full rendering.
                            if let CommittedEvent::ToolCallRequest(req) = committed {
                                tool_coordinator.set_tool_state(&req.id, ToolCallState::Queued);
                                tool_renderer.complete(&req.id);

                                match tool_coordinator.prepare_one(req.clone()) {
                                    Ok(executor) => {
                                        // Run the unified per-tool permission
                                        // pipeline. The await blocks the
                                        // streaming event loop while the user
                                        // decides; LLM events buffer in the
                                        // channel and are processed after.
                                        let decision = tool_coordinator
                                            .resolve_tool_call_decision(
                                                executor,
                                                &prompter,
                                                is_tty,
                                                &mut turn_state,
                                                &tool_renderer,
                                            )
                                            .await;

                                        match decision {
                                            ToolCallDecision::Approved {
                                                executor,
                                                rendered_arguments,
                                            } => {
                                                if let Some(content) = rendered_arguments {
                                                    conv.update_events(|stream| {
                                                        store_rendered_arguments(
                                                            stream, &req.id, &content,
                                                        );
                                                    });
                                                }
                                                pending_tools
                                                    .insert_approved(req.id.clone(), executor);
                                            }
                                            ToolCallDecision::Skipped(resp)
                                            | ToolCallDecision::Failed(resp) => {
                                                pending_tools.insert_resolved(req.id.clone(), resp);
                                            }
                                        }
                                    }
                                    Err(resp) => {
                                        pending_tools.insert_resolved(req.id.clone(), resp);
                                    }
                                }
                            }

                            if is_finished {
                                tool_renderer.cancel_all();
                            }
                        }

                        StreamingLoopEvent::PreparingTick(elapsed) => {
                            tool_renderer.tick(elapsed);
                        }
                    }
                }

                // Clean up any preparing state on early loop exit.
                tool_renderer.cancel_all();

                conv.flush()?;
            }

            TurnPhase::Executing => {
                // On restart: walk the stream for unresponded tool-call
                // requests in the current turn and re-prepare them into
                // `pending_tools` via the existing batch APIs. From there
                // the unified executing path below picks up.
                //
                // The streaming and restart prep flows are still two
                // separate codepaths today; both converge on
                // `pending_tools` and `build_execution_plan`, which is the
                // load-bearing invariant for this refactor.
                if restart_requested {
                    restart_requested = false;

                    let restart_calls: Vec<ToolCallRequest> = lock
                        .events()
                        .iter_turns()
                        .next_back()
                        .map(|t| {
                            t.iter()
                                .filter_map(|e| e.event.as_tool_call_request())
                                .filter(|req| {
                                    lock.events().find_tool_call_response(&req.id).is_none()
                                })
                                .cloned()
                                .collect()
                        })
                        .unwrap_or_default();

                    if restart_calls.is_empty() {
                        break;
                    }

                    let unavailable = tool_coordinator.prepare(restart_calls);
                    let restart_prompter = ToolPrompter::with_prompt_backend(
                        printer.clone(),
                        build_editor_backend(&cfg.editor),
                        prompt_backend.clone(),
                        reply_edit_mode(cfg.editor.inline.edit_mode),
                    );
                    let (executors, skipped) = tool_coordinator
                        .run_permission_phase(
                            &restart_prompter,
                            is_tty,
                            &mut turn_state,
                            &tool_renderer,
                        )
                        .await;

                    for (_idx, exec) in executors {
                        let id = exec.tool_id().to_owned();
                        pending_tools.insert_approved(id, exec);
                    }
                    for (_idx, resp) in skipped {
                        pending_tools.insert_resolved(resp.id.clone(), resp);
                    }
                    for (_idx, resp) in unavailable {
                        pending_tools.insert_resolved(resp.id.clone(), resp);
                    }
                }

                // Unified executing path: derive the work to do by walking
                // the conversation stream and reconciling against
                // `pending_tools`. The stream is the source of truth.
                let mut conv = lock.as_mut();
                let plan = build_execution_plan(&conv.events(), &mut pending_tools);

                if plan.is_empty() {
                    break;
                }

                let (items, orphaned) = plan.into_parts();

                let mut approved: Vec<(usize, Box<dyn Executor>)> = Vec::new();
                let mut pre_resolved: Vec<(usize, ToolCallResponse)> = Vec::new();
                for item in items {
                    match item.work {
                        PendingEntry::Approved(exec) => approved.push((item.index, exec)),
                        PendingEntry::Resolved(resp) => pre_resolved.push((item.index, resp)),
                    }
                }

                // Orphans: a `ToolCallRequest` in the stream's current turn
                // without a matching pending entry. Should never happen in
                // correct operation — every flushed request goes through
                // the prep flow which writes to `pending_tools`. Synthesize
                // an error response so the conversation stays valid (every
                // request must have a response before the next provider
                // call) and surface the inconsistency.
                for (idx, req) in orphaned {
                    warn!(
                        id = %req.id,
                        name = %req.name,
                        "ToolCallRequest in stream without a pending entry; synthesizing error \
                         response.",
                    );
                    pre_resolved.push((idx, ToolCallResponse {
                        id: req.id,
                        result: Err(
                            "Tool call had no prepared executor (internal inconsistency).".into(),
                        ),
                    }));
                }

                tool_coordinator.reset_for_execution();

                let execution_result = tool_coordinator
                    .execute_with_prompting(
                        approved,
                        Arc::clone(&prompter),
                        signals.resubscribe(),
                        &mut turn_coordinator,
                        &mut turn_state,
                        &printer,
                        prompt_backend.as_ref(),
                        build_editor_backend(&cfg.editor),
                        reply_edit_mode(cfg.editor.inline.edit_mode),
                        Arc::clone(&inquiry_backend),
                        &conv,
                        mcp_client,
                        root,
                        &tool_renderer,
                        is_tty,
                    )
                    .await;

                if execution_result.restart_requested {
                    restart_requested = true;
                    continue;
                }

                if commit_tool_responses(
                    execution_result,
                    pre_resolved,
                    &mut tool_coordinator,
                    &mut turn_coordinator,
                    &mut conv,
                )? {
                    tool_choice = ToolChoice::Auto;
                }
            }
        }
    }

    Ok(())
}

async fn build_inquiry_backend(
    cfg: &AppConfig,
    tools: Vec<ToolDefinition>,
    model: ModelDetails,
    provider: Arc<dyn Provider>,
    attachments: Vec<Attachment>,
) -> Result<Arc<LlmInquiryBackend>, Error> {
    let sections = build_sections(&cfg.assistant, !tools.is_empty());
    let inquiry_override = &cfg.conversation.inquiry.assistant;

    // Use the inquiry system prompt if configured, otherwise fall back to the
    // parent assistant's system prompt.
    let default_system_prompt = inquiry_override
        .system_prompt
        .clone()
        .or_else(|| cfg.assistant.system_prompt.clone());

    // Track providers we've already constructed to avoid duplicates.
    let mut providers: IndexMap<ProviderId, Arc<dyn Provider>> = IndexMap::new();

    // Build the default InquiryConfig from the global inquiry override
    // merged with the parent assistant config.
    let default_config = if let Some(inquiry_model_cfg) = inquiry_override.model.as_ref() {
        let inquiry_model_id = inquiry_model_cfg.id.resolved();
        let inquiry_provider: Arc<dyn Provider> =
            Arc::from(get_provider(inquiry_model_id.provider, &cfg.providers.llm)?);
        debug!(model = %inquiry_model_id, "Fetching inquiry model details.");
        let inquiry_model = inquiry_provider
            .model_details(&inquiry_model_id.name)
            .await?;

        if inquiry_model.structured_output == Some(false) {
            warn!(
                model = inquiry_model_id.to_string(),
                "Inquiry model does not support structured output. Inquiry responses may be \
                 unreliable.",
            );
        }

        info!(
            model = inquiry_model.name(),
            "Using dedicated model for inquiries."
        );

        providers.insert(inquiry_model_id.provider, Arc::clone(&inquiry_provider));

        InquiryConfig {
            provider: inquiry_provider,
            model: inquiry_model,
            system_prompt: default_system_prompt,
            sections: sections.clone(),
        }
    } else {
        providers.insert(model.id.provider, Arc::clone(&provider));

        InquiryConfig {
            provider: Arc::clone(&provider),
            model: model.clone(),
            system_prompt: default_system_prompt,
            sections: sections.clone(),
        }
    };

    let overrides = build_inquiry_overrides(cfg, &default_config, &mut providers).await?;

    Ok(Arc::new(LlmInquiryBackend::new(
        default_config,
        overrides,
        attachments,
        tools,
    )))
}

/// Walk active tool configs to build per-question [`InquiryConfig`] overrides
/// from `QuestionTarget::Assistant(config)` entries that have non-empty config.
async fn build_inquiry_overrides(
    cfg: &AppConfig,
    default_config: &InquiryConfig,
    providers: &mut IndexMap<ProviderId, Arc<dyn Provider>>,
) -> Result<IndexMap<(String, String), InquiryConfig>, Error> {
    let mut overrides = IndexMap::new();

    for (tool_name, tool_cfg) in cfg.conversation.tools.iter() {
        for (question_id, question_cfg) in tool_cfg.questions() {
            let QuestionTarget::Assistant(ref per_q) = question_cfg.target else {
                continue;
            };
            if PartialConfig::is_empty(per_q.as_ref()) {
                continue;
            }

            // Resolve per-question model (if overridden), falling back to
            // the default inquiry config's model.
            let has_model_override = !PartialConfig::is_empty(&per_q.model.id);
            let (inq_provider, inq_model) = if has_model_override {
                let model_id = per_q
                    .model
                    .id
                    .resolve(&cfg.providers.llm.aliases)
                    .map_err(|e| Error::CliConfig(e.to_string()))?;

                let prov = if let Some(p) = providers.get(&model_id.provider) {
                    Arc::clone(p)
                } else {
                    let p: Arc<dyn Provider> =
                        Arc::from(get_provider(model_id.provider, &cfg.providers.llm)?);
                    providers.insert(model_id.provider, Arc::clone(&p));
                    p
                };

                let details = prov.model_details(&model_id.name).await?;

                if details.structured_output == Some(false) {
                    warn!(
                        tool = %tool_name,
                        question = %question_id,
                        model = %model_id,
                        "Per-question inquiry model does not support structured \
                         output. Inquiry responses may be unreliable.",
                    );
                }

                (prov, details)
            } else {
                (
                    Arc::clone(&default_config.provider),
                    default_config.model.clone(),
                )
            };

            // System prompt: per-question -> global inquiry -> main.
            let system_prompt = per_q
                .system_prompt
                .as_ref()
                .map(|s| s.to_string())
                .or_else(|| default_config.system_prompt.clone());

            overrides.insert((tool_name.to_owned(), question_id.clone()), InquiryConfig {
                provider: inq_provider,
                model: inq_model,
                system_prompt,
                sections: default_config.sections.clone(),
            });
        }
    }

    Ok(overrides)
}

/// Assemble tool responses from the executor's results plus any pre-resolved
/// responses (skipped tools, unavailable tools, orphan synthesizations), commit
/// them to the conversation stream, and flush to disk.
///
/// Returns `true` if a follow-up LLM cycle is needed (i.e. tool responses were
/// added and the coordinator wants to continue).
fn commit_tool_responses(
    result: ExecutionResult,
    pre_resolved: Vec<(usize, ToolCallResponse)>,
    tool: &mut ToolCoordinator,
    turn: &mut super::turn::TurnCoordinator,
    conv: &mut ConversationMut,
) -> Result<bool, Error> {
    // Persist any rendered custom-argument output accumulated during the
    // permission phase into the corresponding ToolCallRequest events.
    flush_rendered_arguments(tool, conv);

    // Both `result.responses` and `pre_resolved` are already keyed by the
    // plan index assigned in `build_execution_plan`. Sorting by that
    // index restores stream order for the persisted responses.
    let mut indexed: Vec<(usize, ToolCallResponse)> = result.responses;
    indexed.extend(pre_resolved);
    indexed.sort_by_key(|(idx, _)| *idx);
    let responses: Vec<_> = indexed.into_iter().map(|(_, r)| r).collect();

    let action = conv.update_events(|stream| turn.handle_tool_responses(stream, responses));
    conv.flush()?;

    Ok(matches!(action, Action::SendFollowUp))
}

/// Write a single rendered-arguments entry into event metadata.
fn store_rendered_arguments(stream: &mut ConversationStream, tool_call_id: &str, content: &str) {
    for event in stream.iter_mut() {
        let is_match = event
            .event
            .as_tool_call_request()
            .is_some_and(|r| r.id == tool_call_id);
        if is_match {
            set_rendered_arguments(event.event, content);
            return;
        }
    }
}

/// Drain accumulated rendered arguments from the coordinator and write them
/// into the corresponding `ToolCallRequest` events in the stream.
fn flush_rendered_arguments(coordinator: &mut ToolCoordinator, conv: &mut ConversationMut) {
    let rendered = coordinator.drain_rendered_arguments();
    if rendered.is_empty() {
        return;
    }
    conv.update_events(|stream| {
        for (tool_call_id, content) in &rendered {
            store_rendered_arguments(stream, tool_call_id, content);
        }
    });
}

fn map_llm_error(error: jp_llm::Error, models: Vec<ModelDetails>) -> Error {
    match error {
        jp_llm::Error::UnknownModel(model) => Error::UnknownModel {
            model,
            available: models.into_iter().map(|v| v.id.name.to_string()).collect(),
        },
        _ => error.into(),
    }
}

#[cfg(test)]
#[path = "turn_loop_tests.rs"]
mod tests;
