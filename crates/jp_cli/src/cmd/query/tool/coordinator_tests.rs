use async_trait::async_trait;
use camino::Utf8PathBuf;
use camino_tempfile::Utf8TempDir;
use jp_config::{
    AppConfig,
    conversation::tool::{ToolConfig, ToolSource, style::PartialDisplayStyleConfig},
};
use jp_conversation::{Conversation, event::ChatRequest};
use jp_inquire::{
    ReplyEditMode, ReplyOutcome,
    prompt::{MockPromptBackend, PromptBackend},
};
use jp_mcp::{Client, server::builtin::BuiltinExecutors};
use jp_printer::{ErrChannel, OutputFormat, Printer};
use jp_process::MockProcessRunner;
use jp_tool::{InvocationContext, ToolDefinition, ToolDocs};
use jp_workspace::{ConversationLock, Workspace};
use schematic::Config as _;
use serde_json::json;

use super::*;
use crate::{
    access::approvals::ApprovalStore,
    cmd::query::{
        tool::{
            executor::mock::{MockExecutor, TestExecutorSource},
            inquiry::MockInquiryBackend,
            mcp_executor::TerminalExecutorSource,
        },
        turn::TurnCoordinator,
    },
    render::tool::ToolRenderer,
    signals::testing::detached_router,
};

fn empty_executor_source() -> Box<dyn ExecutorSource> {
    Box::new(TestExecutorSource::new())
}

/// A lock on an empty in-memory conversation, for a permission phase whose
/// calls ask no questions and so record nothing on it.
fn test_lock() -> (Workspace, ConversationLock) {
    let config = Arc::new(jp_config::AppConfig::new_test());
    let mut workspace = Workspace::in_memory(Utf8PathBuf::new());
    let id = workspace.create_conversation(Conversation::default(), config);
    let handle = workspace.acquire_conversation(&id).unwrap();
    let lock = workspace.test_lock(handle);
    (workspace, lock)
}

fn strip_ansi(text: &str) -> String {
    String::from_utf8(strip_ansi_escapes::strip(text)).expect("valid utf-8 after stripping ANSI")
}

#[test]
fn test_is_prompting_default_false() {
    let coordinator = ToolCoordinator::new(
        jp_config::AppConfig::new_test().conversation.tools,
        empty_executor_source(),
    );
    assert!(!coordinator.is_prompting());
}

#[test]
fn test_is_prompting_derived_from_states() {
    let mut coordinator = ToolCoordinator::new(
        jp_config::AppConfig::new_test().conversation.tools,
        empty_executor_source(),
    );

    // No tools = not prompting
    assert!(!coordinator.is_prompting());

    // Add a tool in Pending state = not prompting
    coordinator.set_tool_state("tool_1", ToolCallState::Queued);
    assert!(!coordinator.is_prompting());

    // Add a tool in Running state = not prompting
    coordinator.set_tool_state("tool_1", ToolCallState::Running);
    assert!(!coordinator.is_prompting());

    // Set to AwaitingPermission = prompting
    coordinator.set_tool_state("tool_1", ToolCallState::AwaitingPermission);
    assert!(coordinator.is_prompting());

    // Set to AwaitingInput = prompting
    coordinator.set_tool_state("tool_1", ToolCallState::AwaitingInput);
    assert!(coordinator.is_prompting());

    // Set to AwaitingResultEdit = prompting
    coordinator.set_tool_state("tool_1", ToolCallState::AwaitingResultEdit);
    assert!(coordinator.is_prompting());

    // Set to Completed = not prompting
    coordinator.set_tool_state("tool_1", ToolCallState::Completed);
    assert!(!coordinator.is_prompting());
}

#[test]
fn test_is_prompting_any_tool() {
    let mut coordinator = ToolCoordinator::new(
        jp_config::AppConfig::new_test().conversation.tools,
        empty_executor_source(),
    );

    // Multiple tools, none prompting
    coordinator.set_tool_state("tool_1", ToolCallState::Running);
    coordinator.set_tool_state("tool_2", ToolCallState::Completed);
    coordinator.set_tool_state("tool_3", ToolCallState::Queued);
    assert!(!coordinator.is_prompting());

    // One tool prompting = is_prompting returns true
    coordinator.set_tool_state("tool_2", ToolCallState::AwaitingInput);
    assert!(coordinator.is_prompting());
}

#[test]
fn test_tool_call_state_is_prompting() {
    assert!(!ToolCallState::Queued.is_prompting());
    assert!(ToolCallState::AwaitingPermission.is_prompting());
    assert!(!ToolCallState::Running.is_prompting());
    assert!(ToolCallState::AwaitingInput.is_prompting());
    assert!(ToolCallState::AwaitingResultEdit.is_prompting());
    assert!(!ToolCallState::Completed.is_prompting());
}

#[test]
fn test_cancel_does_not_panic() {
    let coordinator = ToolCoordinator::new(
        jp_config::AppConfig::new_test().conversation.tools,
        empty_executor_source(),
    );
    // Should not panic
    coordinator.cancel();
    // Calling cancel multiple times should also not panic
    coordinator.cancel();
}

#[test]
fn test_result_mode_default() {
    let coordinator = ToolCoordinator::new(
        jp_config::AppConfig::new_test().conversation.tools,
        empty_executor_source(),
    );
    // Non-existent tool returns default (Unattended)
    assert_eq!(
        coordinator.result_mode("nonexistent_tool"),
        ResultMode::Unattended
    );
}

