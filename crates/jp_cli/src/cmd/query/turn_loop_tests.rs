use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use camino_tempfile::tempdir;
use futures::stream;
use indexmap::IndexMap;
use jp_config::{
    AppConfig, PartialAppConfig,
    conversation::tool::{
        CommandConfigOrString, PartialCommandConfigOrString, PartialToolConfig, QuestionConfig,
        QuestionTarget, RunMode, ToolConfig, ToolSource,
        style::{DisplayStyleConfig, InlineResults, LinkStyle, ParametersStyle},
    },
    model::id::{self, Name, PartialModelIdConfig, ProviderId},
};
use jp_conversation::{
    Conversation, ConversationEvent,
    event::{ChatRequest, ChatResponse, InquirySource, TurnStart},
};
use jp_inquire::prompt::MockPromptBackend;
use jp_llm::{
    Error as LlmError, EventStream, Provider,
    event::{Event, FinishReason},
    model::ModelDetails,
    provider::mock::MockProvider,
    query::ChatQuery,
    tool::{
        builtin::BuiltinExecutors,
        executor::{
            Executor, ExecutorResult, ExecutorSource, MockExecutor, PermissionInfo,
            TestExecutorSource,
        },
    },
};
use jp_printer::{OutputFormat, Printer};
use jp_storage::Storage;
use jp_tool::{AnswerType, Question};
use jp_workspace::Workspace;
use schematic::Config as _;
use serde_json::{Map, Value, json};
use tokio::{sync::broadcast, time::timeout};

use super::*;
use crate::{
    cmd::query::tool::{ToolCoordinator, executor::TerminalExecutorSource},
    signals::SignalTo,
};

fn empty_executor_source() -> Box<dyn ExecutorSource> {
    Box::new(TerminalExecutorSource::new(BuiltinExecutors::new(), &[]))
}

/// A mock provider that returns different responses on each call.
///
/// This enables testing multi-cycle conversations where the LLM returns
/// tool calls on the first request, then a final message on the follow-up.
#[derive(Debug)]
struct SequentialMockProvider {
    /// Sequence of event lists to return on each call.
    responses: Vec<Vec<Event>>,

    /// Current call index (atomic for interior mutability in async trait).
    call_index: AtomicUsize,

    /// Model details to return.
    model: ModelDetails,
}

impl SequentialMockProvider {
    /// Create a provider that returns tool calls first, then a message.
    fn with_tool_then_message(tool_id: &str, tool_name: &str, final_message: &str) -> Self {
        // First response: tool call
        let tool_call_events = vec![
            Event::Part {
                index: 0,
                event: ConversationEvent::now(ToolCallRequest {
                    id: tool_id.to_string(),
                    name: tool_name.to_string(),
                    arguments: Map::new(),
                }),
            },
            Event::flush(0),
            Event::Finished(FinishReason::Completed),
        ];

        // Second response: final message
        let message_events = vec![
            Event::Part {
                index: 0,
                event: ConversationEvent::now(ChatResponse::message(final_message)),
            },
            Event::flush(0),
            Event::Finished(FinishReason::Completed),
        ];

        Self {
            responses: vec![tool_call_events, message_events],
            call_index: AtomicUsize::new(0),
            model: ModelDetails::empty(id::ModelIdConfig {
                provider: ProviderId::Test,
                name: "sequential-mock".parse().expect("valid name"),
            }),
        }
    }
}

#[async_trait]
impl Provider for SequentialMockProvider {
    async fn model_details(&self, name: &id::Name) -> Result<ModelDetails, LlmError> {
        let mut model = self.model.clone();
        model.id.name = name.clone();
        Ok(model)
    }

    async fn models(&self) -> Result<Vec<ModelDetails>, LlmError> {
        Ok(vec![self.model.clone()])
    }

    async fn chat_completion_stream(
        &self,
        _model: &ModelDetails,
        _query: ChatQuery,
    ) -> Result<EventStream, LlmError> {
        let index = self.call_index.fetch_add(1, Ordering::SeqCst);
        let events = self
            .responses
            .get(index)
            .cloned()
            .unwrap_or_else(|| vec![Event::Finished(FinishReason::Completed)]);

        Ok(Box::pin(stream::iter(events.into_iter().map(Ok))))
    }
}

#[tokio::test]
async fn test_quit_during_streaming_persists_content() {
    // 1. Create workspace with real file persistence
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let storage = root.join(".jp");

    let config = AppConfig::new_test();
    let mut workspace = Workspace::new(root)
        .persisted_at(&storage)
        .expect("failed to enable persistence");

    // Create a conversation with an initial user query
    let conv_id = workspace.create_conversation(Conversation::default(), config.clone().into());

    let handle = workspace.acquire_conversation(&conv_id).unwrap();
    let lock = workspace.test_lock(handle);

    let chat_request = ChatRequest::from("What is 2+2?");

    // 2. Create mock provider with content
    let response_content = "The answer is 4.";
    let provider: Arc<dyn Provider> = Arc::new(MockProvider::with_message(response_content));
    let model = provider
        .model_details(&"test-model".parse().unwrap())
        .await
        .unwrap();

    // 3. Set up other dependencies
    let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let mcp_client = jp_mcp::Client::default();
    let (signal_tx, signal_rx) = broadcast::channel(16);
    let _turn_state = TurnState::default();

    // 4. Send Quit signal before starting (it will be received during streaming)
    signal_tx.send(SignalTo::Quit).unwrap();

    // Run the turn loop - it will receive the Quit signal and persist
    let result = run_turn_loop(
        Arc::clone(&provider),
        &model,
        &config,
        &signal_rx,
        &mcp_client,
        root,
        false, // is_tty
        &[],   // attachments
        &lock,
        ToolChoice::Auto,
        &[], // tools
        printer.clone(),
        Arc::new(MockPromptBackend::new()),
        ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
        chat_request.clone(),
    )
    .await;

    // The turn loop should complete successfully (Quit triggers graceful exit)
    assert!(result.is_ok(), "Turn loop should complete: {result:?}");

    // Verify printer output - may be partial due to Quit signal
    printer.flush();
    let output = out.lock();
    // Output may be empty or partial depending on timing, but should not error
    drop(output);

    // 5. Verify file on disk contains the content
    let reader = Storage::new(&storage).unwrap();
    let content = reader
        .read_test_events_raw(&conv_id)
        .expect("events should be persisted");

    // The persisted content should contain the user query
    assert!(
        content.contains("What is 2+2?"),
        "Persisted events should contain the user query.\nFile contents:\n{content}"
    );
}

#[tokio::test]
async fn test_normal_completion_persists_content() {
    // This test verifies normal (non-interrupted) completion also persists correctly
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let storage = root.join(".jp");

    let config = AppConfig::new_test();
    let mut workspace = Workspace::new(root)
        .persisted_at(&storage)
        .expect("failed to enable persistence");

    let conv_id = workspace.create_conversation(Conversation::default(), config.clone().into());

    let handle = workspace.acquire_conversation(&conv_id).unwrap();
    let lock = workspace.test_lock(handle);

    let chat_request = ChatRequest::from("Hello");

    let response_content = "Hello! How can I help you today?";
    let provider: Arc<dyn Provider> = Arc::new(MockProvider::with_message(response_content));
    let model = provider
        .model_details(&"test-model".parse().unwrap())
        .await
        .unwrap();

    let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let mcp_client = jp_mcp::Client::default();
    let (_signal_tx, signal_rx) = broadcast::channel(16);

    run_turn_loop(
        Arc::clone(&provider),
        &model,
        &config,
        &signal_rx,
        &mcp_client,
        root,
        false,
        &[],
        &lock,
        ToolChoice::Auto,
        &[],
        printer.clone(),
        Arc::new(MockPromptBackend::new()),
        ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
        chat_request.clone(),
    )
    .await
    .unwrap();

    // Verify printer output contains the LLM response
    // Note: markdown renderer may escape special characters like '!' → '\!'
    printer.flush();
    let output = out.lock();
    assert!(
        output.contains("How can I help you"),
        "Printer output should contain LLM response.\nOutput:\n{output}"
    );

    // Verify persistence
    let reader = Storage::new(&storage).unwrap();
    let content = reader
        .read_test_events_raw(&conv_id)
        .expect("events should be persisted");

    assert!(
        content.contains("Hello"),
        "Should contain user query.\nFile contents:\n{content}"
    );
    assert!(
        content.contains(response_content),
        "Should contain assistant response.\nFile contents:\n{content}"
    );
}

#[tokio::test]
async fn test_tool_call_cycle_completes_with_followup() {
    // Tests the full tool execution cycle:
    // 1. LLM returns a tool call
    // 2. Tool execution phase runs (tool not found, but cycle continues)
    // 3. LLM returns final message
    // 4. Conversation persists with tool call and final response

    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let storage = root.join(".jp");

    let config = AppConfig::new_test();
    let mut workspace = Workspace::new(root)
        .persisted_at(&storage)
        .expect("failed to enable persistence");

    let conv_id = workspace.create_conversation(Conversation::default(), config.clone().into());

    let handle = workspace.acquire_conversation(&conv_id).unwrap();
    let lock = workspace.test_lock(handle);

    let chat_request = ChatRequest::from("List files in current directory");

    // Provider returns tool call first, then message
    let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider::with_tool_then_message(
        "call_123",
        "fs_list_files",
        "Here are the files in the directory.",
    ));
    let model = provider
        .model_details(&"test-model".parse().unwrap())
        .await
        .unwrap();

    let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let mcp_client = jp_mcp::Client::default();
    let (_signal_tx, signal_rx) = broadcast::channel(16);

    let result = run_turn_loop(
        Arc::clone(&provider),
        &model,
        &config,
        &signal_rx,
        &mcp_client,
        root,
        false,
        &[],
        &lock,
        ToolChoice::Auto,
        &[], // No tool definitions - tests the "tool not found" path
        printer.clone(),
        Arc::new(MockPromptBackend::new()),
        ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
        chat_request.clone(),
    )
    .await;

    assert!(result.is_ok(), "Turn loop should complete: {result:?}");

    // Verify printer output contains final LLM response
    printer.flush();
    let output = out.lock();
    assert!(
        output.contains("Here are the files"),
        "Printer output should contain final LLM response.\nOutput:\n{output}"
    );

    // Verify persistence
    let reader = Storage::new(&storage).unwrap();
    let content = reader
        .read_test_events_raw(&conv_id)
        .expect("events should be persisted");

    // Should contain the user query
    assert!(
        content.contains("List files"),
        "Should contain user query.\nFile contents:\n{content}"
    );

    // Should contain the tool call request
    assert!(
        content.contains("fs_list_files") || content.contains("call_123"),
        "Should contain tool call.\nFile contents:\n{content}"
    );

    // Should contain the final message
    assert!(
        content.contains("Here are the files"),
        "Should contain final response.\nFile contents:\n{content}"
    );
}

