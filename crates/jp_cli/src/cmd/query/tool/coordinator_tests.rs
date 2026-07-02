use async_trait::async_trait;
use camino::Utf8PathBuf;
use jp_config::conversation::tool::{ToolConfig, ToolSource, style::PartialDisplayStyleConfig};
use jp_inquire::{ReplyOutcome, prompt::MockPromptBackend};
use jp_llm::tool::executor::MockExecutor;
use jp_printer::{ErrChannel, OutputFormat, Printer};
use schematic::Config as _;

use super::{super::executor::TerminalExecutorSource, *};
use crate::render::tool::ToolRenderer;

fn empty_executor_source() -> Box<dyn jp_llm::tool::executor::ExecutorSource> {
    Box::new(TerminalExecutorSource::new(
        jp_llm::tool::builtin::BuiltinExecutors::new(),
        &[],
        std::sync::Arc::new(crate::access::approvals::ApprovalStore::default()),
        jp_llm::tool::InvocationContext::default(),
    ))
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
            },
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
            },
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

#[tokio::test]
async fn test_pre_render_for_prompt_function_call_fires_before_approval() {
    // Regression test for the bug where `fs_delete_file`-style tools
    // (built-in parameter style + `run = "ask"`) showed the permission
    // prompt without first rendering the arguments. `FormatMode::Ask`
    // exists to defer side-effecting custom formatters; it should not
    // suppress rendering for the pure built-in styles.
    let tool_config = ToolConfig::from_partial(
        jp_config::conversation::tool::PartialToolConfig {
            source: Some(ToolSource::Builtin { tool: None }),
            style: Some(PartialDisplayStyleConfig {
                parameters: Some(ParametersStyle::FunctionCall),
                ..Default::default()
            }),
            ..Default::default()
        },
        vec![],
    )
    .expect("valid tool config");

    let mut tools_config = jp_config::AppConfig::new_test().conversation.tools;
    tools_config.insert("fs_delete_file".to_string(), tool_config);

    let coordinator = ToolCoordinator::new(tools_config, empty_executor_source());

    // Sanity-check the precondition: with no explicit `format` and the
    // default `run = "ask"`, the format mode derives to `Ask`. The bug
    // was that this gated rendering even for non-Custom styles.
    assert_eq!(coordinator.format_mode("fs_delete_file"), FormatMode::Ask);

    let (printer, _stdout, stderr) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let style_config = jp_config::AppConfig::new_test().style;
    let tool_renderer = ToolRenderer::new(
        ErrChannel::new(printer.clone()),
        style_config,
        Utf8PathBuf::from("/tmp"),
        false,
        jp_llm::tool::InvocationContext::default(),
    );

    let mut args = Map::new();
    args.insert("path".into(), Value::String("src/foo.rs".into()));

    let result = coordinator
        .pre_render_for_prompt("fs_delete_file", &args, &tool_renderer)
        .await;

    // Non-Custom styles should always pre-render. `content` is `None`
    // because only Custom formatters produce persistable rendered content.
    assert!(
        matches!(result, Ok(Some(None))),
        "pre-render should fire for FunctionCall style, got: {result:?}"
    );

    printer.flush();
    let output = stderr.lock();
    assert!(
        output.contains("fs_delete_file"),
        "stderr should contain tool name; got: {output:?}"
    );
    assert!(
        output.contains("src/foo.rs"),
        "stderr should contain the rendered argument; got: {output:?}"
    );
}