#[test]
fn test_result_mode_with_configured_tool() {
    use jp_config::conversation::tool::{ToolConfig, ToolSource};
    use schematic::Config as _;

    // Create a tool config with a specific result mode
    let tool_config = ToolConfig::from_partial(
        jp_config::conversation::tool::PartialToolConfig {
            source: Some(ToolSource::Builtin { tool: None }),
            result: Some(ResultMode::Ask),
            ..Default::default()
        },
        vec![],
    )
    .expect("valid tool config");

    let mut tools_config = jp_config::AppConfig::new_test().conversation.tools;
    tools_config.insert("my_tool".to_string(), tool_config);

    let coordinator = ToolCoordinator::new(tools_config, empty_executor_source());

    // Configured tool returns the configured mode
    assert_eq!(coordinator.result_mode("my_tool"), ResultMode::Ask);

    // Non-existent tool still returns default
    assert_eq!(
        coordinator.result_mode("other_tool"),
        ResultMode::Unattended
    );
}

/// A tool the model named but the config never declared reads its membership
/// from the `*` defaults block, the same place replay reads it from.
///
/// A model can emit a name it was never offered, and `tool_definitions` builds
/// the offered set from this very map, so the name arrives here with no entry
/// to consult.
#[test]
fn joins_reasoning_falls_back_to_the_defaults_block_for_an_unconfigured_tool() {
    let mut tools = jp_config::AppConfig::new_test().conversation.tools;
    tools.defaults.style.joins_reasoning = false;

    let coordinator = ToolCoordinator::new(tools, empty_executor_source());

    assert!(!coordinator.joins_reasoning("a_tool_the_model_invented"));
}

#[test]
fn test_question_target_nonexistent_tool() {
    let coordinator = ToolCoordinator::new(
        jp_config::AppConfig::new_test().conversation.tools,
        empty_executor_source(),
    );
    // Non-existent tool returns None
    assert!(
        coordinator
            .question_target("nonexistent_tool", "any_question")
            .is_none()
    );
}

#[test]
fn test_question_target_with_configured_question() {
    use jp_config::conversation::tool::{QuestionTarget, ToolConfig, ToolSource};
    use schematic::Config as _;

    // Create a tool config with a question
    let tool_config = ToolConfig::from_partial(
        jp_config::conversation::tool::PartialToolConfig {
            source: Some(ToolSource::Builtin { tool: None }),
            questions: indexmap::indexmap! {
                "confirm".to_string() => jp_config::conversation::tool::PartialQuestionConfig {
                    target: Some(QuestionTarget::Assistant(Box::default())),
                    answer: None,
                }
            }
            .into(),
            ..Default::default()
        },
        vec![],
    )
    .expect("valid tool config");

    let mut tools_config = jp_config::AppConfig::new_test().conversation.tools;
    tools_config.insert("my_tool".to_string(), tool_config);

    let coordinator = ToolCoordinator::new(tools_config, empty_executor_source());

    // Configured question returns the target
    assert_eq!(
        coordinator.question_target("my_tool", "confirm"),
        Some(QuestionTarget::Assistant(Box::default()))
    );

    // Non-existent question returns None
    assert!(
        coordinator
            .question_target("my_tool", "other_question")
            .is_none()
    );
}

#[test]
fn test_static_answer_nonexistent_tool() {
    let coordinator = ToolCoordinator::new(
        jp_config::AppConfig::new_test().conversation.tools,
        empty_executor_source(),
    );
    assert!(
        coordinator
            .static_answer("nonexistent_tool", "any_question")
            .is_none()
    );
}

#[test]
fn test_static_answer_with_configured_answer() {
    use jp_config::conversation::tool::{QuestionTarget, ToolConfig, ToolSource};
    use schematic::Config as _;

    // Create a tool config with a question that has a static answer
    let tool_config = ToolConfig::from_partial(
        jp_config::conversation::tool::PartialToolConfig {
            source: Some(ToolSource::Builtin { tool: None }),
            questions: indexmap::indexmap! {
                "confirm".to_string() => jp_config::conversation::tool::PartialQuestionConfig {
                    target: Some(QuestionTarget::User),
                    answer: Some(serde_json::json!(true)),
                },
                "no_answer".to_string() => jp_config::conversation::tool::PartialQuestionConfig {
                    target: Some(QuestionTarget::User),
                    answer: None,
                }
            }
            .into(),
            ..Default::default()
        },
        vec![],
    )
    .expect("valid tool config");

    let mut tools_config = jp_config::AppConfig::new_test().conversation.tools;
    tools_config.insert("my_tool".to_string(), tool_config);

    let coordinator = ToolCoordinator::new(tools_config, empty_executor_source());

    // Question with static answer returns the answer
    assert_eq!(
        coordinator.static_answer("my_tool", "confirm"),
        Some(serde_json::json!(true))
    );

    // Question without static answer returns None
    assert!(coordinator.static_answer("my_tool", "no_answer").is_none());

    // Non-existent question returns None
    assert!(
        coordinator
            .static_answer("my_tool", "nonexistent")
            .is_none()
    );
}

/// Build a coordinator around a single tool with the given parameter style.
fn coordinator_with_style(name: &str, parameters: ParametersStyle) -> ToolCoordinator {
    let tool_config = ToolConfig::from_partial(
        jp_config::conversation::tool::PartialToolConfig {
            source: Some(ToolSource::Builtin { tool: None }),
            style: Some(PartialDisplayStyleConfig {
                parameters: Some(parameters),
                ..Default::default()
            }),
            ..Default::default()
        },
        vec![],
    )
    .expect("valid tool config");

    let mut tools_config = jp_config::AppConfig::new_test().conversation.tools;
    tools_config.insert(name.to_owned(), tool_config);
    ToolCoordinator::new(tools_config, empty_executor_source())
}

