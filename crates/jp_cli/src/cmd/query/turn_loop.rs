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
use futures::{Stream, StreamExt as _, future, stream::SelectAll};
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
    tool::{ToolDefinition, executor::Executor},
};
use jp_printer::Printer;
use jp_workspace::{ConversationLock, ConversationMut};
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_stream::wrappers::{BroadcastStream, ReceiverStream, errors::BroadcastStreamRecvError};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::{
    build_sections, build_thread,
    interrupt::{LoopAction, handle_llm_event, handle_streaming_signal},
    stream::{StreamRetryState, handle_stream_error},
    tool::{
        PermissionDecision, ToolCallState, ToolCoordinator, ToolPrompter, ToolRenderer,
        inquiry::{InquiryBackend, InquiryConfig, LlmInquiryBackend},
        spawn_line_timer,
    },
    turn::{Action, TurnCoordinator, TurnPhase, TurnState},
};
use crate::{
    cmd::query::tool::coordinator::ExecutionResult,
    error::Error,
    render::{metadata::set_rendered_arguments, tool::RenderOutcome},
    signals::{SignalRx, SignalTo},
};

/// Events produced by the merged streaming loop sources.
enum StreamingLoopEvent {
    /// A signal from the signal handler (e.g. Ctrl+C).
    Signal(SignalTo),
    /// An event from the LLM provider stream.
    Llm(Box<Result<Event, StreamError>>),
    /// A tick from the preparing indicator timer, carrying the elapsed
    /// time since the timer started.
    PreparingTick(Duration),
}

