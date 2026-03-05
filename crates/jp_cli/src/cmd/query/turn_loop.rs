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
use futures::{Stream, StreamExt as _, stream::SelectAll};
use jp_attachment::Attachment;
use jp_config::{
    AppConfig, assistant::tool_choice::ToolChoice, conversation::tool::style::ParametersStyle,
    style::streaming::StreamingConfig,
};
use jp_conversation::{
    ConversationId,
    event::{ChatRequest, ToolCallRequest},
};
use jp_inquire::prompt::PromptBackend;
use jp_llm::{
    Provider, error::StreamError, event::Event, model::ModelDetails, query::ChatQuery,
    tool::ToolDefinition,
};
use jp_printer::Printer;
use jp_workspace::Workspace;
use tokio_stream::wrappers::{BroadcastStream, ReceiverStream, errors::BroadcastStreamRecvError};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use super::{
    build_sections, build_thread,
    interrupt::{LoopAction, handle_llm_event, handle_streaming_signal},
    stream::{StreamRetryState, handle_stream_error},
    tool::{
        FormatResult, ToolCallState, ToolCoordinator, ToolPrompter, ToolRenderer,
        inquiry::{InquiryBackend, LlmInquiryBackend},
    },
    turn::{Action, TurnCoordinator, TurnPhase, TurnState},
};
use crate::{
    error::Error,
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
    /// Async argument formatting completed (Custom style).
    FormatComplete(FormatResult),
}

/// Wrapper enum that unifies heterogeneous stream sources for
/// [`SelectAll`].
///
/// Each variant holds a different concrete stream type, but they all
/// yield [`StreamingLoopEvent`]. This avoids boxing while allowing
/// `select_all` to poll them as a single merged stream.
enum StreamSource<S, L, T, F> {
    Signal(S),
    Llm(L),
    Tick(T),
    Format(F),
}