#[test]
fn test_pre_render_for_prompt_function_call_fires_before_approval() {
    // Regression test for the bug where `fs_delete_file`-style tools
    // (built-in parameter style + `run = "ask"`) showed the permission prompt
    // without first rendering the arguments. Deferral exists to hold back a
    // side-effecting custom formatter, and must not suppress rendering for the
    // pure built-in styles.
    let coordinator = coordinator_with_style("fs_delete_file", ParametersStyle::FunctionCall);

    let (printer, _stdout, stderr) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let tool_renderer = ToolRenderer::new(
        ErrChannel::new(printer.clone()),
        jp_config::AppConfig::new_test().style,
    );

    let mut args = Map::new();
    args.insert("path".into(), Value::String("src/foo.rs".into()));
    let executor = MockExecutor::completed("call-1", "fs_delete_file", "done")
        .with_arguments(args)
        .with_permission_info(PermissionInfo {
            tool_id: "call-1".into(),
            tool_name: "fs_delete_file".into(),
            tool_source: ToolSource::Builtin { tool: None },
            run_mode: RunMode::Ask,
            arguments: Value::Object(Map::new()),
        });

    let result = coordinator.pre_render_for_prompt(&executor, &tool_renderer);

    // Built-in styles print their arguments inline, so they render before the
    // prompt and produce no content for the caller to persist.
    assert!(
        matches!(result, PreRender::Ready(None)),
        "pre-render should fire for FunctionCall style, got: {result:?}"
    );

    printer.flush();
    assert_eq!(
        strip_ansi(&stderr.lock()),
        "Calling tool fs_delete_file(path: \"src/foo.rs\")\n"
    );
}

#[test]
fn test_pre_render_for_prompt_custom_defers_until_the_service_formats() {
    // Counterpart to the test above: a Custom formatter is a user-controlled
    // command run by the execution service, so until the service reports its
    // output there is nothing to show and rendering defers.
    use jp_config::conversation::tool::CommandConfigOrString;

    let coordinator = coordinator_with_style(
        "custom_tool",
        ParametersStyle::Custom(CommandConfigOrString::String("echo SHOULD-NOT-RUN".into())),
    );

    let (printer, _stdout, stderr) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let tool_renderer = ToolRenderer::new(
        ErrChannel::new(printer.clone()),
        jp_config::AppConfig::new_test().style,
    );

    // A mock executor never formats arguments, standing in for a service that
    // has not run the formatter yet.
    let executor = MockExecutor::completed("call-1", "custom_tool", "done");
    let result = coordinator.pre_render_for_prompt(&executor, &tool_renderer);

    assert!(
        matches!(result, PreRender::Deferred),
        "an unformatted Custom style should defer rendering, got: {result:?}"
    );

    printer.flush();
    // Nothing at all is printed: not the formatter's output, and not a header
    // with nothing under it.
    assert_eq!(strip_ansi(&stderr.lock()), "");
}

/// Take `requests` from arrival to their responses through `coordinator`, as
/// the turn loop does once the response has finished streaming.
///
/// Prompts are answered by `prompts`, and questions routed to the assistant by
/// `inquiries`.
async fn drive(
    coordinator: &mut ToolCoordinator,
    requests: Vec<ToolCallRequest>,
    prompts: Arc<dyn PromptBackend>,
    inquiries: Arc<dyn InquiryBackend>,
    turn_state: &mut TurnState,
    conv: &ConversationMut,
    printer: &Arc<Printer>,
) -> ExecutionResult {
    let style = jp_config::AppConfig::new_test().style;
    let mut renderer = ToolRenderer::new(ErrChannel::new(printer.clone()), style.clone());
    let prompter = Arc::new(ToolPrompter::with_backends(
        printer.clone(),
        None,
        Arc::clone(&prompts),
    ));
    let mut host = Host {
        prompter: &prompter,
        inquiry_backend: &inquiries,
        conv,
        turn_state,
        renderer: &mut renderer,
        printer,
        interactive: true,
    };
    for request in requests {
        coordinator.submit(request, &mut host);
    }

    let router = detached_router();
    let mut turn_coordinator = TurnCoordinator::new(printer.clone(), style, None, None, None);
    let mut interrupt_ui = InterruptUi {
        turn_coordinator: &mut turn_coordinator,
        printer,
        backend: prompts.as_ref(),
        editor: None,
        edit_mode: ReplyEditMode::default(),
    };
    coordinator
        .finish(
            &mut host,
            &router,
            &mut interrupt_ui,
            &mut TurnInterrupts::none(),
        )
        .await
}

/// Minimal `Executor` whose `set_arguments` actually mutates state.
///
/// `MockExecutor::set_arguments` is a no-op, which is fine for tests that don't
/// exercise the prompt-edit path but useless for verifying the
/// pre-render-invalidation logic around the approval prompt.
struct EditableExecutor {
    tool_id: String,
    tool_name: String,
    arguments: std::sync::Mutex<Map<String, Value>>,
    permission_info: PermissionInfo,
}