#[tokio::test]
async fn test_pre_render_for_prompt_custom_ask_defers_rendering() {
    // Counterpart to the test above: Custom formatters with the default
    // `FormatMode::Ask` should still defer rendering until after approval,
    // because the formatter is a user-controlled shell command.
    use jp_config::conversation::tool::CommandConfigOrString;

    let tool_config = ToolConfig::from_partial(
        jp_config::conversation::tool::PartialToolConfig {
            source: Some(ToolSource::Builtin { tool: None }),
            style: Some(PartialDisplayStyleConfig {
                parameters: Some(ParametersStyle::Custom(CommandConfigOrString::String(
                    "echo SHOULD-NOT-RUN".into(),
                ))),
                ..Default::default()
            }),
            ..Default::default()
        },
        vec![],
    )
    .expect("valid tool config");

    let mut tools_config = jp_config::AppConfig::new_test().conversation.tools;
    tools_config.insert("custom_tool".to_string(), tool_config);

    let coordinator = ToolCoordinator::new(tools_config, empty_executor_source());
    assert_eq!(coordinator.format_mode("custom_tool"), FormatMode::Ask);

    let (printer, _stdout, stderr) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let style_config = jp_config::AppConfig::new_test().style;
    let tool_renderer = ToolRenderer::new(
        ErrChannel::new(printer.clone()),
        style_config,
        Utf8PathBuf::from("/tmp"),
        false,
        jp_llm::tool::InvocationContext::default(),
    );

    let result = coordinator
        .pre_render_for_prompt("custom_tool", &Map::new(), &tool_renderer)
        .await;

    assert!(
        matches!(result, Ok(None)),
        "Custom + format=ask should defer rendering, got: {result:?}"
    );

    printer.flush();
    let output = stderr.lock();
    assert!(
        !output.contains("SHOULD-NOT-RUN"),
        "custom formatter must not have run; got: {output:?}"
    );
}

/// Minimal `Executor` whose `set_arguments` actually mutates state.
///
/// `MockExecutor::set_arguments` is a no-op, which is fine for tests that don't
/// exercise the prompt-edit path but useless for verifying the
/// pre-render-invalidation logic in `resolve_tool_call_decision`.
struct EditableExecutor {
    tool_id: String,
    tool_name: String,
    arguments: Map<String, Value>,
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
    fn arguments(&self) -> &Map<String, Value> {
        &self.arguments
    }
    fn permission_info(&self) -> Option<PermissionInfo> {
        Some(self.permission_info.clone())
    }
    fn set_arguments(&mut self, args: Value) {
        if let Value::Object(map) = args {
            self.arguments = map;
        }
    }
    async fn execute(
        &self,
        _answers: &IndexMap<String, Value>,
        _mcp_client: &jp_mcp::Client,
        _root: &camino::Utf8Path,
        _cancellation_token: tokio_util::sync::CancellationToken,
    ) -> ExecutorResult {
        unreachable!("resolve_tool_call_decision does not invoke execute()")
    }
}

