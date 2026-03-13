use super::{super::executor::TerminalExecutorSource, *};

fn empty_executor_source() -> Box<dyn jp_llm::tool::executor::ExecutorSource> {
    Box::new(TerminalExecutorSource::new(
        jp_llm::tool::builtin::BuiltinExecutors::new(),
        &[],
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

#[test]
fn test_static_answers_for_tool_empty() {
    let coordinator = ToolCoordinator::new(
        jp_config::AppConfig::new_test().conversation.tools,
        empty_executor_source(),
    );
    // Non-existent tool returns empty map
    assert!(
        coordinator
            .static_answers_for_tool("nonexistent_tool")
            .is_empty()
    );
}

#[test]
fn test_static_answers_for_tool_collects_all_answers() {
    use jp_config::conversation::tool::{QuestionTarget, ToolConfig, ToolSource};
    use schematic::Config as _;

    // Create a tool config with multiple questions, some with answers
    let tool_config = ToolConfig::from_partial(
        jp_config::conversation::tool::PartialToolConfig {
            source: Some(ToolSource::Builtin { tool: None }),
            questions: indexmap::indexmap! {
                "q1".to_string() => jp_config::conversation::tool::PartialQuestionConfig {
                    target: Some(QuestionTarget::User),
                    answer: Some(serde_json::json!("answer1")),
                },
                "q2".to_string() => jp_config::conversation::tool::PartialQuestionConfig {
                    target: Some(QuestionTarget::User),
                    answer: Some(serde_json::json!(42)),
                },
                "q3".to_string() => jp_config::conversation::tool::PartialQuestionConfig {
                    target: Some(QuestionTarget::User),
                    answer: None, // No static answer
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
    let answers = coordinator.static_answers_for_tool("my_tool");

    // Should have 2 answers (q1 and q2, but not q3)
    assert_eq!(answers.len(), 2);
    assert_eq!(answers.get("q1"), Some(&serde_json::json!("answer1")));
    assert_eq!(answers.get("q2"), Some(&serde_json::json!(42)));
    assert!(answers.get("q3").is_none());
}

#[test]
fn test_pending_prompt_question_variant() {
    let pending = PendingPrompt::Question {
        index: 0,
        question: jp_tool::Question {
            id: "q1".to_string(),
            text: "Test question".to_string(),
            answer_type: jp_tool::AnswerType::Text,
            default: None,
        },
    };

    // Verify we can match and extract fields
    let PendingPrompt::Question { index, question: q } = pending else {
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
        question: jp_tool::Question {
            id: "q1".to_string(),
            text: "First".to_string(),
            answer_type: jp_tool::AnswerType::Text,
            default: None,
        },
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
        question: jp_tool::Question {
            id: "q2".to_string(),
            text: "Third".to_string(),
            answer_type: jp_tool::AnswerType::Boolean,
            default: None,
        },
    });

    // Verify FIFO order
    assert_eq!(queue.len(), 3);

    // First: Question at index 0
    let PendingPrompt::Question { index, question } = queue.pop_front().unwrap() else {
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
    let PendingPrompt::Question { index, question } = queue.pop_front().unwrap() else {
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
        question: jp_tool::Question {
            id: "branch".to_string(),
            text: "Which branch?".to_string(),
            answer_type: jp_tool::AnswerType::Text,
            default: None,
        },
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
        question: jp_tool::Question {
            id: "confirm".to_string(),
            text: "Confirm action?".to_string(),
            answer_type: jp_tool::AnswerType::Boolean,
            default: Some(serde_json::json!(true)),
        },
    });

    // All three should be queued
    assert_eq!(queue.len(), 3);

    // Verify the types alternate as expected
    assert!(matches!(queue[0], PendingPrompt::Question { .. }));
    assert!(matches!(queue[1], PendingPrompt::ResultMode { .. }));
    assert!(matches!(queue[2], PendingPrompt::Question { .. }));
}