#[async_trait]
impl Executor for EditableExecutor {
    fn tool_id(&self) -> &str {
        &self.tool_id
    }
    fn tool_name(&self) -> &str {
        &self.tool_name
    }
    fn arguments(&self) -> Map<String, Value> {
        self.arguments.lock().unwrap().clone()
    }
    fn permission_info(&self) -> Option<PermissionInfo> {
        Some(self.permission_info.clone())
    }
    fn set_arguments(&self, args: Value) {
        if let Value::Object(map) = args {
            *self.arguments.lock().unwrap() = map;
        }
    }
    async fn execute(
        &self,
        _answers: &IndexMap<String, Value>,
        _cancellation_token: tokio_util::sync::CancellationToken,
        _stderr: Option<jp_mcp::server::StderrSink>,
    ) -> ExecutorResult {
        ExecutorResult::Completed(ToolCallResponse {
            id: self.tool_id.clone(),
            result: Ok(format!("deleted {}", self.arguments()["path"])),
        })
    }
}

#[tokio::test]
async fn an_edit_at_the_approval_prompt_draws_the_call_again() {
    // Regression: when the user picks `e` (edit) at the approval prompt and
    // changes arguments, the call drawn before the prompt would otherwise
    // remain as the rendered-of-record while the executor runs the post-edit
    // args. Verify the call is drawn again with the args that will execute.
    let tool_config = ToolConfig::from_partial(
        jp_config::conversation::tool::PartialToolConfig {
            source: Some(ToolSource::Builtin { tool: None }),
            run: Some(RunMode::Ask),
            style: Some(PartialDisplayStyleConfig {
                parameters: Some(ParametersStyle::FunctionCall),
                inline_results: Some(jp_config::conversation::tool::style::InlineResults::Off),
                results_file_link: Some(jp_config::conversation::tool::style::LinkStyle::Off),
                ..Default::default()
            }),
            ..Default::default()
        },
        vec![],
    )
    .expect("valid tool config");

    let mut tools_config = jp_config::AppConfig::new_test().conversation.tools;
    tools_config.insert("fs_delete_file".to_string(), tool_config);

    let source = TestExecutorSource::new().with_executor("fs_delete_file", |request| {
        Box::new(EditableExecutor {
            tool_id: request.id.clone(),
            tool_name: request.name.clone(),
            arguments: std::sync::Mutex::new(request.arguments.clone()),
            permission_info: PermissionInfo {
                tool_id: request.id.clone(),
                tool_name: request.name.clone(),
                tool_source: ToolSource::Builtin { tool: None },
                run_mode: RunMode::Ask,
                arguments: Value::Object(request.arguments),
            },
        })
    });
    let mut coordinator = ToolCoordinator::new(tools_config, Box::new(source));

    let (printer, _stdout, stderr) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);

    // The prompt backend picks `e` (edit arguments) and supplies the post-edit
    // JSON through the inline reply widget.
    let post_edit = serde_json::json!({"path": "src/bar.rs"});
    let prompts = MockPromptBackend::new()
        .with_inline_responses(['e'])
        .with_reply_outcomes([ReplyOutcome::Submit(
            serde_json::to_string(&post_edit).unwrap(),
        )]);

    let mut turn_state = TurnState::default();
    let (_workspace, lock) = test_lock();
    let result = drive(
        &mut coordinator,
        vec![ToolCallRequest {
            id: "call_1".into(),
            name: "fs_delete_file".into(),
            arguments: json!({"path": "src/foo.rs"}).as_object().unwrap().clone(),
        }],
        Arc::new(prompts),
        Arc::new(MockInquiryBackend::new(HashMap::new())),
        &mut turn_state,
        &lock.as_mut(),
        &printer,
    )
    .await;

    assert_eq!(
        result.reviews["call_1"].response,
        ToolCallResponse {
            id: "call_1".into(),
            result: Ok("deleted \"src/bar.rs\"".into()),
        },
        "the call runs with the post-edit args"
    );

    printer.flush();
    assert_eq!(
        strip_ansi(&stderr.lock()),
        "Calling tool fs_delete_file(path: \"src/foo.rs\")\nCalling tool fs_delete_file(path: \
         \"src/bar.rs\")\n"
    );
}

#[test]
fn test_permission_decision_cache_is_isolated_from_answers() {
    let mut coordinator = ToolCoordinator::new(
        jp_config::AppConfig::new_test().conversation.tools,
        empty_executor_source(),
    );
    let mut turn_state = TurnState::default();

    let info = PermissionInfo {
        tool_id: "call_1".into(),
        tool_name: "my_tool".into(),
        tool_source: ToolSource::Builtin { tool: None },
        run_mode: RunMode::Ask,
        arguments: Value::Null,
    };

    // Persisting a "run" decision lands only in the permission cache.
    coordinator
        .apply_permission_result(
            Ok(PermissionResult::Run {
                arguments: Value::Null,
                persist: true,
            }),
            &info,
            &mut turn_state,
            &MockExecutor::completed("call_1", "my_tool", "ok"),
        )
        .expect("a run decision approves the call");

    assert_eq!(turn_state.remembered_permission("my_tool"), Some(true));
    assert!(
        turn_state.tools["my_tool"].answers.is_empty(),
        "a permission decision must not leak into the tool-answer cache"
    );

    // A later call for the same tool reuses the decision without prompting.
    let executor = MockExecutor::completed("call_2", "my_tool", "ok").with_permission_info(info);
    let decision = coordinator.decide_permission(&executor, true, &turn_state);
    assert!(
        matches!(decision, PermissionDecision::Approved),
        "a remembered `y` decision approves without prompting"
    );
}