#[tokio::test]
async fn test_quit_during_tool_execution_persists() {
    // Tests that Quit signal during tool execution still persists content.
    // The tool call should be saved even if execution is interrupted.

    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let storage = root.join(".jp");

    let config = AppConfig::new_test();
    let mut workspace = Workspace::new(root)
        .persisted_at(&storage)
        .expect("failed to enable persistence");

    let conv_id = workspace.create_conversation(Conversation::default(), config.clone().into());

    let handle = workspace.acquire_conversation(&conv_id).unwrap();
    let lock = workspace.test_lock(handle);

    let chat_request = ChatRequest::from("Run a tool");

    // Provider returns tool call (we'll quit during execution phase)
    let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider::with_tool_then_message(
        "call_456",
        "some_tool",
        "This message should not appear",
    ));
    let model = provider
        .model_details(&"test-model".parse().unwrap())
        .await
        .unwrap();

    let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let mcp_client = jp_mcp::Client::default();
    let (signal_tx, signal_rx) = broadcast::channel(16);

    // We need to send Quit at the right moment. Since tool execution
    // happens quickly (tool not found = immediate return), we spawn
    // a task that sends Quit with a small delay.
    let signal_handle = tokio::spawn(async move {
        // Small delay to let streaming complete and enter executing phase
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        let _ = signal_tx.send(SignalTo::Quit);
    });

    let result = run_turn_loop(
        Arc::clone(&provider),
        &model,
        &config,
        &signal_rx,
        &mcp_client,
        root,
        false,
        &[],
        &lock,
        ToolChoice::Auto,
        &[],
        printer.clone(),
        Arc::new(MockPromptBackend::new()),
        ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
        chat_request.clone(),
    )
    .await;

    signal_handle.await.unwrap();

    // Should complete (either normally or via quit)
    assert!(result.is_ok(), "Turn loop should complete: {result:?}");

    // Verify printer was used (output may be partial due to quit)
    printer.flush();
    let _output = out.lock();

    // Verify persistence happened
    let reader = Storage::new(&storage).unwrap();
    let content = reader
        .read_test_events_raw(&conv_id)
        .expect("events should be persisted");

    // Should contain at least the user query
    assert!(
        content.contains("Run a tool"),
        "Should contain user query.\nFile contents:\n{content}"
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn test_multiple_tool_calls_in_sequence() {
    // Tests that multiple tool calls are handled correctly.
    // The executing phase should process all pending calls.

    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let storage = root.join(".jp");

    let config = AppConfig::new_test();
    let mut workspace = Workspace::new(root)
        .persisted_at(&storage)
        .expect("failed to enable persistence");

    let conv_id = workspace.create_conversation(Conversation::default(), config.clone().into());

    let handle = workspace.acquire_conversation(&conv_id).unwrap();
    let lock = workspace.test_lock(handle);

    let chat_request = ChatRequest::from("Do multiple things");

    // Create provider with multiple tool calls in first response
    let provider: Arc<dyn Provider> = Arc::new({
        let tool_call_events = vec![
            Event::Part {
                index: 0,
                event: ConversationEvent::now(ToolCallRequest {
                    id: "call_1".to_string(),
                    name: "tool_a".to_string(),
                    arguments: Map::new(),
                }),
            },
            Event::Part {
                index: 1,
                event: ConversationEvent::now(ToolCallRequest {
                    id: "call_2".to_string(),
                    name: "tool_b".to_string(),
                    arguments: Map::new(),
                }),
            },
            Event::flush(0),
            Event::flush(1),
            Event::Finished(FinishReason::Completed),
        ];

        let message_events = vec![
            Event::Part {
                index: 0,
                event: ConversationEvent::now(ChatResponse::message("Both tasks completed.")),
            },
            Event::flush(0),
            Event::Finished(FinishReason::Completed),
        ];

        SequentialMockProvider {
            responses: vec![tool_call_events, message_events],
            call_index: AtomicUsize::new(0),
            model: ModelDetails::empty(id::ModelIdConfig {
                provider: ProviderId::Test,
                name: "multi-tool-mock".parse().expect("valid name"),
            }),
        }
    });

    let model = provider
        .model_details(&"test-model".parse().unwrap())
        .await
        .unwrap();

    let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let mcp_client = jp_mcp::Client::default();
    let (_signal_tx, signal_rx) = broadcast::channel(16);

    let result = run_turn_loop(
        Arc::clone(&provider),
        &model,
        &config,
        &signal_rx,
        &mcp_client,
        root,
        false,
        &[],
        &lock,
        ToolChoice::Auto,
        &[],
        printer.clone(),
        Arc::new(MockPromptBackend::new()),
        ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
        chat_request.clone(),
    )
    .await;

    assert!(result.is_ok(), "Turn loop should complete: {result:?}");

    // Verify printer output contains the final message
    printer.flush();
    let output = out.lock();
    assert!(
        output.contains("Both tasks completed"),
        "Printer output should contain final LLM response.\nOutput:\n{output}"
    );
    drop(output);

    // Verify persistence
    let reader = Storage::new(&storage).unwrap();
    let content = reader
        .read_test_events_raw(&conv_id)
        .expect("events should be persisted");

    // Should contain both tool calls
    assert!(
        content.contains("tool_a") || content.contains("call_1"),
        "Should contain first tool call.\nFile contents:\n{content}"
    );
    assert!(
        content.contains("tool_b") || content.contains("call_2"),
        "Should contain second tool call.\nFile contents:\n{content}"
    );

    // Should contain final message
    assert!(
        content.contains("Both tasks completed"),
        "Should contain final response.\nFile contents:\n{content}"
    );
}

#[tokio::test]
async fn test_empty_tool_response_continues_cycle() {
    // Tests that when tool execution returns empty (e.g., tool not found),
    // the cycle still continues to the follow-up LLM call.

    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let storage = root.join(".jp");

    let config = AppConfig::new_test();
    let mut workspace = Workspace::new(root)
        .persisted_at(&storage)
        .expect("failed to enable persistence");

    let conv_id = workspace.create_conversation(Conversation::default(), config.clone().into());

    let handle = workspace.acquire_conversation(&conv_id).unwrap();
    let lock = workspace.test_lock(handle);

    let chat_request = ChatRequest::from("Use unknown tool");

    // Provider returns a call to a tool that won't be found
    let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider::with_tool_then_message(
        "call_unknown",
        "nonexistent_tool",
        "I was unable to use that tool.",
    ));
    let model = provider
        .model_details(&"test-model".parse().unwrap())
        .await
        .unwrap();

    let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let mcp_client = jp_mcp::Client::default();
    let (_signal_tx, signal_rx) = broadcast::channel(16);

    let result = run_turn_loop(
        Arc::clone(&provider),
        &model,
        &config,
        &signal_rx,
        &mcp_client,
        root,
        false,
        &[],
        &lock,
        ToolChoice::Auto,
        &[], // No tools configured - tool_coordinator.prepare will fail
        printer.clone(),
        Arc::new(MockPromptBackend::new()),
        ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
        chat_request.clone(),
    )
    .await;

    assert!(result.is_ok(), "Turn loop should complete: {result:?}");

    // Verify printer output contains the follow-up response
    printer.flush();
    let output = out.lock();
    assert!(
        output.contains("unable to use that tool"),
        "Printer output should contain follow-up response.\nOutput:\n{output}"
    );
    drop(output);

    // The second LLM call should have happened
    let reader = Storage::new(&storage).unwrap();
    let content = reader
        .read_test_events_raw(&conv_id)
        .expect("events should be persisted");

    // Should contain the follow-up message from the LLM
    assert!(
        content.contains("unable to use that tool"),
        "Should contain follow-up response.\nFile contents:\n{content}"
    );
}