/// Wrapper enum that unifies heterogeneous stream sources for
/// [`SelectAll`].
///
/// Each variant holds a different concrete stream type, but they all
/// yield [`StreamingLoopEvent`]. This avoids boxing while allowing
/// `select_all` to poll them as a single merged stream.
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
/// This is extracted from `handle_turn` to enable integration testing
/// without requiring a real LLM provider. The function handles the complete
/// turn lifecycle:
///
/// 1. Streaming LLM responses
/// 2. Handling user interrupts (Ctrl+C)
/// 3. Executing tool calls
/// 4. Persisting conversation state
///
/// # Errors
///
/// Returns an error if:
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
) -> Result<(), Error> {
    let mut turn_state = TurnState::default();
    let mut stream_retry = StreamRetryState::new(cfg.assistant.request, is_tty);
    let mut turn_coordinator = TurnCoordinator::new(printer.clone(), cfg.style.clone());
    let mut tool_renderer = ToolRenderer::new(
        if cfg.style.tool_call.show && !printer.format().is_json() {
            printer.clone()
        } else {
            Printer::sink().into()
        },
        cfg.style.clone(),
        root.to_path_buf(),
        is_tty,
    );

    let inquiry_backend: Arc<dyn InquiryBackend> = build_inquiry_backend(
        cfg,
        tools.to_vec(),
        model.clone(),
        provider.clone(),
        attachments.to_vec(),
    )
    .await?;

    info!(model = model.name(), "Starting conversation turn.");

    // Track any tool call that needs to be restarted before the turn ends.
    let mut pending_restart_calls: Option<Vec<ToolCallRequest>> = None;

    // Permission results collected during the streaming phase,
    // consumed by the executing phase: (approved, skipped, unavailable).
    #[allow(clippy::type_complexity)]
    let mut streaming_perm_results: Option<(
        Vec<(usize, Box<dyn Executor>)>,
        Vec<(usize, ToolCallResponse)>,
        Vec<(usize, ToolCallResponse)>,
    )> = None;

    // Prompter shared between streaming (permission prompts) and
    // executing (tool question prompts) phases.
    let prompter = Arc::new(ToolPrompter::with_prompt_backend(
        printer.clone(),
        cfg.editor.path(),
        prompt_backend.clone(),
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

                let llm_stream = StreamSource::Llm(
                    provider
                        .chat_completion_stream(model, query)
                        .await
                        .map_err(|e| map_llm_error(e, vec![]))?
                        .fuse()
                        .map(|result| StreamingLoopEvent::Llm(Box::new(result))),
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

                // Permission results collected during the streaming phase.
                let mut perm_approved = vec![];
                let mut perm_skipped = vec![];
                let mut perm_unavailable = vec![];
                let mut perm_tool_index: usize = 0;
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
                                        LoopAction::Return(result) => return result,
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

                            let is_flush = matches!(event, Event::Flush { .. });
                            let is_finished = matches!(event, Event::Finished(_));

                            let action = conv.update_events(|stream| {
                                handle_llm_event(event, &mut turn_coordinator, stream)
                            });
                            match action {
                                LoopAction::Continue => {}
                                LoopAction::Break => break,
                                LoopAction::Return(()) => return Ok(()),
                            }

                            // On Flush of a tool call: clear the temp line,
                            // prepare the executor, decide permission, then
                            // render the tool call header + arguments. For
                            // attended tools the permission prompt comes first,
                            // so the user approves before seeing the full
                            // rendering.
                            let flushed_req = is_flush
                                .then(|| {
                                    conv.events()
                                        .last()
                                        .as_ref()
                                        .and_then(|e| e.as_tool_call_request())
                                        .cloned()
                                })
                                .flatten();
                            if let Some(req) = flushed_req {
                                tool_coordinator.set_tool_state(&req.id, ToolCallState::Queued);
                                tool_renderer.complete(&req.id);

                                // Prepare executor and decide permission.
                                let idx = perm_tool_index;
                                perm_tool_index += 1;

                                match tool_coordinator.prepare_one(req.clone()) {
                                    Ok(executor) => {
                                        let decision = tool_coordinator.decide_permission(
                                            executor,
                                            is_tty,
                                            &turn_state,
                                        );
                                        match decision {
                                            PermissionDecision::Approved(exec) => {
                                                let id = exec.tool_id().to_owned();
                                                let name = exec.tool_name().to_owned();
                                                let args = exec.arguments().clone();
                                                match tool_coordinator
                                                    .render_approved_tool(
                                                        &name,
                                                        &args,
                                                        &tool_renderer,
                                                    )
                                                    .await
                                                {
                                                    RenderOutcome::Rendered { content } => {
                                                        if let Some(c) = content {
                                                            conv.update_events(|stream| {
                                                                store_rendered_arguments(
                                                                    stream, &req.id, &c,
                                                                );
                                                            });
                                                        }
                                                        perm_approved.push((idx, exec));
                                                    }
                                                    RenderOutcome::Suppressed { error } => {
                                                        perm_skipped.push((
                                                            idx,
                                                            ToolCoordinator::render_failed_response(
                                                                id, &name, &error,
                                                            ),
                                                        ));
                                                    }
                                                }
                                            }
                                            PermissionDecision::Skipped(resp) => {
                                                perm_skipped.push((idx, resp));
                                            }
                                            PermissionDecision::NeedsPrompt {
                                                executor: exec,
                                                info,
                                            } => {
                                                tool_coordinator.set_tool_state(
                                                    &info.tool_id,
                                                    ToolCallState::AwaitingPermission,
                                                );
                                                // Await the prompt inline. This pauses
                                                // the event loop — LLM events buffer in
                                                // the channel and are processed after
                                                // the user answers.
                                                let result = prompter
                                                    .prompt_permission(&info, mcp_client)
                                                    .await;
                                                match tool_coordinator.apply_permission_result(
                                                    result,
                                                    &info,
                                                    &mut turn_state,
                                                    exec,
                                                ) {
                                                    Ok(exec) => {
                                                        let args = exec.arguments().clone();
                                                        match tool_coordinator
                                                            .render_approved_tool(
                                                                &info.tool_name,
                                                                &args,
                                                                &tool_renderer,
                                                            )
                                                            .await
                                                        {
                                                            RenderOutcome::Rendered { content } => {
                                                                if let Some(c) = content {
                                                                    conv.update_events(|stream| {
                                                                        store_rendered_arguments(
                                                                            stream, &req.id, &c,
                                                                        );
                                                                    });
                                                                }
                                                                perm_approved.push((idx, exec));
                                                            }
                                                            RenderOutcome::Suppressed { error } => {
                                                                perm_skipped.push((
                                                                    idx,
                                                                    ToolCoordinator::render_failed_response(
                                                                        info.tool_id.clone(),
                                                                        &info.tool_name,
                                                                        &error,
                                                                    ),
                                                                ));
                                                            }
                                                        }
                                                    }
                                                    Err(resp) => {
                                                        perm_skipped.push((idx, resp));
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    Err(resp) => perm_unavailable.push((idx, resp)),
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

                // Stash streaming-phase permission results for the
                // executing phase to consume.
                streaming_perm_results = Some((perm_approved, perm_skipped, perm_unavailable));

                conv.flush()?;
            }

            TurnPhase::Executing => {
                // If restarting, use the batch path (re-prepare + full
                // permission phase). Otherwise consume the results
                // collected during the streaming phase.
                if let Some(restart_calls) = pending_restart_calls.take() {
                    if restart_calls.is_empty() {
                        break;
                    }

                    let original_calls = restart_calls.clone();
                    let unavailable_responses = tool_coordinator.prepare(restart_calls);
                    let restart_prompter = ToolPrompter::with_prompt_backend(
                        printer.clone(),
                        cfg.editor.path(),
                        prompt_backend.clone(),
                    );
                    let (executors, skipped_responses) = tool_coordinator
                        .run_permission_phase(
                            &restart_prompter,
                            mcp_client,
                            is_tty,
                            &mut turn_state,
                            &tool_renderer,
                        )
                        .await;

                    let mut conv = lock.as_mut();
                    let execution_result = tool_coordinator
                        .execute_with_prompting(
                            executors,
                            restart_prompter.into(),
                            signals.resubscribe(),
                            &mut turn_coordinator,
                            &mut turn_state,
                            &printer,
                            prompt_backend.as_ref(),
                            Arc::clone(&inquiry_backend),
                            &conv,
                            mcp_client,
                            root,
                            &tool_renderer,
                            is_tty,
                        )
                        .await;

                    if execution_result.restart_requested {
                        pending_restart_calls = Some(original_calls);
                        continue;
                    }

                    if commit_tool_responses(
                        execution_result,
                        skipped_responses,
                        unavailable_responses,
                        &mut tool_coordinator,
                        &mut turn_coordinator,
                        &mut conv,
                    )? {
                        tool_choice = ToolChoice::Auto;
                    }
                    continue;
                }

                // Normal path: consume results collected during streaming.
                let (approved, skipped, unavailable) =
                    streaming_perm_results.take().unwrap_or_default();

                // Save original calls for potential restart.
                let original_calls = turn_coordinator.take_pending_tool_calls();

                if approved.is_empty() && skipped.is_empty() && unavailable.is_empty() {
                    break;
                }

                // Reset coordinator state for the execution phase.
                tool_coordinator.reset_for_execution();

                let mut conv = lock.as_mut();
                let execution_result = tool_coordinator
                    .execute_with_prompting(
                        approved,
                        Arc::clone(&prompter),
                        signals.resubscribe(),
                        &mut turn_coordinator,
                        &mut turn_state,
                        &printer,
                        prompt_backend.as_ref(),
                        Arc::clone(&inquiry_backend),
                        &conv,
                        mcp_client,
                        root,
                        &tool_renderer,
                        is_tty,
                    )
                    .await;

                if execution_result.restart_requested {
                    pending_restart_calls = Some(original_calls);
                    continue;
                }

                if commit_tool_responses(
                    execution_result,
                    skipped,
                    unavailable,
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

/// Assemble tool responses from execution, skipped, and unavailable results,
/// commit them to the conversation stream, and flush to disk.
///
/// Returns `true` if a follow-up LLM cycle is needed (i.e. tool responses were
/// added and the coordinator wants to continue).
fn commit_tool_responses(
    result: ExecutionResult,
    skipped: Vec<(usize, ToolCallResponse)>,
    unavailable: Vec<(usize, ToolCallResponse)>,
    tool: &mut ToolCoordinator,
    turn: &mut super::turn::TurnCoordinator,
    conv: &mut ConversationMut,
) -> Result<bool, Error> {
    // Persist any rendered custom-argument output accumulated during the
    // permission phase into the corresponding ToolCallRequest events.
    flush_rendered_arguments(tool, conv);

    let mut indexed: Vec<_> = result.responses.into_iter().enumerate().collect();
    indexed.extend(skipped);
    indexed.extend(unavailable);
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