#[test]
fn test_cancellation_reason_mapping() {
    // `InquiryError::Cancelled` is a user-initiated cancellation; every other
    // variant is a genuine backend failure.
    assert_eq!(
        ToolCoordinator::cancellation_reason(&InquiryError::Cancelled),
        CancellationReason::User
    );
    assert_eq!(
        ToolCoordinator::cancellation_reason(&InquiryError::MissingStructuredData),
        CancellationReason::BackendError
    );
    assert_eq!(
        ToolCoordinator::cancellation_reason(&InquiryError::AnswerExtraction {
            reason: "nope".into()
        }),
        CancellationReason::BackendError
    );
    assert_eq!(
        ToolCoordinator::cancellation_reason(&InquiryError::Other("boom".into())),
        CancellationReason::BackendError
    );
}

#[test]
fn test_prompt_cancellation_reason_mapping() {
    // Esc/Ctrl-C/EOF at the prompt is a user cancellation; any other prompt
    // failure (I/O, no TTY, writer errors) is a backend error.
    assert_eq!(
        ToolCoordinator::prompt_cancellation_reason(&Error::Inquire(
            InquireError::OperationCanceled
        )),
        CancellationReason::User
    );
    assert_eq!(
        ToolCoordinator::prompt_cancellation_reason(&Error::Inquire(
            InquireError::OperationInterrupted
        )),
        CancellationReason::User
    );
    assert_eq!(
        ToolCoordinator::prompt_cancellation_reason(&Error::Inquire(InquireError::NotTTY)),
        CancellationReason::BackendError
    );
    assert_eq!(
        ToolCoordinator::prompt_cancellation_reason(&Error::Fmt(std::fmt::Error)),
        CancellationReason::BackendError
    );
}

#[test]
fn a_custom_style_shows_the_key_the_assistant_called() {
    // A `source` that names an implementation (`local.fs_list_files` under the
    // key `ls`) changes the name the tool runs under, but the header the user
    // reads stays the name the assistant called.
    use jp_config::conversation::tool::CommandConfigOrString;

    let tool_config = ToolConfig::from_partial(
        jp_config::conversation::tool::PartialToolConfig {
            source: Some(ToolSource::Local {
                tool: Some("fs_list_files".to_owned()),
            }),
            style: Some(PartialDisplayStyleConfig {
                parameters: Some(ParametersStyle::Custom(CommandConfigOrString::String(
                    "echo {{tool.name}}".to_owned(),
                ))),
                ..Default::default()
            }),
            ..Default::default()
        },
        vec![],
    )
    .expect("valid tool config");

    let mut tools_config = jp_config::AppConfig::new_test().conversation.tools;
    tools_config.insert("ls".to_owned(), tool_config);

    let (printer, _stdout, stderr) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let tool_renderer = ToolRenderer::new(
        ErrChannel::new(printer.clone()),
        jp_config::AppConfig::new_test().style,
    );

    // The execution service ran the formatter and reported what it printed;
    // nothing here shells out to produce this.
    let content = tool_renderer.render_custom_result("ls", "fs_list_files".into());

    assert_eq!(content.as_deref(), Some("fs_list_files"));

    printer.flush();
    assert_eq!(
        strip_ansi(&stderr.lock()),
        "Calling tool ls\n\nfs_list_files\n"
    );
}

#[tokio::test]
async fn remembered_denial_does_not_run_http_argument_formatter() {
    let root = Utf8TempDir::new().unwrap();
    let mut config = AppConfig::new_test();
    let partial = serde_json::from_value(json!({
        "source":"builtin", "run":"ask", "format":"unattended",
        "style":{"parameters":{"program":"formatter", "args":[], "shell":false}}
    }))
    .unwrap();
    let runner = Arc::new(MockProcessRunner::never_called());
    config.conversation.tools.insert(
        "example".into(),
        ToolConfig::from_partial(partial, vec![]).unwrap(),
    );
    let definitions = vec![ToolDefinition {
        name: "example".into(),
        docs: ToolDocs::default(),
        parameters: json!({"type":"object","properties":{}}),
    }];
    let (source, owner) = TerminalExecutorSource::start(
        BuiltinExecutors::new(),
        runner.clone(),
        &definitions,
        &config.conversation.tools,
        Arc::new(ApprovalStore::default()),
        InvocationContext::default(),
        &Client::default(),
        root.path().to_owned(),
    )
    .await
    .unwrap();
    let mut coordinator = ToolCoordinator::new(config.conversation.tools.clone(), Box::new(source));
    let printer = Arc::new(Printer::sink());
    let mut state = TurnState::default();
    state.remember_permission("example", false);
    let (_workspace, lock) = test_lock();
    let result = drive(
        &mut coordinator,
        vec![ToolCallRequest {
            id: "call-1".into(),
            name: "example".into(),
            arguments: Map::new(),
        }],
        Arc::new(MockPromptBackend::new()),
        Arc::new(MockInquiryBackend::new(HashMap::new())),
        &mut state,
        &lock.as_mut(),
        &printer,
    )
    .await;
    let response = result.reviews["call-1"].response.clone();
    assert_eq!(response, ToolCallResponse {
        id: "call-1".into(),
        result: Ok("Tool skipped by user (remembered).".into()),
    });
    assert_eq!(
        runner.calls(),
        vec![],
        "a call the user already denied must not run its formatter"
    );
    coordinator
        .acknowledge_reviews(vec![Review::unchanged(response)])
        .await
        .unwrap();
    owner.shutdown().await.unwrap();
}