impl<S, L, T, F> Stream for StreamSource<S, L, T, F>
where
    S: Stream<Item = StreamingLoopEvent> + Unpin,
    L: Stream<Item = StreamingLoopEvent> + Unpin,
    T: Stream<Item = StreamingLoopEvent> + Unpin,
    F: Stream<Item = StreamingLoopEvent> + Unpin,
{
    type Item = StreamingLoopEvent;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.get_mut() {
            Self::Signal(s) => Pin::new(s).poll_next(cx),
            Self::Llm(s) => Pin::new(s).poll_next(cx),
            Self::Tick(s) => Pin::new(s).poll_next(cx),
            Self::Format(s) => Pin::new(s).poll_next(cx),
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
) -> Option<(CancellationToken, tokio::task::JoinHandle<()>)> {
    if !is_tty {
        return None;
    }

    super::tool::spawn_line_timer(
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
    workspace: &mut Workspace,
    mut tool_choice: ToolChoice,
    tools: &[ToolDefinition],
    conversation_id: ConversationId,
    printer: Arc<Printer>,
    prompt_backend: Arc<dyn PromptBackend>,
    mut tool_coordinator: ToolCoordinator,
    chat_request: ChatRequest,
) -> Result<(), Error> {
    let mut turn_state = TurnState::default();
    let mut stream_retry = StreamRetryState::new(cfg.assistant.request);
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

    let sections = build_sections(&cfg.assistant, !tools.is_empty());
    let inquiry_backend: Arc<dyn InquiryBackend> = Arc::new(LlmInquiryBackend::new(
        Arc::clone(&provider),
        model.clone(),
        cfg.assistant.system_prompt.clone().map(String::from),
        sections,
        attachments.to_vec(),
    ));

    info!(model = model.name(), "Starting conversation turn.");

    // Track any tool call that needs to be restarted before the turn ends.
    let mut pending_restart_calls: Option<Vec<ToolCallRequest>> = None;

    loop {
        match turn_coordinator.current_phase() {
            TurnPhase::Idle => {
                let conversation_stream = workspace
                    .get_events_mut(&conversation_id)
                    .expect("conversation must exist");

                turn_coordinator.start_turn(conversation_stream, chat_request.clone());
            }

            TurnPhase::Complete | TurnPhase::Aborted => return Ok(()),

            TurnPhase::Streaming => {
                // Rebuild thread from workspace events to ensure latest context.
                let events_stream = workspace
                    .get_events(&conversation_id)
                    .expect("conversation must exist")
                    .clone();

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
                        futures::future::ready(match result {
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
                        .map_err(|e| {
                            // Convert to cli Error for the ? below.
                            map_llm_error(e, vec![])
                        })?
                        .fuse()
                        .map(|e| StreamingLoopEvent::Llm(Box::new(e))),
                );
                turn_state.request_count += 1;

                // Reset preparing display for this streaming cycle.
                tool_renderer.reset();

                // Channel for preparing ticks. The sender is passed to
                // PreparingDisplay which spawns a timer task. The receiver
                // is merged into the event loop via SelectAll.
                let (tick_tx, tick_rx) = tokio::sync::mpsc::channel::<Duration>(1);
                let tick_stream = StreamSource::Tick(
                    ReceiverStream::new(tick_rx).map(StreamingLoopEvent::PreparingTick),
                );

                // Channel for async argument formatting results (Custom style).
                let (format_tx, format_rx) = tokio::sync::mpsc::channel::<FormatResult>(16);
                let format_stream = StreamSource::Format(
                    ReceiverStream::new(format_rx).map(StreamingLoopEvent::FormatComplete),
                );

                let mut streams: SelectAll<_> =
                    SelectAll::from_iter([sig_stream, llm_stream, tick_stream, format_stream]);

                let conversation_stream = workspace
                    .get_events_mut(&conversation_id)
                    .expect("conversation must exist");

                while let Some(event) = streams.next().await {
                    // Cancel and await the waiting indicator on the first
                    // event, ensuring its cleanup (line clear) completes
                    // before we render any content.
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

                            match handle_streaming_signal(
                                signal,
                                &mut turn_coordinator,
                                conversation_stream,
                                &printer,
                                prompt_backend.as_ref(),
                                !llm_alive,
                            ) {
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
                                        conversation_stream,
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

                            // Register preparing tool calls. Flush the
                            // markdown buffer first so buffered text appears
                            // before the "Calling tool" line (fixes Issue 1).
                            if let Event::Part { ref event, .. } = event
                                && let Some(req) = event.as_tool_call_request()
                            {
                                turn_coordinator.flush_renderer();
                                turn_coordinator.reset_content_kind();

                                tool_renderer.register(&req.id, &req.name, &tick_tx);
                                tool_coordinator.set_tool_state(
                                    &req.id,
                                    ToolCallState::ReceivingArguments {
                                        name: req.name.clone(),
                                    },
                                );
                            }

                            let is_flush = matches!(event, Event::Flush { .. });
                            let is_finished = matches!(event, Event::Finished(_));

                            match handle_llm_event(
                                event,
                                &mut turn_coordinator,
                                conversation_stream,
                            ) {
                                LoopAction::Continue => {}
                                LoopAction::Break => break,
                                LoopAction::Return(()) => return Ok(()),
                            }

                            // On Flush of a tool call: format arguments and
                            // print the permanent line (or spawn async for
                            // Custom style).
                            if is_flush
                                && let Some(last) = conversation_stream.last()
                                && let Some(req) = last.as_tool_call_request()
                            {
                                tool_coordinator.set_tool_state(&req.id, ToolCallState::Queued);

                                let style = tool_coordinator.parameter_style(&req.name);
                                if matches!(style, ParametersStyle::Custom(_)) {
                                    // Spawn async formatting for Custom style.
                                    let format_root = root.to_path_buf();
                                    let id = req.id.clone();
                                    let name = req.name.clone();
                                    let args = req.arguments.clone();
                                    let tx = format_tx.clone();
                                    tokio::spawn(async move {
                                        let formatted = super::tool::renderer::format_args(
                                            &name,
                                            &args,
                                            &style,
                                            &format_root,
                                        )
                                        .await;
                                        drop(
                                            tx.send(FormatResult {
                                                id,
                                                name,
                                                formatted,
                                            })
                                            .await,
                                        );
                                    });
                                } else {
                                    // Non-Custom styles are pure formatting (no I/O).
                                    let formatted = super::tool::renderer::format_args(
                                        &req.name,
                                        &req.arguments,
                                        &style,
                                        root,
                                    )
                                    .await;
                                    match formatted {
                                        Ok(args) => {
                                            tool_renderer.complete(&req.id, &req.name, &args);
                                        }
                                        Err(_) => tool_renderer.remove_pending(&req.id),
                                    }
                                }
                            }

                            if is_finished {
                                stream_retry.reset();
                                tool_renderer.cancel_all();
                            }
                        }

                        StreamingLoopEvent::PreparingTick(elapsed) => {
                            tool_renderer.tick(elapsed);
                        }

                        StreamingLoopEvent::FormatComplete(result) => match result.formatted {
                            Ok(formatted) => {
                                tool_renderer.complete(&result.id, &result.name, &formatted);
                            }
                            Err(_) => {
                                tool_renderer.remove_pending(&result.id);
                            }
                        },
                    }
                }

                // Clean up any preparing state on early loop exit.
                tool_renderer.cancel_all();

                workspace.persist_active_conversation()?;
            }

            TurnPhase::Executing => {
                // Use restart calls if available, otherwise take from
                // coordinator. These are mutually exclusive:
                //
                // - Restart: we stayed in Executing, so coordinator has no new
                //   calls.
                // - Normal: pending_restart_calls is None, take from
                //   coordinator.
                let calls = pending_restart_calls
                    .take()
                    .unwrap_or_else(|| turn_coordinator.take_pending_tool_calls());

                if calls.is_empty() {
                    break;
                }

                // Store tool calls for potential restart
                let original_calls = calls.clone();

                if let Err(error) = tool_coordinator.prepare(calls, mcp_client).await {
                    error!(error = error.to_string(), "Failed to prepare tools");
                }

                // Note: cancellation_token is handled internally by execute_with_prompting

                // Run permission phase - prompts user for each tool that needs it.
                // This sets tool states to AwaitingPermission during prompts.
                // Use the injected prompt backend for testability.
                let prompter = ToolPrompter::with_prompt_backend(
                    printer.clone(),
                    cfg.editor.path(),
                    prompt_backend.clone(),
                );
                let (executors, skipped_responses) = tool_coordinator
                    .run_permission_phase(&prompter, mcp_client, is_tty, &mut turn_state)
                    .await;

                // Execute approved tools with streaming results and prompting
                let inquiry_events = workspace
                    .get_events_mut(&conversation_id)
                    .expect("conversation must exist");

                let execution_result = tool_coordinator
                    .execute_with_prompting(
                        executors,
                        prompter.into(),
                        signals.resubscribe(),
                        &mut turn_coordinator,
                        &mut turn_state,
                        &printer,
                        prompt_backend.as_ref(),
                        Arc::clone(&inquiry_backend),
                        inquiry_events,
                        mcp_client,
                        root,
                        &tool_renderer,
                        is_tty,
                    )
                    .await;

                // If restart was requested, re-execute with the original calls
                // instead of adding the cancelled responses to the conversation
                if execution_result.restart_requested {
                    pending_restart_calls = Some(original_calls);
                    continue;
                }

                // Combine execution responses with skipped responses and sort
                // by original index to maintain request order
                let mut indexed_responses: Vec<(usize, _)> =
                    execution_result.responses.into_iter().enumerate().collect();
                indexed_responses.extend(skipped_responses);
                indexed_responses.sort_by_key(|(index, _)| *index);
                let responses: Vec<_> = indexed_responses.into_iter().map(|(_, r)| r).collect();

                let ws_stream = workspace
                    .get_events_mut(&conversation_id)
                    .expect("conversation must exist");

                if let Action::SendFollowUp =
                    turn_coordinator.handle_tool_responses(ws_stream, responses)
                {
                    tool_choice = ToolChoice::Auto;
                }

                workspace.persist_active_conversation()?;
            }
        }
    }

    Ok(())
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