#[tokio::test]
async fn test_resolve_tool_call_decision_invalidates_prerender_on_edit() {
    // Regression: when the user picks `e` (edit) at the approval prompt
    // and changes arguments, the previously-rendered call would otherwise
    // remain as the rendered-of-record while the executor runs the
    // post-edit args. For built-in styles under the default `run = ask`
    // this now affects every tool, not just the rare `format = unattended`
    // case the original caveat documented. Verify the pre-render gets
    // invalidated and step 3 re-renders with the args that will execute.
    let tool_config = ToolConfig::from_partial(
        jp_config::conversation::tool::PartialToolConfig {
            source: Some(ToolSource::Builtin { tool: None }),
            run: Some(RunMode::Ask),
            style: Some(PartialDisplayStyleConfig {
                parameters: Some(ParametersStyle::FunctionCall),
                ..Default::default()
            }),
            ..Default::default()
        },
        vec![],
    )
    .expect("valid tool config");

    let mut tools_config = jp_config::AppConfig::new_test().conversation.tools;
    tools_config.insert("fs_delete_file".to_string(), tool_config);

    let mut coordinator = ToolCoordinator::new(tools_config, empty_executor_source());

    let (printer, _stdout, stderr) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let style_config = jp_config::AppConfig::new_test().style;
    let tool_renderer = ToolRenderer::new(
        ErrChannel::new(printer.clone()),
        style_config,
        Utf8PathBuf::from("/tmp"),
        false,
        jp_llm::tool::InvocationContext::default(),
    );

    let mut pre_edit_args = Map::new();
    pre_edit_args.insert("path".into(), Value::String("src/foo.rs".into()));
    let executor: Box<dyn Executor> = Box::new(EditableExecutor {
        tool_id: "call_1".into(),
        tool_name: "fs_delete_file".into(),
        arguments: pre_edit_args.clone(),
        permission_info: PermissionInfo {
            tool_id: "call_1".into(),
            tool_name: "fs_delete_file".into(),
            tool_source: ToolSource::Builtin { tool: None },
            run_mode: RunMode::Ask,
            arguments: Value::Object(pre_edit_args),
        },
    });

    // The prompt backend picks `e` (edit arguments) and supplies the post-edit
    // JSON through the inline reply widget.
    let post_edit = serde_json::json!({"path": "src/bar.rs"});
    let prompt_backend = MockPromptBackend::new()
        .with_inline_responses(['e'])
        .with_reply_outcomes([ReplyOutcome::Submit(
            serde_json::to_string(&post_edit).unwrap(),
        )]);
    let prompter = ToolPrompter::with_backends(printer.clone(), None, Arc::new(prompt_backend));

    let mut turn_state = TurnState::default();

    let decision = coordinator
        .resolve_tool_call_decision(executor, &prompter, true, &mut turn_state, &tool_renderer)
        .await;

    match decision {
        ToolCallDecision::Approved { executor, .. } => {
            assert_eq!(
                executor.arguments().get("path"),
                Some(&Value::String("src/bar.rs".into())),
                "executor must carry the post-edit args"
            );
        }
        ToolCallDecision::Skipped(_) => panic!("Expected Approved, got Skipped"),
        ToolCallDecision::Failed(_) => panic!("Expected Approved, got Failed"),
    }

    printer.flush();
    let output = stderr.lock();
    assert!(
        output.contains("src/foo.rs"),
        "pre-render with pre-edit args must be in scrollback; got: {output:?}"
    );
    assert!(
        output.contains("src/bar.rs"),
        "post-approval re-render must reflect post-edit args; got: {output:?}"
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
            Box::new(MockExecutor::completed("call_1", "my_tool", "ok")),
        )
        .expect("run decision returns the executor");

    assert_eq!(
        turn_state
            .remembered_permission_decisions
            .get(&PermissionCacheKey::new("my_tool")),
        Some(&true),
    );
    assert!(
        turn_state.remembered_tool_answers.is_empty(),
        "a permission decision must not leak into the tool-answer cache"
    );

    // A later call for the same tool reuses the decision without prompting.
    let executor =
        Box::new(MockExecutor::completed("call_2", "my_tool", "ok").with_permission_info(info));
    let decision = coordinator.decide_permission(executor, true, &turn_state);
    assert!(
        matches!(decision, PermissionDecision::Approved(_)),
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
fn test_pending_prompt_question_variant() {
    let pending = PendingPrompt::Question {
        index: 0,
        question: jp_tool::Question::text("q1", "Test question").unwrap(),
        inquiry_id: InquiryId::new("iq"),
    };

    // Verify we can match and extract fields
    let PendingPrompt::Question {
        index, question: q, ..
    } = pending
    else {
        panic!("Expected Question variant");
    };
    assert_eq!(index, 0);
    assert_eq!(q.id, "q1");
}

#[test]
fn test_pending_prompt_result_mode_variant() {
    let response = ToolCallResponse {
        id: "call_1".to_string(),
        result: Ok("output".to_string()),
    };

    let pending = PendingPrompt::ResultMode {
        index: 1,
        tool_id: "call_1".to_string(),
        tool_name: "my_tool".to_string(),
        response: response.clone(),
        result_mode: ResultMode::Ask,
    };

    // Verify we can match and extract fields
    let PendingPrompt::ResultMode {
        index,
        tool_id,
        tool_name,
        response: r,
        result_mode,
    } = pending
    else {
        panic!("Expected ResultMode variant");
    };
    assert_eq!(index, 1);
    assert_eq!(tool_id, "call_1");
    assert_eq!(tool_name, "my_tool");
    assert_eq!(r.id, "call_1");
    assert_eq!(result_mode, ResultMode::Ask);
}

#[test]
fn test_pending_prompt_queue_fifo_order() {
    let mut queue: VecDeque<PendingPrompt> = VecDeque::new();

    // Add a question prompt
    queue.push_back(PendingPrompt::Question {
        index: 0,
        question: jp_tool::Question::text("q1", "First").unwrap(),
        inquiry_id: InquiryId::new("iq"),
    });

    // Add a result mode prompt
    queue.push_back(PendingPrompt::ResultMode {
        index: 1,
        tool_id: "call_1".to_string(),
        tool_name: "tool_a".to_string(),
        response: ToolCallResponse {
            id: "call_1".to_string(),
            result: Ok("output".to_string()),
        },
        result_mode: ResultMode::Edit,
    });

    // Add another question prompt
    queue.push_back(PendingPrompt::Question {
        index: 2,
        question: jp_tool::Question::boolean("q2", "Third").unwrap(),
        inquiry_id: InquiryId::new("iq"),
    });

    // Verify FIFO order
    assert_eq!(queue.len(), 3);

    // First: Question at index 0
    let PendingPrompt::Question {
        index, question, ..
    } = queue.pop_front().unwrap()
    else {
        panic!("Expected Question");
    };
    assert_eq!(index, 0);
    assert_eq!(question.id, "q1");

    // Second: ResultMode at index 1
    let PendingPrompt::ResultMode { index, tool_id, .. } = queue.pop_front().unwrap() else {
        panic!("Expected ResultMode");
    };
    assert_eq!(index, 1);
    assert_eq!(tool_id, "call_1");

    // Third: Question at index 2
    let PendingPrompt::Question {
        index, question, ..
    } = queue.pop_front().unwrap()
    else {
        panic!("Expected Question");
    };
    assert_eq!(index, 2);
    assert_eq!(question.id, "q2");

    assert!(queue.is_empty());
}

#[test]
fn test_pending_prompt_mixed_types_interleaved() {
    // This tests the real-world scenario where prompts arrive interleaved:
    // Tool 0 needs input, Tool 1 completes with Ask mode, Tool 2 needs input
    let mut queue: VecDeque<PendingPrompt> = VecDeque::new();

    // Simulate: while prompt_active, queue these in arrival order
    queue.push_back(PendingPrompt::Question {
        index: 0,
        question: jp_tool::Question::text("branch", "Which branch?").unwrap(),
        inquiry_id: InquiryId::new("iq"),
    });

    queue.push_back(PendingPrompt::ResultMode {
        index: 1,
        tool_id: "call_tool1".to_string(),
        tool_name: "fs_read".to_string(),
        response: ToolCallResponse {
            id: "call_tool1".to_string(),
            result: Ok("file contents".to_string()),
        },
        result_mode: ResultMode::Ask,
    });

    queue.push_back(PendingPrompt::Question {
        index: 2,
        question: jp_tool::Question::boolean("confirm", "Confirm action?")
            .unwrap()
            .with_default(serde_json::json!(true)),
        inquiry_id: InquiryId::new("iq"),
    });

    // All three should be queued
    assert_eq!(queue.len(), 3);

    // Verify the types alternate as expected
    assert!(matches!(queue[0], PendingPrompt::Question { .. }));
    assert!(matches!(queue[1], PendingPrompt::ResultMode { .. }));
    assert!(matches!(queue[2], PendingPrompt::Question { .. }));
}