/// Take one unattended call to `example` to its response, where the formatter
/// asks the tool's `confirm` question and describes the call with the answer.
///
/// The configuration routes the question to the assistant, which answers from
/// `answers`, keyed by inquiry id.
/// Returns the call's response, what was persisted for its description, and the
/// inquiry responses the conversation recorded.
async fn decide_with_assistant(
    answers: HashMap<String, Value>,
) -> (ToolCallResponse, Option<String>, Vec<InquiryResponse>) {
    let partial: jp_config::conversation::tool::PartialToolConfig = serde_json::from_value(json!({
        "source": "builtin", "run": "unattended",
        "questions": {"confirm": {"target": "assistant"}},
        "style": {"parameters": "describe"},
    }))
    .unwrap();
    let mut tools = jp_config::AppConfig::new_test().conversation.tools;
    tools.insert(
        "example".into(),
        ToolConfig::from_partial(partial, vec![]).unwrap(),
    );
    let source = TestExecutorSource::new().with_executor("example", |request| {
        Box::new(AskingFormatter::new(request.id, request.name, false))
    });
    let mut coordinator = ToolCoordinator::new(tools, Box::new(source));
    let request = ToolCallRequest {
        id: "call-1".into(),
        name: "example".into(),
        arguments: Map::new(),
    };

    // The assistant is asked from the conversation as it stands, so the turn
    // holding the call has to exist.
    let (_workspace, lock) = test_lock();
    let conv = lock.as_mut();
    conv.update_events(|events| {
        events.start_turn(ChatRequest::from("Run it."));
        events
            .current_turn_mut()
            .add_tool_call_request(request.clone())
            .build()
            .unwrap();
    });

    let printer = Arc::new(Printer::sink());
    let result = drive(
        &mut coordinator,
        vec![request],
        Arc::new(MockPromptBackend::new()),
        Arc::new(MockInquiryBackend::new(answers)),
        &mut TurnState::default(),
        &conv,
        &printer,
    )
    .await;
    let inquiries = conv
        .events()
        .iter()
        .filter_map(|event| event.event.as_inquiry_response())
        .cloned()
        .collect();
    let described = coordinator.drain_rendered_arguments().remove("call-1");

    let response = result.reviews["call-1"].response.clone();
    (response, described, inquiries)
}

/// The assistant answers the formatter's question before the call is decided,
/// so the call is shown, and recorded, as the formatter describes it with the
/// answer.
#[tokio::test]
async fn the_assistant_answers_a_formatters_question_before_the_call_is_decided() {
    let (response, described, inquiries) = decide_with_assistant(HashMap::from([(
        "call-1.confirm.1".to_owned(),
        json!(true),
    )]))
    .await;

    assert_eq!(response, ToolCallResponse {
        id: "call-1".into(),
        result: Ok("ran".into()),
    });
    assert_eq!(described.as_deref(), Some("confirm = true"));
    assert_eq!(inquiries, vec![InquiryResponse::answered(
        InquiryId::new("call-1.confirm.1".to_owned()),
        json!(true),
    )]);
}

/// An assistant that cannot answer the formatter's question settles the call
/// there, with a response telling the model why, and nothing runs.
#[tokio::test]
async fn an_unanswered_formatter_question_settles_the_call() {
    let (response, described, inquiries) = decide_with_assistant(HashMap::new()).await;

    assert_eq!(described, None);
    assert_eq!(response, ToolCallResponse {
        id: "call-1".into(),
        result: Err(
            "The tool 'example' asked a follow-up question (\"Continue?\") that was routed to a \
             secondary assistant for resolution, but the secondary assistant failed to provide a \
             valid answer. Error: No mock answer for inquiry: call-1.confirm.1. You may retry the \
             tool call or end the turn."
                .into()
        ),
    });
    assert_eq!(inquiries, vec![InquiryResponse::Cancelled {
        id: InquiryId::new("call-1.confirm.1".to_owned()),
        reason: CancellationReason::BackendError,
    }]);
}

/// A prompt backend whose inline selects wait for the test to answer them, and
/// that fails the test if anything else is asked.
struct HeldPromptBackend {
    answers: std::sync::Mutex<std::sync::mpsc::Receiver<char>>,
}

impl PromptBackend for HeldPromptBackend {
    fn inline_select(
        &self,
        _message: &str,
        _options: Vec<jp_inquire::InlineOption>,
        _default: Option<char>,
        _writer: &mut dyn std::io::Write,
    ) -> Result<char, InquireError> {
        self.answers
            .lock()
            .unwrap()
            .recv()
            .map_err(|_| InquireError::OperationCanceled)
    }

    fn inline_reply(
        &self,
        message: &str,
        _initial_text: &str,
        _edit_mode: ReplyEditMode,
        _editor_escape: bool,
        _help: Option<&str>,
        _output: Box<dyn std::io::Write + Send>,
    ) -> Result<ReplyOutcome, InquireError> {
        panic!("unexpected prompt: {message}")
    }

    fn text(
        &self,
        message: &str,
        _default: Option<&str>,
        _writer: &mut dyn std::io::Write,
    ) -> Result<String, InquireError> {
        panic!("unexpected prompt: {message}")
    }

    fn select(
        &self,
        message: &str,
        _options: Vec<String>,
        _default: Option<usize>,
        _writer: &mut dyn std::io::Write,
    ) -> Result<String, InquireError> {
        panic!("unexpected prompt: {message}")
    }

    fn password(
        &self,
        message: &str,
        _writer: &mut dyn std::io::Write,
    ) -> Result<String, InquireError> {
        panic!("unexpected prompt: {message}")
    }
}

/// An executor standing in for the execution service, whose argument formatter
/// asks the tool's `confirm` question and then describes the call as `confirm =
/// <answer>`; running answers `ran`.
struct AskingFormatter {
    tool_id: String,
    tool_name: String,

    /// Whether the call asks before it runs.
    asks: bool,