/// Tests the restart flow:
///
/// 1. LLM returns a tool call
/// 2. During execution, Shutdown signal is received
/// 3. User selects "Restart" from menu (mocked)
/// 4. Tool execution restarts with original calls
/// 5. Eventually completes with follow-up message
#[tokio::test(flavor = "multi_thread")]
async fn test_tool_restart_on_shutdown_signal() {
    // Wrap the entire test in a timeout to prevent infinite hangs
    let test_result = Box::pin(timeout(Duration::from_secs(10), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        // Create config with a slow tool (sleeps for 1 second)
        let mut partial = PartialAppConfig::default();
        partial.assistant.model.id = PartialModelIdConfig {
            provider: Some(ProviderId::Anthropic),
            name: Some(Name("test".to_owned())),
        }
        .into();
        partial.conversation.tools.defaults.run = Some(RunMode::Unattended);
        partial.conversation.tools.tools =
            IndexMap::from_iter([("slow_tool".to_string(), PartialToolConfig {
                source: Some(ToolSource::Local { tool: None }),
                command: Some(PartialCommandConfigOrString::String("sleep 1".to_string())),
                run: Some(RunMode::Unattended),
                ..Default::default()
            })]);
        let config = AppConfig::from_partial(partial, vec![]).expect("valid config");

        let mut workspace = Workspace::new(root)
            .persisted_at(&storage)
            .expect("failed to enable persistence");

        let conv_id =
            workspace.create_conversation(Conversation::default(), Arc::new(config.clone()));

        let handle = workspace.acquire_conversation(&conv_id).unwrap();
        let lock = workspace.test_lock(handle);

        let chat_request = ChatRequest::from("Please use a tool");

        // Provider returns tool call first, then a message.
        let provider = Arc::new(SequentialMockProvider::with_tool_then_message(
            "call_restart",
            "slow_tool",
            "Tool completed after restart.",
        ));
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let (signal_tx, signal_rx) = broadcast::channel(16);

        // Mock user selecting 'r' (Restart) when interrupted.
        // Provide extra 'c' (continue) responses in case of unexpected prompts.
        let backend = MockPromptBackend::new().with_inline_responses(['r', 'c', 'c', 'c', 'c']);

        // Send Shutdown signal after 100ms (tool sleeps for 1s, so plenty of margin).
        let signal_handle = tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            let _ = signal_tx.send(SignalTo::Shutdown);
        });

        let result = run_turn_loop(
            Arc::clone(&provider) as Arc<dyn Provider>,
            &model,
            &config,
            &signal_rx,
            &mcp_client,
            root,
            false,
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer.clone(),
            Arc::new(backend),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            chat_request.clone(),
        )
        .await;

        signal_handle.await.unwrap();

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        // Verify printer output contains the final message
        printer.flush();
        let output = out.lock();
        assert!(
            output.contains("Tool completed after restart"),
            "Printer output should contain final response.\nOutput:\n{output}"
        );
        drop(output);

        // Verify the provider was called at least twice (tool call + final message)
        let call_count = provider.call_index.load(Ordering::SeqCst);
        assert!(
            call_count >= 2,
            "Provider should be called at least twice, got {call_count}"
        );

        // Verify persistence includes the final message
        let reader = Storage::new(&storage).unwrap();
        let content = reader
            .read_test_events_raw(&conv_id)
            .expect("events should be persisted");

        assert!(
            content.contains("Tool completed after restart"),
            "Should contain final response.\nFile contents:\n{content}"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out after 10 seconds");
}

#[tokio::test]
async fn test_merged_stream_exits_after_tool_response() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.conversation.tools.defaults.run = RunMode::Unattended;
        config
            .conversation
            .tools
            .insert("echo_tool".to_string(), ToolConfig {
                source: ToolSource::Local { tool: None },
                command: Some(CommandConfigOrString::String("echo hello".to_string())),
                run: Some(RunMode::Unattended),
                enable: None,
                summary: None,
                description: None,
                examples: None,
                parameters: IndexMap::new(),
                result: None,
                style: None,
                questions: IndexMap::new(),
                options: Map::new(),
            });

        let mut workspace = Workspace::new(root)
            .persisted_at(&storage)
            .expect("failed to enable persistence");

        let conv_id =
            workspace.create_conversation(Conversation::default(), Arc::new(config.clone()));

        let handle = workspace.acquire_conversation(&conv_id).unwrap();
        let lock = workspace.test_lock(handle);

        let chat_request = ChatRequest::from("Please use echo_tool");

        // Provider returns tool call first, then a final message
        let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider::with_tool_then_message(
            "call_echo",
            "echo_tool",
            "Tool executed successfully.",
        ));
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let (_signal_tx, signal_rx) = broadcast::channel(16);

        // No signals sent - the turn loop should complete naturally after
        // the tool executes and the follow-up LLM response is received.
        let result = run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &signal_rx,
            &mcp_client,
            root,
            false,
            &[],
            &lock,
            ToolChoice::Auto,
            &[], // Tool definitions come from config, not this param
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            chat_request.clone(),
        )
        .await;

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        // Verify printer output contains the final message
        printer.flush();
        let output = out.lock();
        assert!(
            output.contains("Tool executed successfully"),
            "Printer output should contain final response.\nOutput:\n{output}"
        );
        drop(output);

        // Verify the conversation persisted with the final message
        let reader = Storage::new(&storage).unwrap();
        let content = reader
            .read_test_events_raw(&conv_id)
            .expect("events should be persisted");

        assert!(
            content.contains("Tool executed successfully"),
            "Should contain final response after tool execution.\nFile contents:\n{content}"
        );
    }))
    .await;

    assert!(
        test_result.is_ok(),
        "Test timed out after 5 seconds - merged stream likely blocked forever after tool response"
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn test_tool_call_with_run_mode_ask_approves() {
    // Tests: LLM returns tool call → Ask prompt → user presses 'y' → tool executes
    // Uses MockExecutor to avoid shell commands.
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        // Configure tool with run = Ask (no command needed - we use MockExecutor)
        let mut config = AppConfig::new_test();
        config.conversation.tools.defaults.run = RunMode::Ask;
        config
            .conversation
            .tools
            .insert("mock_tool".to_string(), ToolConfig {
                source: ToolSource::Local { tool: None },
                command: None, // No real command
                run: Some(RunMode::Ask),
                enable: None,
                summary: None,
                description: None,
                examples: None,
                parameters: IndexMap::new(),
                result: None,
                style: None,
                questions: IndexMap::new(),
                options: Map::new(),
            });

        let mut workspace = Workspace::new(root)
            .persisted_at(&storage)
            .expect("failed to enable persistence");

        let conv_id =
            workspace.create_conversation(Conversation::default(), Arc::new(config.clone()));

        let handle = workspace.acquire_conversation(&conv_id).unwrap();
        let lock = workspace.test_lock(handle);

        let chat_request = ChatRequest::from("Please use mock_tool");

        let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider::with_tool_then_message(
            "call_ask",
            "mock_tool",
            "Tool was approved and executed.",
        ));
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let (_signal_tx, signal_rx) = broadcast::channel(16);

        // Mock: user presses 'y' to approve
        let backend = MockPromptBackend::new().with_inline_responses(['y']);

        // Use TestExecutorSource with MockExecutor that requires permission
        let executor_source = TestExecutorSource::new().with_executor("mock_tool", |req| {
            Box::new(
                MockExecutor::completed(&req.id, &req.name, "mock output").with_permission_info(
                    PermissionInfo {
                        tool_id: req.id.clone(),
                        tool_name: req.name.clone(),
                        tool_source: ToolSource::Local { tool: None },
                        run_mode: RunMode::Ask,
                        arguments: Value::Object(req.arguments.clone()),
                    },
                ),
            )
        });
        let tool_defs = executor_source.tool_definitions();

        let result = run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &signal_rx,
            &mcp_client,
            root,
            true, // is_tty = true to enable prompts
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(backend),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request.clone(),
        )
        .await;

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        // Verify printer output contains the final message
        printer.flush();
        let output = out.lock();
        assert!(
            output.contains("Tool was approved and executed"),
            "Printer output should contain final response.\nOutput:\n{output}"
        );
        drop(output);

        // Verify the tool was executed using typed API
        let events = lock.events().clone();

        // Find tool call responses
        let tool_responses: Vec<_> = events
            .into_iter()
            .filter_map(|e| e.event.into_tool_call_response())
            .collect();

        assert_eq!(
            tool_responses.len(),
            1,
            "Should have exactly one tool response"
        );

        let response = &tool_responses[0];
        assert!(
            response.result.is_ok(),
            "Tool should have succeeded: {:?}",
            response.result
        );

        // Verify the actual content
        assert_eq!(
            response.content(),
            "mock output",
            "Tool output should match mock executor output"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

/// Tests: LLM returns tool call → Ask prompt → user presses 'n' → tool skipped
/// Uses `MockExecutor` to avoid shell commands.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn test_tool_call_with_run_mode_ask_skips() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        // Configure tool with run = Ask
        let mut config = AppConfig::new_test();
        config.conversation.tools.defaults.run = RunMode::Ask;
        config
            .conversation
            .tools
            .insert("mock_tool".to_string(), ToolConfig {
                source: ToolSource::Local { tool: None },
                command: None,
                run: Some(RunMode::Ask),
                enable: None,
                summary: None,
                description: None,
                examples: None,
                parameters: IndexMap::new(),
                result: None,
                style: None,
                questions: IndexMap::new(),
                options: Map::new(),
            });

        let mut workspace = Workspace::new(root)
            .persisted_at(&storage)
            .expect("failed to enable persistence");

        let conv_id =
            workspace.create_conversation(Conversation::default(), Arc::new(config.clone()));

        let handle = workspace.acquire_conversation(&conv_id).unwrap();
        let lock = workspace.test_lock(handle);

        let chat_request = ChatRequest::from("Please use mock_tool");

        let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider::with_tool_then_message(
            "call_skip",
            "mock_tool",
            "Tool was skipped by user.",
        ));
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let (_signal_tx, signal_rx) = broadcast::channel(16);

        // Mock: user presses 'n' to skip
        let backend = MockPromptBackend::new().with_inline_responses(['n']);

        // Use TestExecutorSource with MockExecutor that requires permission
        let executor_source = TestExecutorSource::new().with_executor("mock_tool", |req| {
            Box::new(
                MockExecutor::completed(&req.id, &req.name, "should not see this")
                    .with_permission_info(PermissionInfo {
                        tool_id: req.id.clone(),
                        tool_name: req.name.clone(),
                        tool_source: ToolSource::Local { tool: None },
                        run_mode: RunMode::Ask,
                        arguments: Value::Object(req.arguments.clone()),
                    }),
            )
        });
        let tool_defs = executor_source.tool_definitions();

        let result = run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &signal_rx,
            &mcp_client,
            root,
            true,
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(backend),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request.clone(),
        )
        .await;

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        // Verify printer output contains the final message
        printer.flush();
        let output = out.lock();
        assert!(
            output.contains("Tool was skipped by user"),
            "Printer output should contain final response.\nOutput:\n{output}"
        );
        drop(output);

        // Verify the tool was skipped using typed API
        let events = lock.events().clone();

        // Find tool call responses
        let tool_responses: Vec<_> = events
            .into_iter()
            .filter_map(|e| e.event.into_tool_call_response())
            .collect();

        assert_eq!(
            tool_responses.len(),
            1,
            "Should have exactly one tool response"
        );

        let response = &tool_responses[0];
        // Tool was skipped, so result is Ok with skip message
        assert!(
            response.result.is_ok(),
            "Skipped tool should have Ok result: {:?}",
            response.result
        );

        // Verify the skip message is in the content
        assert!(
            response.content().contains("skipped"),
            "Should contain 'skipped' in response: {}",
            response.content()
        );

        // Should NOT contain the mock output (tool didn't run)
        assert!(
            !response.content().contains("should not see this"),
            "Should NOT contain mock output since tool was skipped: {}",
            response.content()
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn test_tool_call_with_run_mode_unattended() {
    // Tests: LLM returns tool call → Unattended mode → tool runs without prompt
    // Uses MockExecutor to avoid shell commands.
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        // Configure tool with run = Unattended (no prompt needed)
        let mut config = AppConfig::new_test();
        config.conversation.tools.defaults.run = RunMode::Unattended;
        config
            .conversation
            .tools
            .insert("mock_tool".to_string(), ToolConfig {
                source: ToolSource::Local { tool: None },
                command: None,
                run: Some(RunMode::Unattended),
                enable: None,
                summary: None,
                description: None,
                examples: None,
                parameters: IndexMap::new(),
                result: None,
                style: None,
                questions: IndexMap::new(),
                options: Map::new(),
            });

        let mut workspace = Workspace::new(root)
            .persisted_at(&storage)
            .expect("failed to enable persistence");

        let conv_id =
            workspace.create_conversation(Conversation::default(), Arc::new(config.clone()));

        let handle = workspace.acquire_conversation(&conv_id).unwrap();
        let lock = workspace.test_lock(handle);

        let chat_request = ChatRequest::from("Please use mock_tool");

        let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider::with_tool_then_message(
            "call_unattended",
            "mock_tool",
            "Tool ran in unattended mode.",
        ));
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let (_signal_tx, signal_rx) = broadcast::channel(16);

        // No prompt responses needed - tool runs without asking
        let backend = MockPromptBackend::new();

        // MockExecutor without permission_info (Unattended mode)
        let executor_source = TestExecutorSource::new().with_executor("mock_tool", |req| {
            // No permission_info = no prompt required
            Box::new(MockExecutor::completed(
                &req.id,
                &req.name,
                "unattended execution output",
            ))
        });
        let tool_defs = executor_source.tool_definitions();

        let result = run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &signal_rx,
            &mcp_client,
            root,
            true, // is_tty doesn't matter for Unattended
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(backend),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request.clone(),
        )
        .await;

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        // Verify printer output contains the final message
        printer.flush();
        let output = out.lock();
        assert!(
            output.contains("Tool ran in unattended mode"),
            "Printer output should contain final response.\nOutput:\n{output}"
        );
        drop(output);

        // Verify the tool was executed using typed API (not raw JSON)
        let events = lock.events().clone();

        // Find tool call responses
        let tool_responses: Vec<_> = events
            .into_iter()
            .filter_map(|e| e.event.into_tool_call_response())
            .collect();

        assert_eq!(
            tool_responses.len(),
            1,
            "Should have exactly one tool response"
        );

        let response = &tool_responses[0];
        assert!(
            response.result.is_ok(),
            "Tool should have succeeded: {:?}",
            response.result
        );

        // Verify the actual content (decoded from base64 by the typed API)
        assert_eq!(
            response.content(),
            "unattended execution output",
            "Tool output should match mock executor output"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn test_tool_call_with_run_mode_skip() {
    // Tests: LLM returns tool call → Skip mode → tool is skipped without prompt
    // Uses MockExecutor to avoid shell commands.
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        // Configure tool with run = Skip (always skipped)
        let mut config = AppConfig::new_test();
        config.conversation.tools.defaults.run = RunMode::Skip;
        config
            .conversation
            .tools
            .insert("mock_tool".to_string(), ToolConfig {
                source: ToolSource::Local { tool: None },
                command: None,
                run: Some(RunMode::Skip),
                enable: None,
                summary: None,
                description: None,
                examples: None,
                parameters: IndexMap::new(),
                result: None,
                style: None,
                questions: IndexMap::new(),
                options: Map::new(),
            });

        let mut workspace = Workspace::new(root)
            .persisted_at(&storage)
            .expect("failed to enable persistence");

        let conv_id =
            workspace.create_conversation(Conversation::default(), Arc::new(config.clone()));

        let handle = workspace.acquire_conversation(&conv_id).unwrap();
        let lock = workspace.test_lock(handle);

        let chat_request = ChatRequest::from("Please use mock_tool");

        let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider::with_tool_then_message(
            "call_skip",
            "mock_tool",
            "Tool was skipped by configuration.",
        ));
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let (_signal_tx, signal_rx) = broadcast::channel(16);

        // No prompt responses needed - tool is skipped automatically
        let backend = MockPromptBackend::new();

        // MockExecutor with Skip mode permission_info - executor returns completion
        // but the prompter should return Skip before execution happens
        let executor_source = TestExecutorSource::new().with_executor("mock_tool", |req| {
            Box::new(
                MockExecutor::completed(
                    &req.id,
                    &req.name,
                    "SHOULD NOT SEE THIS - tool should be skipped",
                )
                .with_permission_info(PermissionInfo {
                    tool_id: req.id.clone(),
                    tool_name: req.name.clone(),
                    tool_source: ToolSource::Local { tool: None },
                    run_mode: RunMode::Skip,
                    arguments: Value::Object(req.arguments.clone()),
                }),
            )
        });
        let tool_defs = executor_source.tool_definitions();

        let result = run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &signal_rx,
            &mcp_client,
            root,
            true,
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(backend),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request.clone(),
        )
        .await;

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        // Verify printer output contains the final message
        printer.flush();
        let output = out.lock();
        assert!(
            output.contains("Tool was skipped by configuration"),
            "Printer output should contain final response.\nOutput:\n{output}"
        );
        drop(output);

        // Verify the tool was skipped using typed API
        let events = lock.events().clone();

        // Find tool call responses
        let tool_responses: Vec<_> = events
            .into_iter()
            .filter_map(|e| e.event.into_tool_call_response())
            .collect();

        assert_eq!(
            tool_responses.len(),
            1,
            "Should have exactly one tool response"
        );

        let response = &tool_responses[0];
        // Tool was skipped, so result is Ok with skip message
        assert!(
            response.result.is_ok(),
            "Skipped tool should have Ok result: {:?}",
            response.result
        );

        // Verify the skip message is in the content
        assert!(
            response.content().contains("skipped"),
            "Should contain 'skipped' in response: {}",
            response.content()
        );

        // Should NOT contain the mock output (tool was skipped)
        assert!(
            !response.content().contains("SHOULD NOT SEE THIS"),
            "Should NOT contain mock output since tool was skipped: {}",
            response.content()
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn test_multiple_tools_with_different_run_modes() {
    // Tests: LLM returns 2 tool calls → one Ask (approved), one Unattended
    // Both should complete successfully with proper handling.
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        // tool_ask requires approval
        config
            .conversation
            .tools
            .insert("tool_ask".to_string(), ToolConfig {
                source: ToolSource::Local { tool: None },
                command: None,
                run: Some(RunMode::Ask),
                enable: None,
                summary: None,
                description: None,
                examples: None,
                parameters: IndexMap::new(),
                result: None,
                style: None,
                questions: IndexMap::new(),
                options: Map::new(),
            });
        // tool_unattended runs automatically
        config
            .conversation
            .tools
            .insert("tool_unattended".to_string(), ToolConfig {
                source: ToolSource::Local { tool: None },
                command: None,
                run: Some(RunMode::Unattended),
                enable: None,
                summary: None,
                description: None,
                examples: None,
                parameters: IndexMap::new(),
                result: None,
                style: None,
                questions: IndexMap::new(),
                options: Map::new(),
            });

        let mut workspace = Workspace::new(root)
            .persisted_at(&storage)
            .expect("failed to enable persistence");

        let conv_id =
            workspace.create_conversation(Conversation::default(), Arc::new(config.clone()));

        let handle = workspace.acquire_conversation(&conv_id).unwrap();
        let lock = workspace.test_lock(handle);

        let chat_request = ChatRequest::from("Use both tools");

        // Provider returns two tool calls, then a message
        let provider: Arc<dyn Provider> = Arc::new({
            let tool_call_events = vec![
                Event::Part {
                    index: 0,
                    event: ConversationEvent::now(ToolCallRequest {
                        id: "call_ask".to_string(),
                        name: "tool_ask".to_string(),
                        arguments: Map::new(),
                    }),
                },
                Event::Part {
                    index: 1,
                    event: ConversationEvent::now(ToolCallRequest {
                        id: "call_unattended".to_string(),
                        name: "tool_unattended".to_string(),
                        arguments: Map::new(),
                    }),
                },
                Event::flush(0),
                Event::flush(1),
                Event::Finished(FinishReason::Completed),
            ];

            let message_events = vec![
                Event::Part {
                    index: 0,
                    event: ConversationEvent::now(ChatResponse::message("Both tools completed.")),
                },
                Event::flush(0),
                Event::Finished(FinishReason::Completed),
            ];

            SequentialMockProvider {
                responses: vec![tool_call_events, message_events],
                call_index: AtomicUsize::new(0),
                model: ModelDetails::empty(id::ModelIdConfig {
                    provider: ProviderId::Test,
                    name: "multi-mode-mock".parse().expect("valid name"),
                }),
            }
        });

        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let (_signal_tx, signal_rx) = broadcast::channel(16);

        // User presses 'y' to approve the Ask tool
        let backend = MockPromptBackend::new().with_inline_responses(['y']);

        let executor_source = TestExecutorSource::new()
            .with_executor("tool_ask", |req| {
                Box::new(
                    MockExecutor::completed(&req.id, &req.name, "ask tool output")
                        .with_permission_info(PermissionInfo {
                            tool_id: req.id.clone(),
                            tool_name: req.name.clone(),
                            tool_source: ToolSource::Local { tool: None },
                            run_mode: RunMode::Ask,
                            arguments: Value::Object(req.arguments.clone()),
                        }),
                )
            })
            .with_executor("tool_unattended", |req| {
                // No permission_info = runs without prompt
                Box::new(MockExecutor::completed(
                    &req.id,
                    &req.name,
                    "unattended tool output",
                ))
            });
        let tool_defs = executor_source.tool_definitions();

        let result = run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &signal_rx,
            &mcp_client,
            root,
            true,
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(backend),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request.clone(),
        )
        .await;

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        // Verify printer output contains the final message
        printer.flush();
        let output = out.lock();
        assert!(
            output.contains("Both tools completed"),
            "Printer output should contain final response.\nOutput:\n{output}"
        );
        drop(output);

        // Verify both tools were executed using typed API
        let events = lock.events().clone();

        // Find tool call responses
        let tool_responses: Vec<_> = events
            .into_iter()
            .filter_map(|e| e.event.into_tool_call_response())
            .collect();

        assert_eq!(
            tool_responses.len(),
            2,
            "Should have exactly two tool responses"
        );

        // Both tools should have succeeded
        for response in &tool_responses {
            assert!(
                response.result.is_ok(),
                "Tool {} should have succeeded: {:?}",
                response.id,
                response.result
            );
        }

        // Find each tool's response by checking content
        let ask_response = tool_responses
            .iter()
            .find(|r| r.content() == "ask tool output");
        let unattended_response = tool_responses
            .iter()
            .find(|r| r.content() == "unattended tool output");

        assert!(
            ask_response.is_some(),
            "Should have response from tool_ask with 'ask tool output'"
        );
        assert!(
            unattended_response.is_some(),
            "Should have response from tool_unattended with 'unattended tool output'"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn test_tool_call_returns_error() {
    // Tests: LLM returns tool call → tool returns error → error is persisted
    // Uses MockExecutor to simulate error without shell commands.
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.conversation.tools.defaults.run = RunMode::Unattended;
        config
            .conversation
            .tools
            .insert("failing_tool".to_string(), ToolConfig {
                source: ToolSource::Local { tool: None },
                command: None,
                run: Some(RunMode::Unattended),
                enable: None,
                summary: None,
                description: None,
                examples: None,
                parameters: IndexMap::new(),
                result: None,
                style: None,
                questions: IndexMap::new(),
                options: Map::new(),
            });

        let mut workspace = Workspace::new(root)
            .persisted_at(&storage)
            .expect("failed to enable persistence");

        let conv_id =
            workspace.create_conversation(Conversation::default(), Arc::new(config.clone()));

        let handle = workspace.acquire_conversation(&conv_id).unwrap();
        let lock = workspace.test_lock(handle);

        let chat_request = ChatRequest::from("Use failing_tool");

        let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider::with_tool_then_message(
            "call_fail",
            "failing_tool",
            "Tool failed, here is the error.",
        ));
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let (_signal_tx, signal_rx) = broadcast::channel(16);

        let backend = MockPromptBackend::new();

        // MockExecutor that returns an error
        let executor_source = TestExecutorSource::new().with_executor("failing_tool", |req| {
            Box::new(MockExecutor::error(
                &req.id,
                &req.name,
                "Simulated tool failure",
            ))
        });
        let tool_defs = executor_source.tool_definitions();

        let result = run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &signal_rx,
            &mcp_client,
            root,
            true,
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(backend),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request.clone(),
        )
        .await;

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        // Verify printer output contains the final message
        printer.flush();
        let output = out.lock();
        assert!(
            output.contains("Tool failed"),
            "Printer output should contain final response.\nOutput:\n{output}"
        );
        drop(output);

        // Verify the tool error using typed API
        let events = lock.events().clone();

        // Find tool call responses
        let tool_responses: Vec<_> = events
            .into_iter()
            .filter_map(|e| e.event.into_tool_call_response())
            .collect();

        assert_eq!(
            tool_responses.len(),
            1,
            "Should have exactly one tool response"
        );

        let response = &tool_responses[0];

        // Tool should have failed (result is Err)
        assert!(
            response.result.is_err(),
            "Tool should have failed: {:?}",
            response.result
        );

        // Verify the error message
        assert_eq!(
            response.content(),
            "Simulated tool failure",
            "Error message should match mock executor error"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

/// A mock provider that delays before returning the stream.
///
/// This simulates a slow API response, allowing us to test the
/// waiting indicator during the HTTP round-trip.
#[derive(Debug)]
struct DelayedMockProvider {
    delay: Duration,
    response: String,
    model: ModelDetails,
}

impl DelayedMockProvider {
    fn new(delay: Duration, response: &str) -> Self {
        Self {
            delay,
            response: response.to_string(),
            model: ModelDetails::empty(id::ModelIdConfig {
                provider: ProviderId::Test,
                name: "delayed-mock".parse().expect("valid name"),
            }),
        }
    }
}

#[async_trait]
impl Provider for DelayedMockProvider {
    async fn model_details(&self, name: &id::Name) -> Result<ModelDetails, LlmError> {
        let mut model = self.model.clone();
        model.id.name = name.clone();
        Ok(model)
    }

    async fn models(&self) -> Result<Vec<ModelDetails>, LlmError> {
        Ok(vec![self.model.clone()])
    }

    async fn chat_completion_stream(
        &self,
        _model: &ModelDetails,
        _query: ChatQuery,
    ) -> Result<EventStream, LlmError> {
        tokio::time::sleep(self.delay).await;

        let events = vec![
            Event::Part {
                index: 0,
                event: ConversationEvent::now(ChatResponse::message(&self.response)),
            },
            Event::flush(0),
            Event::Finished(FinishReason::Completed),
        ];

        Ok(Box::pin(stream::iter(events.into_iter().map(Ok))))
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_waiting_indicator_shows_during_delay() {
    // Tests that the waiting indicator appears when the LLM takes longer
    // than the configured delay. Uses a multi_thread runtime so the
    // spawned timer task can run concurrently with run_cycle().await.

    let test_result = Box::pin(timeout(Duration::from_secs(10), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        // Set delay to 0 so the indicator appears immediately
        config.style.streaming.progress.show = true;
        config.style.streaming.progress.delay_secs = 0;
        config.style.streaming.progress.interval_ms = 100;

        let mut workspace = Workspace::new(root)
            .persisted_at(&storage)
            .expect("failed to enable persistence");

        let conv_id =
            workspace.create_conversation(Conversation::default(), Arc::new(config.clone()));

        let handle = workspace.acquire_conversation(&conv_id).unwrap();
        let lock = workspace.test_lock(handle);

        let chat_request = ChatRequest::from("Hello");

        // Provider delays 500ms before returning stream
        let provider: Arc<dyn Provider> = Arc::new(DelayedMockProvider::new(
            Duration::from_millis(500),
            "Response after delay",
        ));
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let (_signal_tx, signal_rx) = broadcast::channel(16);

        run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &signal_rx,
            &mcp_client,
            root,
            true, // is_tty = true to enable the indicator
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            chat_request.clone(),
        )
        .await
        .unwrap();

        printer.flush();
        let output = out.lock();

        // The output should contain the waiting indicator text
        // (may be overwritten by \r, but the raw buffer captures all writes)
        assert!(
            output.contains("Waiting…"),
            "Output should contain waiting indicator.\nOutput:\n{output}"
        );

        // And the final response should also be present
        assert!(
            output.contains("Response after delay"),
            "Output should contain LLM response.\nOutput:\n{output}"
        );

        // The clear sequence should also be present (indicator was cleared)
        assert!(
            output.contains("\r\x1b[K"),
            "Output should contain clear sequence.\nOutput:\n{output}"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_waiting_indicator_not_shown_when_disabled() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.style.streaming.progress.show = false;

        let mut workspace = Workspace::new(root)
            .persisted_at(&storage)
            .expect("failed to enable persistence");

        let conv_id =
            workspace.create_conversation(Conversation::default(), Arc::new(config.clone()));

        let handle = workspace.acquire_conversation(&conv_id).unwrap();
        let lock = workspace.test_lock(handle);

        let chat_request = ChatRequest::from("Hello");

        let provider: Arc<dyn Provider> = Arc::new(DelayedMockProvider::new(
            Duration::from_millis(200),
            "Quick response",
        ));
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let (_signal_tx, signal_rx) = broadcast::channel(16);

        run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &signal_rx,
            &mcp_client,
            root,
            true, // is_tty
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            chat_request.clone(),
        )
        .await
        .unwrap();

        printer.flush();
        let output = out.lock();

        // Should NOT contain waiting indicator
        assert!(
            !output.contains("Waiting…"),
            "Output should NOT contain waiting indicator when disabled.\nOutput:\n{output}"
        );

        // But should contain the response
        assert!(
            output.contains("Quick response"),
            "Output should contain LLM response.\nOutput:\n{output}"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_waiting_indicator_not_shown_for_non_tty() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.style.streaming.progress.show = true;
        config.style.streaming.progress.delay_secs = 0;

        let mut workspace = Workspace::new(root)
            .persisted_at(&storage)
            .expect("failed to enable persistence");

        let conv_id =
            workspace.create_conversation(Conversation::default(), Arc::new(config.clone()));

        let handle = workspace.acquire_conversation(&conv_id).unwrap();
        let lock = workspace.test_lock(handle);

        let chat_request = ChatRequest::from("Hello");

        let provider: Arc<dyn Provider> = Arc::new(DelayedMockProvider::new(
            Duration::from_millis(200),
            "Non-tty response",
        ));
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let (_signal_tx, signal_rx) = broadcast::channel(16);

        run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &signal_rx,
            &mcp_client,
            root,
            false, // is_tty = false
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            chat_request.clone(),
        )
        .await
        .unwrap();

        printer.flush();
        let output = out.lock();

        // Should NOT contain waiting indicator for non-TTY
        assert!(
            !output.contains("Waiting…"),
            "Output should NOT contain waiting indicator for non-TTY.\nOutput:\n{output}"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

#[tokio::test(flavor = "multi_thread")]
#[expect(clippy::too_many_lines, clippy::items_after_statements)]
async fn test_multi_part_tool_call_shows_preparing_spinner() {
    // Tests the multi-part streaming tool call flow:
    // 1. LLM emits initial Part with tool name+id (empty args)
    //    → "Calling tool X (receiving arguments…)" spinner appears
    // 2. After a small delay, LLM emits final Part with parsed arguments
    // 3. Flush completes the tool call, spinner is cleared
    // 4. Tool executes (not found), follow-up LLM returns message
    // 5. Verify the spinner text appeared in the output
    let test_result = Box::pin(timeout(Duration::from_secs(10), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        // Enable the preparing indicator with no delay so it shows
        // immediately.
        config.style.tool_call.show = true;
        config.style.tool_call.preparing.show = true;
        config.style.tool_call.preparing.delay_secs = 0;
        config.style.tool_call.preparing.interval_ms = 50;

        let mut workspace = Workspace::new(root)
            .persisted_at(&storage)
            .expect("failed to enable persistence");

        let conv_id =
            workspace.create_conversation(Conversation::default(), Arc::new(config.clone()));

        let handle = workspace.acquire_conversation(&conv_id).unwrap();
        let lock = workspace.test_lock(handle);

        let chat_request = ChatRequest::from("Create a file");

        // Build a provider that simulates multi-part tool call streaming
        // with a delay between the initial Part and the final Part, giving
        // the spawned indicator task time to tick.
        let mut args = Map::new();
        args.insert("path".into(), "test.rs".into());
        args.insert("content".into(), "fn main() {}".into());

        let tool_call_events: Vec<Result<Event, jp_llm::error::StreamError>> = vec![
            // Initial Part: name+id known, arguments still streaming
            Ok(Event::Part {
                index: 0,
                event: ConversationEvent::now(ToolCallRequest {
                    id: "call_multi".to_string(),
                    name: "fs_create_file".to_string(),
                    arguments: Map::new(),
                }),
            }),
        ];

        let args_clone = args.clone();
        let delayed_events: Vec<Result<Event, jp_llm::error::StreamError>> = vec![
            Ok(Event::Part {
                index: 0,
                event: ConversationEvent::now(ToolCallRequest {
                    id: "call_multi".to_string(),
                    name: "fs_create_file".to_string(),
                    arguments: args_clone,
                }),
            }),
            Ok(Event::flush(0)),
            Ok(Event::Finished(FinishReason::Completed)),
        ];

        // Stream the initial Part immediately, then after 200ms stream the
        // rest. This gives the indicator task time to tick.
        let first_stream = futures::stream::iter(tool_call_events);
        let delay_stream = futures::stream::once(async {
            tokio::time::sleep(Duration::from_millis(200)).await;
            // This value is discarded; we just need the delay
            Ok(Event::Finished(FinishReason::Completed))
        })
        .filter(|_| futures::future::ready(false)); // discard the dummy

        // Use the delayed_events after the delay
        let rest_stream = futures::stream::iter(delayed_events);

        // Chain: initial Part → delay → final Part + Flush + Finished
        let combined_first_cycle: jp_llm::EventStream =
            Box::pin(first_stream.chain(delay_stream).chain(rest_stream));

        let message_events: Vec<Result<Event, jp_llm::error::StreamError>> = vec![
            Ok(Event::Part {
                index: 0,
                event: ConversationEvent::now(ChatResponse::message("File created.")),
            }),
            Ok(Event::flush(0)),
            Ok(Event::Finished(FinishReason::Completed)),
        ];

        // Custom provider: first call returns the delayed stream, second
        // returns the message.
        struct DelayedToolCallProvider {
            first_cycle: std::sync::Mutex<Option<jp_llm::EventStream>>,
            second_cycle: std::sync::Mutex<Option<Vec<Result<Event, jp_llm::error::StreamError>>>>,
            model: ModelDetails,
        }

        impl fmt::Debug for DelayedToolCallProvider {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct("DelayedToolCallProvider").finish()
            }
        }

        #[async_trait]
        impl Provider for DelayedToolCallProvider {
            async fn model_details(&self, name: &id::Name) -> Result<ModelDetails, LlmError> {
                let mut m = self.model.clone();
                m.id.name = name.clone();
                Ok(m)
            }

            async fn models(&self) -> Result<Vec<ModelDetails>, LlmError> {
                Ok(vec![self.model.clone()])
            }

            async fn chat_completion_stream(
                &self,
                _model: &ModelDetails,
                _query: ChatQuery,
            ) -> Result<jp_llm::EventStream, LlmError> {
                if let Some(stream) = self.first_cycle.lock().unwrap().take() {
                    return Ok(stream);
                }
                if let Some(events) = self.second_cycle.lock().unwrap().take() {
                    return Ok(Box::pin(futures::stream::iter(events)));
                }
                Ok(Box::pin(futures::stream::iter(vec![Ok(Event::Finished(
                    FinishReason::Completed,
                ))])))
            }
        }

        let provider: Arc<dyn Provider> = Arc::new(DelayedToolCallProvider {
            first_cycle: std::sync::Mutex::new(Some(combined_first_cycle)),
            second_cycle: std::sync::Mutex::new(Some(message_events)),
            model: ModelDetails::empty(id::ModelIdConfig {
                provider: ProviderId::Test,
                name: "delayed-tool-mock".parse().expect("valid name"),
            }),
        });

        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let (_signal_tx, signal_rx) = broadcast::channel(16);

        let result = run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &signal_rx,
            &mcp_client,
            root,
            true, // is_tty = true to enable the indicator
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            chat_request.clone(),
        )
        .await;

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        printer.flush();
        let output = out.lock();

        // The spinner should show "Calling tool" with "receiving arguments"
        assert!(
            output.contains("Calling tool"),
            "Output should contain 'Calling tool'.\nOutput:\n{output}"
        );
        assert!(
            output.contains("fs_create_file"),
            "Output should contain the tool name.\nOutput:\n{output}"
        );
        assert!(
            output.contains("receiving arguments"),
            "Output should contain 'receiving arguments'.\nOutput:\n{output}"
        );

        // The clear-to-end-of-line escape should be present
        // (preparing suffix was cleared before printing arguments).
        assert!(
            output.contains("\x1b[K"),
            "Output should contain the clear-to-EOL escape.\nOutput:\n{output}"
        );

        // The final message should also be present
        assert!(
            output.contains("File created"),
            "Output should contain final LLM response.\nOutput:\n{output}"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

#[tokio::test]
async fn test_turn_start_event_is_emitted() {
    // A single run_turn_loop call should inject a TurnStart { index: 0 }
    // event at the beginning of the conversation stream.

    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let storage = root.join(".jp");

    let config = AppConfig::new_test();
    let mut workspace = Workspace::new(root)
        .persisted_at(&storage)
        .expect("failed to enable persistence");

    let conv_id = workspace.create_conversation(Conversation::default(), config.clone().into());

    let handle = workspace.acquire_conversation(&conv_id).unwrap();
    let lock = workspace.test_lock(handle);

    let chat_request = ChatRequest::from("Hello");

    let provider: Arc<dyn Provider> = Arc::new(MockProvider::with_message("Hi there"));
    let model = provider
        .model_details(&"test-model".parse().unwrap())
        .await
        .unwrap();

    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let mcp_client = jp_mcp::Client::default();
    let (_signal_tx, signal_rx) = broadcast::channel(16);

    run_turn_loop(
        Arc::clone(&provider),
        &model,
        &config,
        &signal_rx,
        &mcp_client,
        root,
        false,
        &[],
        &lock,
        ToolChoice::Auto,
        &[],
        printer,
        Arc::new(MockPromptBackend::new()),
        ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
        chat_request.clone(),
    )
    .await
    .unwrap();

    let events = lock.events();
    let turn_starts: Vec<&TurnStart> = events
        .iter()
        .filter_map(|e| e.event.as_turn_start())
        .collect();

    assert_eq!(turn_starts.len(), 1, "Expected exactly one TurnStart event");
}

#[tokio::test]
async fn test_turn_start_index_increments_across_turns() {
    // Two consecutive run_turn_loop calls should produce TurnStart events
    // with indices 0 and 1.

    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let storage = root.join(".jp");

    let config = AppConfig::new_test();
    let mut workspace = Workspace::new(root)
        .persisted_at(&storage)
        .expect("failed to enable persistence");

    let conv_id = workspace.create_conversation(Conversation::default(), config.clone().into());

    let handle = workspace.acquire_conversation(&conv_id).unwrap();
    let lock = workspace.test_lock(handle);

    let mcp_client = jp_mcp::Client::default();

    // First turn.
    let chat_request = ChatRequest::from("First question");

    let provider: Arc<dyn Provider> = Arc::new(MockProvider::with_message("First answer"));
    let model = provider
        .model_details(&"test-model".parse().unwrap())
        .await
        .unwrap();

    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let (_signal_tx, signal_rx) = broadcast::channel(16);

    run_turn_loop(
        Arc::clone(&provider),
        &model,
        &config,
        &signal_rx,
        &mcp_client,
        root,
        false,
        &[],
        &lock,
        ToolChoice::Auto,
        &[],
        printer,
        Arc::new(MockPromptBackend::new()),
        ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
        chat_request.clone(),
    )
    .await
    .unwrap();

    // Second turn.
    let chat_request = ChatRequest::from("Second question");

    let provider: Arc<dyn Provider> = Arc::new(MockProvider::with_message("Second answer"));
    let model = provider
        .model_details(&"test-model".parse().unwrap())
        .await
        .unwrap();

    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let (_signal_tx, signal_rx) = broadcast::channel(16);

    run_turn_loop(
        Arc::clone(&provider),
        &model,
        &config,
        &signal_rx,
        &mcp_client,
        root,
        false,
        &[],
        &lock,
        ToolChoice::Auto,
        &[],
        printer,
        Arc::new(MockPromptBackend::new()),
        ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
        chat_request.clone(),
    )
    .await
    .unwrap();

    // Verify.
    let events = lock.events();
    let turn_starts: Vec<&TurnStart> = events
        .iter()
        .filter_map(|e| e.event.as_turn_start())
        .collect();

    assert_eq!(turn_starts.len(), 2, "Expected two TurnStart events");
}

/// Verifies that buffered markdown text is flushed before the "Calling
/// tool" header appears in the output (Issue 1 fix).
#[tokio::test]
async fn test_markdown_flushed_before_tool_header() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.style.tool_call.show = true;

        let mut workspace = Workspace::new(root)
            .persisted_at(&storage)
            .expect("failed to enable persistence");

        let conv_id =
            workspace.create_conversation(Conversation::default(), Arc::new(config.clone()));

        let handle = workspace.acquire_conversation(&conv_id).unwrap();
        let lock = workspace.test_lock(handle);

        let chat_request = ChatRequest::from("Do something");

        // LLM emits a message part followed immediately by a tool call
        // in the same response. The message must appear before the header.
        let provider: Arc<dyn Provider> = Arc::new({
            let events = vec![
                Event::Part {
                    index: 0,
                    event: ConversationEvent::now(ChatResponse::message("Let me check that.\n\n")),
                },
                Event::flush(0),
                Event::Part {
                    index: 1,
                    event: ConversationEvent::now(ToolCallRequest {
                        id: "call_1".to_string(),
                        name: "fs_read_file".to_string(),
                        arguments: Map::new(),
                    }),
                },
                Event::flush(1),
                Event::Finished(FinishReason::Completed),
            ];

            let followup = vec![
                Event::Part {
                    index: 0,
                    event: ConversationEvent::now(ChatResponse::message("Done.\n\n")),
                },
                Event::flush(0),
                Event::Finished(FinishReason::Completed),
            ];

            SequentialMockProvider {
                responses: vec![events, followup],
                call_index: AtomicUsize::new(0),
                model: ModelDetails::empty(id::ModelIdConfig {
                    provider: ProviderId::Test,
                    name: "md-flush-mock".parse().expect("valid name"),
                }),
            }
        });

        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let (_signal_tx, signal_rx) = broadcast::channel(16);

        run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &signal_rx,
            &mcp_client,
            root,
            false,
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            chat_request.clone(),
        )
        .await
        .unwrap();

        printer.flush();
        let output = out.lock().clone();

        // The markdown text must precede the tool header.
        let md_pos = output
            .find("Let me check that")
            .expect("markdown text should be in output");
        let tool_pos = output
            .find("Calling tool")
            .expect("tool header should be in output");

        assert!(
            md_pos < tool_pos,
            "Markdown text (pos {md_pos}) should appear before 'Calling tool' header (pos \
             {tool_pos}).\nOutput:\n{output}"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

/// Verifies that multiple parallel tool calls produce one permanent
/// "Calling tool X(args)" line each, not garbled across lines.
///
/// Uses `FunctionCall` parameter style so header+args appear on one
/// line, making assertions straightforward.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn test_parallel_tool_calls_rendered_atomically() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.style.tool_call.show = true;

        let fn_call_style = Some(DisplayStyleConfig {
            hidden: false,
            inline_results: InlineResults::Off,
            results_file_link: LinkStyle::Off,
            parameters: ParametersStyle::FunctionCall,
        });

        // Configure tools with FunctionCall style for readable output.
        config.conversation.tools.defaults.run = RunMode::Unattended;
        config
            .conversation
            .tools
            .insert("tool_a".to_string(), ToolConfig {
                source: ToolSource::Local { tool: None },
                command: None,
                run: Some(RunMode::Unattended),
                enable: None,
                summary: None,
                description: None,
                examples: None,
                parameters: IndexMap::new(),
                result: None,
                style: fn_call_style.clone(),
                questions: IndexMap::new(),
                options: Map::new(),
            });
        config
            .conversation
            .tools
            .insert("tool_b".to_string(), ToolConfig {
                source: ToolSource::Local { tool: None },
                command: None,
                run: Some(RunMode::Unattended),
                enable: None,
                summary: None,
                description: None,
                examples: None,
                parameters: IndexMap::new(),
                result: None,
                style: fn_call_style,
                questions: IndexMap::new(),
                options: Map::new(),
            });

        let mut workspace = Workspace::new(root)
            .persisted_at(&storage)
            .expect("failed to enable persistence");

        let conv_id =
            workspace.create_conversation(Conversation::default(), Arc::new(config.clone()));

        let handle = workspace.acquire_conversation(&conv_id).unwrap();
        let lock = workspace.test_lock(handle);

        let chat_request = ChatRequest::from("Use both tools");

        // Two tool calls with actual arguments.
        let mut args_a = Map::new();
        args_a.insert("package".into(), "jp_cli".into());
        let mut args_b = Map::new();
        args_b.insert("path".into(), "/tmp/test.rs".into());

        let provider: Arc<dyn Provider> = Arc::new({
            let tool_events = vec![
                Event::Part {
                    index: 0,
                    event: ConversationEvent::now(ToolCallRequest {
                        id: "call_a".to_string(),
                        name: "tool_a".to_string(),
                        arguments: args_a,
                    }),
                },
                Event::Part {
                    index: 1,
                    event: ConversationEvent::now(ToolCallRequest {
                        id: "call_b".to_string(),
                        name: "tool_b".to_string(),
                        arguments: args_b,
                    }),
                },
                Event::flush(0),
                Event::flush(1),
                Event::Finished(FinishReason::Completed),
            ];

            let followup = vec![
                Event::Part {
                    index: 0,
                    event: ConversationEvent::now(ChatResponse::message("Both done.\n\n")),
                },
                Event::flush(0),
                Event::Finished(FinishReason::Completed),
            ];

            SequentialMockProvider {
                responses: vec![tool_events, followup],
                call_index: AtomicUsize::new(0),
                model: ModelDetails::empty(id::ModelIdConfig {
                    provider: ProviderId::Test,
                    name: "parallel-tools-mock".parse().expect("valid name"),
                }),
            }
        });

        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let (_signal_tx, signal_rx) = broadcast::channel(16);

        let executor_source = TestExecutorSource::new()
            .with_executor("tool_a", |req| {
                Box::new(
                    MockExecutor::completed(&req.id, &req.name, "result_a")
                        .with_arguments(req.arguments.clone()),
                )
            })
            .with_executor("tool_b", |req| {
                Box::new(
                    MockExecutor::completed(&req.id, &req.name, "result_b")
                        .with_arguments(req.arguments.clone()),
                )
            });
        let tool_defs = executor_source.tool_definitions();

        run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &signal_rx,
            &mcp_client,
            root,
            false, // is_tty = false (no timer, keeps output deterministic)
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request.clone(),
        )
        .await
        .unwrap();

        printer.flush();
        let raw = out.lock().clone();

        // The raw buffer contains \r and \x1b[K from temp line
        // rewrites. To check the "final visible" output, find the
        // permanent lines which are the ones written by complete()
        // and contain both the tool name AND its args on the same
        // write (they use FunctionCall style: `(key: "val")`).
        //
        // The permanent line pattern is:
        //   Calling tool <name>(<args>)\n
        //
        // Temp lines never contain parenthesized args.
        assert!(
            raw.contains("tool_a") && raw.contains("jp_cli"),
            "tool_a header and args should both appear.\nOutput:\n{raw}"
        );
        assert!(
            raw.contains("tool_b") && raw.contains("/tmp/test.rs"),
            "tool_b header and args should both appear.\nOutput:\n{raw}"
        );

        // The LAST occurrence of "Calling tool.*tool_a" should be the
        // permanent line (which also contains "jp_cli" on the same
        // write). We verify that permanent lines contain args by
        // checking that the pattern "tool_a(..." and "tool_b(..."
        // appear, which is the FunctionCall format.
        //
        // This is the key anti-regression check: in the old code,
        // args would appear AFTER all headers, so "tool_a" would
        // never be adjacent to "(package:" in the output.
        let has_atomic_a = raw.contains("tool_a\u{1b}[0m(");
        let has_atomic_b = raw.contains("tool_b\u{1b}[0m(");
        assert!(
            has_atomic_a,
            "tool_a should have args immediately after name (atomic permanent \
             line).\nOutput:\n{raw}"
        );
        assert!(
            has_atomic_b,
            "tool_b should have args immediately after name (atomic permanent \
             line).\nOutput:\n{raw}"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

/// Verifies that a single tool call uses "Calling tool" (singular),
/// and that its header+arguments are rendered atomically.
#[tokio::test]
#[expect(clippy::too_many_lines)]
async fn test_single_tool_call_rendered_with_args() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.style.tool_call.show = true;
        config.conversation.tools.defaults.run = RunMode::Unattended;
        config
            .conversation
            .tools
            .insert("fs_read_file".to_string(), ToolConfig {
                source: ToolSource::Local { tool: None },
                command: None,
                run: Some(RunMode::Unattended),
                enable: None,
                summary: None,
                description: None,
                examples: None,
                parameters: IndexMap::new(),
                result: None,
                style: None,
                questions: IndexMap::new(),
                options: Map::new(),
            });

        let mut workspace = Workspace::new(root)
            .persisted_at(&storage)
            .expect("failed to enable persistence");

        let conv_id =
            workspace.create_conversation(Conversation::default(), Arc::new(config.clone()));

        let handle = workspace.acquire_conversation(&conv_id).unwrap();
        let lock = workspace.test_lock(handle);

        let chat_request = ChatRequest::from("Read a file");

        let mut args = Map::new();
        args.insert("path".into(), "/etc/hosts".into());

        let provider: Arc<dyn Provider> = Arc::new({
            let events = vec![
                Event::Part {
                    index: 0,
                    event: ConversationEvent::now(ToolCallRequest {
                        id: "call_1".to_string(),
                        name: "fs_read_file".to_string(),
                        arguments: args,
                    }),
                },
                Event::flush(0),
                Event::Finished(FinishReason::Completed),
            ];

            let followup = vec![
                Event::Part {
                    index: 0,
                    event: ConversationEvent::now(ChatResponse::message("Here.\n\n")),
                },
                Event::flush(0),
                Event::Finished(FinishReason::Completed),
            ];

            SequentialMockProvider {
                responses: vec![events, followup],
                call_index: AtomicUsize::new(0),
                model: ModelDetails::empty(id::ModelIdConfig {
                    provider: ProviderId::Test,
                    name: "single-tool-mock".parse().expect("valid name"),
                }),
            }
        });

        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let (_signal_tx, signal_rx) = broadcast::channel(16);

        let executor_source = TestExecutorSource::new().with_executor("fs_read_file", |req| {
            Box::new(
                MockExecutor::completed(&req.id, &req.name, "file contents")
                    .with_arguments(req.arguments.clone()),
            )
        });
        let tool_defs = executor_source.tool_definitions();

        run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &signal_rx,
            &mcp_client,
            root,
            false,
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request.clone(),
        )
        .await
        .unwrap();

        printer.flush();
        let output = out.lock().clone();

        // Should use singular "Calling tool" (not "tools").
        assert!(
            output.contains("Calling tool"),
            "Output should contain 'Calling tool'.\nOutput:\n{output}"
        );
        assert!(
            !output.contains("Calling tools"),
            "Single tool should use singular, not plural.\nOutput:\n{output}"
        );

        // Header and args should both be present.
        assert!(
            output.contains("fs_read_file"),
            "Should contain tool name.\nOutput:\n{output}"
        );
        assert!(
            output.contains("/etc/hosts"),
            "Should contain tool args.\nOutput:\n{output}"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

/// Mock executor that checks accumulated answers and returns `NeedsInput`
/// for the first unanswered question. When all questions are answered,
/// returns `Completed`. This simulates a tool that requires one or more
/// rounds of inquiry before it can finish.
struct InquiryMockExecutor {
    tool_id: String,
    tool_name: String,
    arguments: Map<String, Value>,
    questions: Vec<Question>,
    output: String,
}

impl InquiryMockExecutor {
    fn new(tool_id: &str, tool_name: &str, questions: Vec<Question>, output: &str) -> Self {
        Self {
            tool_id: tool_id.to_string(),
            tool_name: tool_name.to_string(),
            arguments: Map::new(),
            questions,
            output: output.to_string(),
        }
    }
}

#[async_trait]
impl Executor for InquiryMockExecutor {
    fn tool_id(&self) -> &str {
        &self.tool_id
    }
    fn tool_name(&self) -> &str {
        &self.tool_name
    }
    fn arguments(&self) -> &Map<String, Value> {
        &self.arguments
    }
    fn permission_info(&self) -> Option<PermissionInfo> {
        None
    }
    fn set_arguments(&mut self, _args: Value) {}

    async fn execute(
        &self,
        answers: &IndexMap<String, Value>,
        _mcp_client: &jp_mcp::Client,
        _root: &camino::Utf8Path,
        _cancellation_token: tokio_util::sync::CancellationToken,
    ) -> ExecutorResult {
        for q in &self.questions {
            if !answers.contains_key(&q.id) {
                return ExecutorResult::NeedsInput {
                    tool_id: self.tool_id.clone(),
                    tool_name: self.tool_name.clone(),
                    question: q.clone(),
                    accumulated_answers: answers.clone(),
                };
            }
        }
        ExecutorResult::Completed(jp_conversation::event::ToolCallResponse {
            id: self.tool_id.clone(),
            result: Ok(self.output.clone()),
        })
    }
}

/// Build provider events for a structured inquiry response.
///
/// Emits as `Value::String` to match real provider streaming behavior
/// (the `EventBuilder` parses the JSON string on flush).
fn structured_inquiry_events(inquiry_id: &str, answer: &Value) -> Vec<Event> {
    let data = json!({
        "inquiry_id": inquiry_id,
        "answer": answer,
    });

    vec![
        Event::Part {
            index: 0,
            event: ChatResponse::structured(Value::String(data.to_string())).into(),
        },
        Event::flush(0),
        Event::Finished(FinishReason::Completed),
    ]
}

/// Build provider events for a structured response without `inquiry_id`.
/// Used for parallel inquiry tests where call ordering is non-deterministic.
fn unkeyed_structured_events(answer: &Value) -> Vec<Event> {
    let data = json!({
        "answer": answer,
    });

    vec![
        Event::Part {
            index: 0,
            event: ChatResponse::structured(Value::String(data.to_string())).into(),
        },
        Event::flush(0),
        Event::Finished(FinishReason::Completed),
    ]
}

fn single_tool_call_events(id: &str, name: &str) -> Vec<Event> {
    vec![
        Event::Part {
            index: 0,
            event: ConversationEvent::now(ToolCallRequest {
                id: id.to_string(),
                name: name.to_string(),
                arguments: Map::new(),
            }),
        },
        Event::flush(0),
        Event::Finished(FinishReason::Completed),
    ]
}

fn final_message_events(content: &str) -> Vec<Event> {
    vec![
        Event::Part {
            index: 0,
            event: ConversationEvent::now(ChatResponse::message(content)),
        },
        Event::flush(0),
        Event::Finished(FinishReason::Completed),
    ]
}

/// Create a `ToolConfig` with questions targeting the assistant.
fn inquiry_tool_config(questions: &[&str]) -> ToolConfig {
    ToolConfig {
        source: ToolSource::Local { tool: None },
        command: None,
        run: Some(RunMode::Unattended),
        enable: None,
        summary: None,
        description: None,
        examples: None,
        parameters: IndexMap::new(),
        result: None,
        style: None,
        questions: questions
            .iter()
            .map(|id| {
                (id.to_string(), QuestionConfig {
                    target: QuestionTarget::Assistant(Box::default()),
                    answer: None,
                })
            })
            .collect(),
        options: Map::new(),
    }
}

fn inquiry_mock_model() -> ModelDetails {
    ModelDetails::empty(id::ModelIdConfig {
        provider: ProviderId::Test,
        name: "inquiry-mock".parse().expect("valid name"),
    })
}

/// Tool has one boolean question with `QuestionTarget::Assistant`.
/// Flow: LLM tool call → `NeedsInput` → inquiry → answer → tool completes.
#[tokio::test]
#[expect(clippy::too_many_lines)]
async fn test_tool_with_single_inquiry() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.conversation.tools.defaults.run = RunMode::Unattended;
        config.conversation.tools.insert(
            "inquiry_tool".to_string(),
            inquiry_tool_config(&["confirm"]),
        );

        let mut workspace = Workspace::new(root)
            .persisted_at(&storage)
            .expect("failed to enable persistence");

        let conv_id =
            workspace.create_conversation(Conversation::default(), Arc::new(config.clone()));

        let handle = workspace.acquire_conversation(&conv_id).unwrap();
        let lock = workspace.test_lock(handle);

        let chat_request = ChatRequest::from("Use the tool");

        // Provider call sequence:
        // 1. Tool call
        // 2. Structured inquiry answer (from LlmInquiryBackend)
        // 3. Final message
        let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider {
            responses: vec![
                single_tool_call_events("call_inq", "inquiry_tool"),
                structured_inquiry_events("call_inq.confirm", &json!(true)),
                final_message_events("Inquiry tool completed."),
            ],
            call_index: AtomicUsize::new(0),
            model: inquiry_mock_model(),
        });
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let (_signal_tx, signal_rx) = broadcast::channel(16);

        let executor_source = TestExecutorSource::new().with_executor("inquiry_tool", |req| {
            Box::new(InquiryMockExecutor::new(
                &req.id,
                &req.name,
                vec![Question {
                    id: "confirm".to_string(),
                    text: "Create backup?".to_string(),
                    answer_type: AnswerType::Boolean,
                    default: None,
                }],
                "inquiry tool output",
            ))
        });
        let tool_defs = executor_source.tool_definitions();

        let result = run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &signal_rx,
            &mcp_client,
            root,
            false,
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request,
        )
        .await;

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        printer.flush();
        let output = out.lock();
        assert!(
            output.contains("Inquiry tool completed"),
            "Should contain final LLM response.\nOutput:\n{output}"
        );
        drop(output);

        // Verify the tool response was persisted as successful.
        let events = lock.events().clone();

        let tool_responses: Vec<_> = events
            .clone()
            .into_iter()
            .filter_map(|e| e.event.into_tool_call_response())
            .collect();

        assert_eq!(tool_responses.len(), 1, "Should have one tool response");
        assert!(
            tool_responses[0].result.is_ok(),
            "Tool should have succeeded: {:?}",
            tool_responses[0].result
        );
        assert_eq!(
            tool_responses[0].content(),
            "inquiry tool output",
            "Tool output should match executor output"
        );

        // Verify inquiry events were recorded (RFD 005).
        let req: Vec<_> = events
            .clone()
            .into_iter()
            .filter_map(|e| e.event.into_inquiry_request())
            .collect();
        assert_eq!(req.len(), 1, "Should have one inquiry request");
        assert_eq!(req[0].source, InquirySource::tool("inquiry_tool"));
        assert_eq!(req[0].question.text, "Create backup?");

        let res: Vec<_> = events
            .into_iter()
            .filter_map(|e| e.event.into_inquiry_response())
            .collect();
        assert_eq!(res.len(), 1, "Should have one inquiry response");
        assert_eq!(res[0].answer, json!(true));
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

/// Tool has two questions, each triggering a separate inquiry round.
/// Flow: tool call → `NeedsInput(q1)` → inquiry → answer →
///       `NeedsInput(q2)` → inquiry → answer → completed.
#[tokio::test]
#[expect(clippy::too_many_lines)]
async fn test_tool_with_multiple_inquiries() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.conversation.tools.defaults.run = RunMode::Unattended;
        config.conversation.tools.insert(
            "multi_q_tool".to_string(),
            inquiry_tool_config(&["confirm", "reason"]),
        );

        let mut workspace = Workspace::new(root)
            .persisted_at(&storage)
            .expect("failed to enable persistence");

        let conv_id =
            workspace.create_conversation(Conversation::default(), Arc::new(config.clone()));

        let handle = workspace.acquire_conversation(&conv_id).unwrap();
        let lock = workspace.test_lock(handle);

        let chat_request = ChatRequest::from("Multi-question tool");

        // Provider call sequence:
        // 1. Tool call
        // 2. Structured answer for q1 ("confirm")
        // 3. Structured answer for q2 ("reason")
        // 4. Final message
        let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider {
            responses: vec![
                single_tool_call_events("call_mq", "multi_q_tool"),
                structured_inquiry_events("call_mq.confirm", &json!(true)),
                structured_inquiry_events("call_mq.reason", &json!("performance reasons")),
                final_message_events("Multi-question tool done."),
            ],
            call_index: AtomicUsize::new(0),
            model: inquiry_mock_model(),
        });
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let (_signal_tx, signal_rx) = broadcast::channel(16);

        let executor_source = TestExecutorSource::new().with_executor("multi_q_tool", |req| {
            Box::new(InquiryMockExecutor::new(
                &req.id,
                &req.name,
                vec![
                    Question {
                        id: "confirm".to_string(),
                        text: "Proceed?".to_string(),
                        answer_type: AnswerType::Boolean,
                        default: None,
                    },
                    Question {
                        id: "reason".to_string(),
                        text: "Why?".to_string(),
                        answer_type: AnswerType::Text,
                        default: None,
                    },
                ],
                "both questions answered",
            ))
        });
        let tool_defs = executor_source.tool_definitions();

        let result = run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &signal_rx,
            &mcp_client,
            root,
            false,
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request,
        )
        .await;

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        printer.flush();
        let output = out.lock();
        assert!(
            output.contains("Multi-question tool done"),
            "Should contain final response.\nOutput:\n{output}"
        );
        drop(output);

        let events = lock.events().clone();

        let tool_responses: Vec<_> = events
            .clone()
            .into_iter()
            .filter_map(|e| e.event.into_tool_call_response())
            .collect();

        assert_eq!(tool_responses.len(), 1);
        assert_eq!(tool_responses[0].content(), "both questions answered");

        // Two inquiry rounds should produce two request/response pairs.
        let req: Vec<_> = events
            .clone()
            .into_iter()
            .filter_map(|e| e.event.into_inquiry_request())
            .collect();
        assert_eq!(req.len(), 2, "Should have two inquiry requests");

        let res: Vec<_> = events
            .into_iter()
            .filter_map(|e| e.event.into_inquiry_response())
            .collect();
        assert_eq!(res.len(), 2, "Should have two inquiry responses");
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

/// Two parallel tools: one requires an inquiry, the other completes normally.
/// The inquiry should not block the normal tool from completing.
#[tokio::test]
#[expect(clippy::too_many_lines)]
async fn test_parallel_tools_one_with_inquiry() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.conversation.tools.defaults.run = RunMode::Unattended;
        config.conversation.tools.insert(
            "inquiry_tool".to_string(),
            inquiry_tool_config(&["confirm"]),
        );
        config
            .conversation
            .tools
            .insert("normal_tool".to_string(), ToolConfig {
                source: ToolSource::Local { tool: None },
                command: None,
                run: Some(RunMode::Unattended),
                enable: None,
                summary: None,
                description: None,
                examples: None,
                parameters: IndexMap::new(),
                result: None,
                style: None,
                questions: IndexMap::new(),
                options: Map::new(),
            });

        let mut workspace = Workspace::new(root)
            .persisted_at(&storage)
            .expect("failed to enable persistence");

        let conv_id =
            workspace.create_conversation(Conversation::default(), Arc::new(config.clone()));

        let handle = workspace.acquire_conversation(&conv_id).unwrap();
        let lock = workspace.test_lock(handle);

        let chat_request = ChatRequest::from("Use both tools");

        // Provider call sequence:
        // 1. Two parallel tool calls
        // 2. Structured inquiry answer (for inquiry_tool)
        // 3. Final message
        let parallel_events = vec![
            Event::Part {
                index: 0,
                event: ConversationEvent::now(ToolCallRequest {
                    id: "call_inq".to_string(),
                    name: "inquiry_tool".to_string(),
                    arguments: Map::new(),
                }),
            },
            Event::Part {
                index: 1,
                event: ConversationEvent::now(ToolCallRequest {
                    id: "call_norm".to_string(),
                    name: "normal_tool".to_string(),
                    arguments: Map::new(),
                }),
            },
            Event::flush(0),
            Event::flush(1),
            Event::Finished(FinishReason::Completed),
        ];

        let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider {
            responses: vec![
                parallel_events,
                structured_inquiry_events("call_inq.confirm", &json!(true)),
                final_message_events("Both tools done."),
            ],
            call_index: AtomicUsize::new(0),
            model: inquiry_mock_model(),
        });
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let (_signal_tx, signal_rx) = broadcast::channel(16);

        let executor_source = TestExecutorSource::new()
            .with_executor("inquiry_tool", |req| {
                Box::new(InquiryMockExecutor::new(
                    &req.id,
                    &req.name,
                    vec![Question {
                        id: "confirm".to_string(),
                        text: "Proceed?".to_string(),
                        answer_type: AnswerType::Boolean,
                        default: None,
                    }],
                    "inquiry completed",
                ))
            })
            .with_executor("normal_tool", |req| {
                Box::new(MockExecutor::completed(&req.id, &req.name, "normal output"))
            });
        let tool_defs = executor_source.tool_definitions();

        let result = run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &signal_rx,
            &mcp_client,
            root,
            false,
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request,
        )
        .await;

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        printer.flush();
        let output = out.lock();
        assert!(
            output.contains("Both tools done"),
            "Should contain final response.\nOutput:\n{output}"
        );
        drop(output);

        let events = lock.events().clone();

        let tool_responses: Vec<_> = events
            .into_iter()
            .filter_map(|e| e.event.into_tool_call_response())
            .collect();

        assert_eq!(tool_responses.len(), 2, "Should have two tool responses");

        // Both should have succeeded.
        for r in &tool_responses {
            assert!(
                r.result.is_ok(),
                "Tool {} should succeed: {:?}",
                r.id,
                r.result
            );
        }
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

/// Two parallel tools both requiring inquiries. Uses responses without
/// `inquiry_id` since the concurrent inquiry call order is non-deterministic.
#[tokio::test]
#[expect(clippy::too_many_lines)]
async fn test_parallel_tools_both_with_inquiries() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.conversation.tools.defaults.run = RunMode::Unattended;
        config
            .conversation
            .tools
            .insert("tool_a".to_string(), inquiry_tool_config(&["confirm_a"]));
        config
            .conversation
            .tools
            .insert("tool_b".to_string(), inquiry_tool_config(&["confirm_b"]));

        let mut workspace = Workspace::new(root)
            .persisted_at(&storage)
            .expect("failed to enable persistence");

        let conv_id =
            workspace.create_conversation(Conversation::default(), Arc::new(config.clone()));

        let handle = workspace.acquire_conversation(&conv_id).unwrap();
        let lock = workspace.test_lock(handle);

        let chat_request = ChatRequest::from("Both need inquiries");

        // Provider call sequence:
        // 1. Two parallel tool calls
        // 2. Structured answer (no inquiry_id — order-independent)
        // 3. Structured answer (no inquiry_id — order-independent)
        // 4. Final message
        let parallel_events = vec![
            Event::Part {
                index: 0,
                event: ConversationEvent::now(ToolCallRequest {
                    id: "call_a".to_string(),
                    name: "tool_a".to_string(),
                    arguments: Map::new(),
                }),
            },
            Event::Part {
                index: 1,
                event: ConversationEvent::now(ToolCallRequest {
                    id: "call_b".to_string(),
                    name: "tool_b".to_string(),
                    arguments: Map::new(),
                }),
            },
            Event::flush(0),
            Event::flush(1),
            Event::Finished(FinishReason::Completed),
        ];

        let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider {
            responses: vec![
                parallel_events,
                unkeyed_structured_events(&json!(true)),
                unkeyed_structured_events(&json!(true)),
                final_message_events("Both inquiries resolved."),
            ],
            call_index: AtomicUsize::new(0),
            model: inquiry_mock_model(),
        });
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let (_signal_tx, signal_rx) = broadcast::channel(16);

        let executor_source = TestExecutorSource::new()
            .with_executor("tool_a", |req| {
                Box::new(InquiryMockExecutor::new(
                    &req.id,
                    &req.name,
                    vec![Question {
                        id: "confirm_a".to_string(),
                        text: "Proceed A?".to_string(),
                        answer_type: AnswerType::Boolean,
                        default: None,
                    }],
                    "tool_a done",
                ))
            })
            .with_executor("tool_b", |req| {
                Box::new(InquiryMockExecutor::new(
                    &req.id,
                    &req.name,
                    vec![Question {
                        id: "confirm_b".to_string(),
                        text: "Proceed B?".to_string(),
                        answer_type: AnswerType::Boolean,
                        default: None,
                    }],
                    "tool_b done",
                ))
            });
        let tool_defs = executor_source.tool_definitions();

        let result = run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &signal_rx,
            &mcp_client,
            root,
            false,
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request,
        )
        .await;

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        printer.flush();
        let output = out.lock();
        assert!(
            output.contains("Both inquiries resolved"),
            "Should contain final response.\nOutput:\n{output}"
        );
        drop(output);

        let events = lock.events().clone();

        let tool_responses: Vec<_> = events
            .into_iter()
            .filter_map(|e| e.event.into_tool_call_response())
            .collect();

        assert_eq!(tool_responses.len(), 2, "Should have two tool responses");
        for r in &tool_responses {
            assert!(
                r.result.is_ok(),
                "Tool {} should succeed: {:?}",
                r.id,
                r.result
            );
        }
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

/// Verifies that the retry counter resets after a successful event arrives in a
/// new streaming cycle.
///
/// Scenario with `max_retries=1`:
///   1. Stream produces content, then rate-limits mid-stream (no Finished)
///   2. Retry: stream produces content (counter resets here), then rate-limits
///      again mid-stream
///   3. Retry: stream completes successfully
///
/// Without the fix, the counter would reach 2 after step 2, exceeding the
/// budget of 1 and causing a hard failure.
#[tokio::test]
#[expect(clippy::too_many_lines)]
async fn test_retry_counter_resets_on_successful_event() {
    struct MidStreamRateLimitProvider {
        call_index: Arc<AtomicUsize>,
        model: ModelDetails,
    }

    #[async_trait]
    impl Provider for MidStreamRateLimitProvider {
        async fn model_details(&self, name: &id::Name) -> Result<ModelDetails, LlmError> {
            let mut m = self.model.clone();
            m.id.name = name.clone();
            Ok(m)
        }

        async fn models(&self) -> Result<Vec<ModelDetails>, LlmError> {
            Ok(vec![self.model.clone()])
        }

        async fn chat_completion_stream(
            &self,
            _model: &ModelDetails,
            _query: ChatQuery,
        ) -> Result<EventStream, LlmError> {
            let idx = self.call_index.fetch_add(1, Ordering::SeqCst);

            let events: Vec<Result<Event, StreamError>> = if idx < 2 {
                // Calls 0 and 1: partial content then rate limit error.
                vec![
                    Ok(Event::Part {
                        index: 0,
                        event: ChatResponse::message("partial ").into(),
                    }),
                    Err(StreamError::rate_limit(None)),
                ]
            } else {
                // Call 2: complete successfully.
                vec![
                    Ok(Event::Part {
                        index: 0,
                        event: ChatResponse::message("done.").into(),
                    }),
                    Ok(Event::flush(0)),
                    Ok(Event::Finished(FinishReason::Completed)),
                ]
            };

            Ok(Box::pin(stream::iter(events)))
        }
    }

    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.assistant.request.max_retries = 1;
        config.assistant.request.base_backoff_ms = 1;
        config.assistant.request.max_backoff_secs = 1;

        let mut workspace = Workspace::new(root)
            .persisted_at(&storage)
            .expect("failed to enable persistence");

        let conv_id =
            workspace.create_conversation(Conversation::default(), Arc::new(config.clone()));

        let handle = workspace.acquire_conversation(&conv_id).unwrap();
        let lock = workspace.test_lock(handle);

        let chat_request = ChatRequest::from("Hello");
        let call_index = Arc::new(AtomicUsize::new(0));
        let call_index_clone = Arc::clone(&call_index);

        // Provider that returns partial content + rate limit error on the first
        // two calls, then succeeds on the third.
        let provider: Arc<dyn Provider> = Arc::new(MidStreamRateLimitProvider {
            call_index: call_index_clone,
            model: ModelDetails::empty(id::ModelIdConfig {
                provider: ProviderId::Test,
                name: "rate-limit-mock".parse().expect("valid name"),
            }),
        });
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let (_signal_tx, signal_rx) = broadcast::channel(16);

        let result = run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &signal_rx,
            &mcp_client,
            root,
            false,
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer,
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            chat_request,
        )
        .await;

        // With the fix, this succeeds (counter resets between retries). Without
        // the fix, this would fail with a rate limit error after the second
        // stream failure exhausts the budget.
        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        // Provider should have been called 3 times:
        // call 0: partial + rate limit
        // call 1: partial + rate limit (budget restored by reset)
        // call 2: success
        let total_calls = call_index.load(Ordering::SeqCst);
        assert_eq!(
            total_calls, 3,
            "Expected 3 provider calls (2 partial + 1 success), got {total_calls}"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

/// When the inquiry provider returns a non-structured response, the inquiry
/// fails and the tool is marked as completed with an error.
#[tokio::test]
async fn test_inquiry_failure_marks_tool_as_error() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.conversation.tools.defaults.run = RunMode::Unattended;
        config.conversation.tools.insert(
            "inquiry_tool".to_string(),
            inquiry_tool_config(&["confirm"]),
        );

        let mut workspace = Workspace::new(root)
            .persisted_at(&storage)
            .expect("failed to enable persistence");

        let conv_id =
            workspace.create_conversation(Conversation::default(), Arc::new(config.clone()));

        let handle = workspace.acquire_conversation(&conv_id).unwrap();
        let lock = workspace.test_lock(handle);

        let chat_request = ChatRequest::from("Use the tool");

        // Provider call sequence:
        // 1. Tool call
        // 2. Plain message (NOT structured) → inquiry fails
        // 3. Final message (LLM sees the error and responds)
        let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider {
            responses: vec![
                single_tool_call_events("call_fail", "inquiry_tool"),
                final_message_events("I don't understand the question."),
                final_message_events("The tool failed, sorry."),
            ],
            call_index: AtomicUsize::new(0),
            model: inquiry_mock_model(),
        });
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let (_signal_tx, signal_rx) = broadcast::channel(16);

        let executor_source = TestExecutorSource::new().with_executor("inquiry_tool", |req| {
            Box::new(InquiryMockExecutor::new(
                &req.id,
                &req.name,
                vec![Question {
                    id: "confirm".to_string(),
                    text: "Confirm?".to_string(),
                    answer_type: AnswerType::Boolean,
                    default: None,
                }],
                "should not reach this",
            ))
        });
        let tool_defs = executor_source.tool_definitions();

        let result = run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &signal_rx,
            &mcp_client,
            root,
            false,
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request,
        )
        .await;

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        let events = lock.events().clone();

        let tool_responses: Vec<_> = events
            .into_iter()
            .filter_map(|e| e.event.into_tool_call_response())
            .collect();

        assert_eq!(tool_responses.len(), 1, "Should have one tool response");
        assert!(
            tool_responses[0].result.is_err(),
            "Tool should have failed: {:?}",
            tool_responses[0].result
        );
        let content = tool_responses[0].content();
        assert!(
            content.contains("secondary assistant failed"),
            "Error should explain the inquiry failure: {content}",
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}