    /// The answer the formatter was given, once it has one.
    answer: std::sync::Mutex<Option<Value>>,
}

impl AskingFormatter {
    fn new(tool_id: String, tool_name: String, asks: bool) -> Self {
        Self {
            tool_id,
            tool_name,
            asks,
            answer: std::sync::Mutex::new(None),
        }
    }
}

#[async_trait]
impl Executor for AskingFormatter {
    fn formatted_arguments(&self) -> Option<String> {
        let answer = self.answer.lock().unwrap();
        answer.as_ref().map(|answer| format!("confirm = {answer}"))
    }
    fn tool_id(&self) -> &str {
        &self.tool_id
    }
    fn tool_name(&self) -> &str {
        &self.tool_name
    }
    fn arguments(&self) -> Map<String, Value> {
        Map::new()
    }
    fn permission_info(&self) -> Option<PermissionInfo> {
        self.asks.then(|| PermissionInfo {
            tool_id: self.tool_id.clone(),
            tool_name: self.tool_name.clone(),
            tool_source: ToolSource::Builtin { tool: None },
            run_mode: RunMode::Ask,
            arguments: Value::Object(Map::new()),
        })
    }
    fn set_arguments(&self, _args: Value) {}
    async fn prepare(
        &self,
        _render_arguments: bool,
        _cancellation: CancellationToken,
    ) -> ExecutorResult {
        ExecutorResult::NeedsInput {
            question: Question::boolean("confirm", "Continue?").unwrap(),
            source: InquirySource::tool(&self.tool_name),
            accumulated_answers: IndexMap::new(),
        }
    }
    async fn execute(
        &self,
        answers: &IndexMap<String, Value>,
        _cancellation_token: CancellationToken,
        _stderr: Option<jp_mcp::server::StderrSink>,
    ) -> ExecutorResult {
        let mut answer = self.answer.lock().unwrap();
        // The formatter's question was answered, so it describes the call.
        if answer.is_none() {
            *answer = answers.get("confirm").cloned();
            return ExecutorResult::AwaitingAdmission;
        }
        ExecutorResult::Completed(ToolCallResponse {
            id: self.tool_id.clone(),
            result: Ok("ran".into()),
        })
    }
}

/// An assistant that never answers, and keeps the token each question was asked
/// under.
#[derive(Default)]
struct SilentAssistant {
    asked: std::sync::Mutex<Vec<CancellationToken>>,
}

#[async_trait]
impl InquiryBackend for SilentAssistant {
    async fn inquire(
        &self,
        _events: ConversationStream,
        _inquiry_id: &str,
        _tool_name: &str,
        _question: &Question,
        cancellation_token: CancellationToken,
    ) -> Result<Value, InquiryError> {
        self.asked.lock().unwrap().push(cancellation_token.clone());
        cancellation_token.cancelled().await;
        Err(InquiryError::Cancelled)
    }
}

/// Two calls to `example`, where the second call's formatter asks `confirm` of
/// `target` while the first call's approval prompt is open, and the user
/// answers that prompt with "no, for the rest of the turn".
///
/// Returns the responses, the inquiry responses recorded, what reached the
/// terminal, and the assistant the questions went to.
#[expect(
    clippy::too_many_lines,
    reason = "Keep the setup, the ordered steps, and what is read back in one scenario"
)]
async fn a_remembered_no_while_a_later_call_asks(
    target: &str,
) -> (
    IndexMap<String, Review>,
    Vec<InquiryResponse>,
    String,
    Arc<SilentAssistant>,
) {
    let partial: jp_config::conversation::tool::PartialToolConfig = serde_json::from_value(json!({
        "source": "builtin",
        "run": "ask",
        "questions": {"confirm": {"target": target}},
        "style": {
            "parameters": "function_call",
            "inline_results": "off",
            "results_file_link": "off",
        },
    }))
    .unwrap();
    let mut tools = jp_config::AppConfig::new_test().conversation.tools;
    tools.insert(
        "example".into(),
        ToolConfig::from_partial(partial, vec![]).unwrap(),
    );
    let source = TestExecutorSource::new().with_executor("example", |request| {
        if request.id == "call-1" {
            return Box::new(
                MockExecutor::completed(&request.id, &request.name, "ran")
                    .with_arguments(request.arguments.clone())
                    .with_permission_info(PermissionInfo {
                        tool_id: request.id.clone(),
                        tool_name: request.name.clone(),
                        tool_source: ToolSource::Builtin { tool: None },
                        run_mode: RunMode::Ask,
                        arguments: Value::Object(request.arguments),
                    }),
            );
        }
        Box::new(AskingFormatter::new(request.id, request.name, true))
    });
    let mut coordinator = ToolCoordinator::new(tools, Box::new(source));
    let requests: Vec<ToolCallRequest> = ["call-1", "call-2"]
        .into_iter()
        .enumerate()
        .map(|(n, id)| ToolCallRequest {
            id: id.into(),
            name: "example".into(),
            arguments: json!({"n": n}).as_object().unwrap().clone(),
        })
        .collect();

    // The assistant is asked from the conversation as it stands, so the turn
    // holding the calls has to exist.
    let (_workspace, lock) = test_lock();
    let conv = lock.as_mut();
    conv.update_events(|events| {
        events.start_turn(ChatRequest::from("Run them."));
        let mut turn = events.current_turn_mut();
        for request in &requests {
            turn = turn.add_tool_call_request(request.clone());
        }
        turn.build().unwrap();
    });

    let (answer, answers) = std::sync::mpsc::channel();
    let prompts: Arc<dyn PromptBackend> = Arc::new(HeldPromptBackend {
        answers: std::sync::Mutex::new(answers),
    });
    let assistant = Arc::new(SilentAssistant::default());
    let inquiry_backend: Arc<dyn InquiryBackend> = assistant.clone();
    let (printer, _stdout, stderr) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let style = jp_config::AppConfig::new_test().style;
    let mut renderer = ToolRenderer::new(ErrChannel::new(printer.clone()), style.clone());
    let prompter = Arc::new(ToolPrompter::with_backends(
        printer.clone(),
        None,
        Arc::clone(&prompts),
    ));
    let mut turn_state = TurnState::default();
    let mut host = Host {
        prompter: &prompter,
        inquiry_backend: &inquiry_backend,
        conv: &conv,
        turn_state: &mut turn_state,
        renderer: &mut renderer,
        printer: &printer,
        interactive: true,
    };
    for request in requests {
        coordinator.submit(request, &mut host);
    }

    // The second call's question is routed while the first call's approval
    // prompt waits for its answer.
    let asked = |conv: &ConversationMut| {
        conv.events().iter().any(|event| {
            event
                .event
                .as_inquiry_request()
                .is_some_and(|request| request.id.as_str() == "call-2.confirm.1")
        })
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !asked(&conv) {
            let event = coordinator.next_event().await;
            coordinator.handle(event, &mut host);
        }
        // A question routed to the assistant is only in flight once the
        // assistant has it.
        while target == "assistant" && assistant.asked.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the second call's question was never asked");
    answer.send('N').unwrap();

    let router = detached_router();
    let mut turn_coordinator = TurnCoordinator::new(printer.clone(), style, None, None, None);
    let mut interrupt_ui = InterruptUi {
        turn_coordinator: &mut turn_coordinator,
        printer: &printer,
        backend: prompts.as_ref(),
        editor: None,
        edit_mode: ReplyEditMode::default(),
    };
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        coordinator.finish(
            &mut host,
            &router,
            &mut interrupt_ui,
            &mut TurnInterrupts::none(),
        ),
    )
    .await
    .expect("the calls were never settled");

    let inquiries = conv
        .events()
        .iter()
        .filter_map(|event| event.event.as_inquiry_response())
        .cloned()
        .collect();
    printer.flush();
    let terminal = strip_ansi(&stderr.lock());
    (result.reviews, inquiries, terminal, assistant)
}

/// A question the second call's formatter put to the user is withdrawn when a
/// remembered "no" skips that call: it is never shown, and nothing of the call
/// reaches the screen.
#[tokio::test]
async fn a_remembered_no_withdraws_a_later_calls_question_to_the_user() {
    let (reviews, inquiries, terminal, _) = a_remembered_no_while_a_later_call_asks("user").await;

    assert_eq!(
        reviews["call-1"].response.result,
        Ok("Tool skipped by user.".into())
    );
    assert_eq!(
        reviews["call-2"].response.result,
        Ok("Tool skipped by user (remembered).".into())
    );
    assert_eq!(inquiries, vec![InquiryResponse::Cancelled {
        id: InquiryId::new("call-2.confirm.1"),
        reason: CancellationReason::Withdrawn,
    }]);
    assert_eq!(terminal, "Calling tool example(n: 0)\n");
}

/// A question the second call's formatter put to the assistant is withdrawn
/// when a remembered "no" skips that call: the request is cancelled, and
/// recorded as withdrawn rather than as a failure.
#[tokio::test]
async fn a_remembered_no_withdraws_a_later_calls_question_to_the_assistant() {
    let (reviews, inquiries, terminal, assistant) =
        a_remembered_no_while_a_later_call_asks("assistant").await;

    assert_eq!(
        reviews["call-2"].response.result,
        Ok("Tool skipped by user (remembered).".into())
    );
    assert_eq!(inquiries, vec![InquiryResponse::Cancelled {
        id: InquiryId::new("call-2.confirm.1"),
        reason: CancellationReason::Withdrawn,
    }]);
    let asked = assistant.asked.lock().unwrap();
    assert_eq!(asked.len(), 1);
    assert!(
        asked[0].is_cancelled(),
        "the assistant's request was not cancelled"
    );
    assert_eq!(terminal, "Calling tool example(n: 0)\n");
}

/// A prompter whose inline editor submits `submitted`.
fn prompter_submitting(submitted: &str) -> ToolPrompter {
    ToolPrompter::with_prompt_backend(
        Arc::new(Printer::sink()),
        None,
        Arc::new(
            MockPromptBackend::new()
                .with_reply_outcomes([ReplyOutcome::Submit(submitted.to_owned())]),
        ),
        ReplyEditMode::default(),
    )
}

fn offered() -> ToolCallResponse {
    ToolCallResponse {
        id: "call-1".into(),
        result: Ok("alpha\n\nresource".into()),
    }
}

/// Pressing Enter on the result as offered is not an edit: the execution
/// service keeps delivering the tool's full result, including any content that
/// has no text form.
#[test]
fn submitting_the_offered_result_unchanged_is_not_an_edit() {
    let review =
        ToolCoordinator::handle_edit_result(&prompter_submitting("alpha\n\nresource"), offered());

    assert!(!review.edited);
    assert_eq!(review.response, offered());
}

#[test]
fn submitting_changed_text_replaces_the_result() {
    let review = ToolCoordinator::handle_edit_result(&prompter_submitting("alpha only"), offered());

    assert!(review.edited);
    assert_eq!(review.response, ToolCallResponse {
        id: "call-1".into(),
        result: Ok("alpha only".into()),
    });
}
