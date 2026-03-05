use jp_editor::MockEditorBackend;
use jp_inquire::prompt::MockPromptBackend;
use jp_printer::OutputFormat;

use super::*;

/// Creates a test prompter with a mock editor backend (real prompt backend).
fn prompter_with_mock_editor(mock: MockEditorBackend) -> ToolPrompter {
    let (printer, _, _) = Printer::memory(OutputFormat::TextPretty);
    ToolPrompter::with_editor_backend(Arc::new(printer), mock)
}

/// Creates a test prompter without an editor.
fn prompter_without_editor() -> ToolPrompter {
    let (printer, _, _) = Printer::memory(OutputFormat::TextPretty);
    ToolPrompter::with_backends(Arc::new(printer), None, Arc::new(MockPromptBackend::new()))
}

/// Creates a test prompter with both mock editor and mock prompt backends.
fn prompter_with_mocks(editor: MockEditorBackend, prompt: MockPromptBackend) -> ToolPrompter {
    let (printer, _, _) = Printer::memory(OutputFormat::TextPretty);
    ToolPrompter::with_backends(Arc::new(printer), Some(Arc::new(editor)), Arc::new(prompt))
}

/// Creates a test prompter with mock prompt backend but no editor.
fn prompter_with_mock_prompt(prompt: MockPromptBackend) -> ToolPrompter {
    let (printer, _, _) = Printer::memory(OutputFormat::TextPretty);
    ToolPrompter::with_backends(Arc::new(printer), None, Arc::new(prompt))
}

#[test]
fn test_permission_result_variants() {
    let run = PermissionResult::Run {
        arguments: serde_json::json!({"key": "value"}),
        persist: false,
    };
    assert!(matches!(run, PermissionResult::Run { .. }));

    let skip = PermissionResult::Skip {
        reason: None,
        persist: false,
    };
    assert!(matches!(skip, PermissionResult::Skip { .. }));

    let skip_with_reason = PermissionResult::Skip {
        reason: Some("User cancelled".to_string()),
        persist: false,
    };
    assert!(matches!(skip_with_reason, PermissionResult::Skip {
        reason: Some(_),
        ..
    }));
}

#[test]
fn test_mock_editor_returns_configured_response() {
    let mock = MockEditorBackend::always("edited content");
    let result = mock.edit("original").unwrap();
    assert_eq!(result, "edited content");
}

#[test]
fn test_mock_editor_returns_responses_in_sequence() {
    let mock = MockEditorBackend::with_responses(["first", "second", "third"]);

    assert_eq!(mock.edit("").unwrap(), "first");
    assert_eq!(mock.edit("").unwrap(), "second");
    assert_eq!(mock.edit("").unwrap(), "third");
    // Exhausted, returns empty
    assert_eq!(mock.edit("").unwrap(), "");
}

#[test]
fn test_mock_editor_empty() {
    let mock = MockEditorBackend::empty();
    let result = mock.edit("some content").unwrap();
    assert_eq!(result, "");
}

#[test]
fn test_mock_editor_json() {
    let value = serde_json::json!({"key": "value"});
    let mock = MockEditorBackend::json(&value);
    let result = mock.edit("").unwrap();
    let parsed: Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed, value);
}

#[test]
fn test_edit_arguments_returns_modified_json() {
    let original = serde_json::json!({"key": "original"});
    let modified = serde_json::json!({"key": "modified"});

    let mock = MockEditorBackend::json(&modified);
    let prompter = prompter_with_mock_editor(mock);

    let result = prompter.try_edit_arguments(&original).unwrap();

    match result {
        EditResult::Edited(v) => assert_eq!(v, modified),
        other => panic!("Expected Edited, got {other:?}"),
    }
}

#[test]
fn test_edit_arguments_empty_returns_emptied() {
    let original = serde_json::json!({"key": "value"});

    let mock = MockEditorBackend::empty();
    let prompter = prompter_with_mock_editor(mock);

    let result = prompter.try_edit_arguments(&original).unwrap();

    assert!(matches!(result, EditResult::Emptied));
}

#[test]
fn test_edit_arguments_whitespace_only_returns_emptied() {
    let original = serde_json::json!({"key": "value"});

    let mock = MockEditorBackend::always("   \n\t  ");
    let prompter = prompter_with_mock_editor(mock);

    let result = prompter.try_edit_arguments(&original).unwrap();

    assert!(matches!(result, EditResult::Emptied));
}

#[test]
fn test_edit_arguments_no_editor_returns_error() {
    let prompter = prompter_without_editor();
    let original = serde_json::json!({"key": "value"});

    let result = prompter.try_edit_arguments(&original);

    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("No editor configured")
    );
}

#[test]
#[allow(clippy::approx_constant)]
fn test_edit_arguments_preserves_complex_json() {
    let complex = serde_json::json!({
        "string": "value",
        "number": 42,
        "float": 3.1415,
        "boolean": true,
        "null": null,
        "array": [1, 2, 3],
        "nested": {
            "deep": {
                "value": "found"
            }
        }
    });

    let mock = MockEditorBackend::json(&complex);
    let prompter = prompter_with_mock_editor(mock);

    let result = prompter.try_edit_arguments(&serde_json::json!({})).unwrap();

    match result {
        EditResult::Edited(v) => assert_eq!(v, complex),
        other => panic!("Expected Edited, got {other:?}"),
    }
}

#[test]
fn test_edit_result_returns_modified_content() {
    let mock = MockEditorBackend::always("edited result");
    let prompter = prompter_with_mock_editor(mock);

    let result = prompter.edit_result("original result").unwrap();

    assert_eq!(result, Some("edited result".to_string()));
}

#[test]
fn test_edit_result_empty_returns_none() {
    let mock = MockEditorBackend::empty();
    let prompter = prompter_with_mock_editor(mock);

    let result = prompter.edit_result("original result").unwrap();

    assert_eq!(result, None);
}

#[test]
fn test_edit_result_whitespace_only_returns_none() {
    let mock = MockEditorBackend::always("   \n\t  ");
    let prompter = prompter_with_mock_editor(mock);

    let result = prompter.edit_result("original result").unwrap();

    assert_eq!(result, None);
}

#[test]
fn test_edit_result_no_editor_returns_error() {
    let prompter = prompter_without_editor();

    let result = prompter.edit_result("some result");

    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("No editor configured")
    );
}

#[test]
fn test_edit_result_preserves_multiline_content() {
    let multiline = "line 1\nline 2\nline 3";
    let mock = MockEditorBackend::always(multiline);
    let prompter = prompter_with_mock_editor(mock);

    let result = prompter.edit_result("original").unwrap();

    assert_eq!(result, Some(multiline.to_string()));
}

#[test]
fn test_has_editor_with_editor() {
    let mock = MockEditorBackend::empty();
    let prompter = prompter_with_mock_editor(mock);

    assert!(prompter.has_editor());
}

#[test]
fn test_has_editor_without_editor() {
    let prompter = prompter_without_editor();

    assert!(!prompter.has_editor());
}

/// Creates a `PermissionInfo` for testing.
fn make_permission_info(run_mode: RunMode, arguments: Value) -> PermissionInfo {
    PermissionInfo {
        tool_id: "call_123".to_string(),
        tool_name: "test_tool".to_string(),
        tool_source: ToolSource::Builtin { tool: None },
        run_mode,
        arguments,
    }
}

#[tokio::test]
async fn test_prompt_permission_unattended_returns_original_args() {
    let prompter = prompter_without_editor();
    let mcp_client = jp_mcp::Client::default();
    let original = serde_json::json!({"key": "original"});

    let info = make_permission_info(RunMode::Unattended, original.clone());
    let result = prompter
        .prompt_permission(&info, &mcp_client)
        .await
        .unwrap();

    match result {
        PermissionResult::Run { arguments, .. } => assert_eq!(arguments, original),
        other @ PermissionResult::Skip { .. } => panic!("Expected Run, got {other:?}"),
    }
}

#[tokio::test]
async fn test_prompt_permission_skip_returns_skip() {
    let prompter = prompter_without_editor();
    let mcp_client = jp_mcp::Client::default();

    let info = make_permission_info(RunMode::Skip, serde_json::json!({}));
    let result = prompter
        .prompt_permission(&info, &mcp_client)
        .await
        .unwrap();

    assert!(matches!(result, PermissionResult::Skip {
        reason: None,
        ..
    }));
}

#[tokio::test]
async fn test_prompt_permission_edit_returns_modified_args() {
    let original = serde_json::json!({"key": "original"});
    let modified = serde_json::json!({"key": "modified", "extra": true});

    let mock = MockEditorBackend::json(&modified);
    let prompter = prompter_with_mock_editor(mock);
    let mcp_client = jp_mcp::Client::default();

    let info = make_permission_info(RunMode::Edit, original);
    let result = prompter
        .prompt_permission(&info, &mcp_client)
        .await
        .unwrap();

    match result {
        PermissionResult::Run { arguments, .. } => assert_eq!(arguments, modified),
        other @ PermissionResult::Skip { .. } => {
            panic!("Expected Run with modified args, got {other:?}")
        }
    }
}

#[tokio::test]
async fn test_prompt_permission_edit_can_add_new_fields() {
    let original = serde_json::json!({"existing": "value"});
    let modified = serde_json::json!({
        "existing": "value",
        "new_field": "added",
        "another": 42
    });

    let mock = MockEditorBackend::json(&modified);
    let prompter = prompter_with_mock_editor(mock);
    let mcp_client = jp_mcp::Client::default();

    let info = make_permission_info(RunMode::Edit, original);
    let result = prompter
        .prompt_permission(&info, &mcp_client)
        .await
        .unwrap();

    match result {
        PermissionResult::Run { arguments, .. } => {
            assert_eq!(arguments["existing"], "value");
            assert_eq!(arguments["new_field"], "added");
            assert_eq!(arguments["another"], 42);
        }
        other @ PermissionResult::Skip { .. } => panic!("Expected Run, got {other:?}"),
    }
}

#[tokio::test]
async fn test_prompt_permission_edit_can_remove_fields() {
    let original = serde_json::json!({"keep": "this", "remove": "that"});
    let modified = serde_json::json!({"keep": "this"});

    let mock = MockEditorBackend::json(&modified);
    let prompter = prompter_with_mock_editor(mock);
    let mcp_client = jp_mcp::Client::default();

    let info = make_permission_info(RunMode::Edit, original);
    let result = prompter
        .prompt_permission(&info, &mcp_client)
        .await
        .unwrap();

    match result {
        PermissionResult::Run { arguments, .. } => {
            assert_eq!(arguments, modified);
            assert!(arguments.get("remove").is_none());
        }
        other @ PermissionResult::Skip { .. } => panic!("Expected Run, got {other:?}"),
    }
}

#[tokio::test]
async fn test_prompt_permission_edit_can_change_types() {
    let original = serde_json::json!({"value": "string"});
    let modified = serde_json::json!({"value": 123});

    let mock = MockEditorBackend::json(&modified);
    let prompter = prompter_with_mock_editor(mock);
    let mcp_client = jp_mcp::Client::default();

    let info = make_permission_info(RunMode::Edit, original);
    let result = prompter
        .prompt_permission(&info, &mcp_client)
        .await
        .unwrap();

    match result {
        PermissionResult::Run { arguments, .. } => {
            assert_eq!(arguments["value"], 123);
        }
        other @ PermissionResult::Skip { .. } => panic!("Expected Run, got {other:?}"),
    }
}

#[test]
fn test_edit_result_modifies_tool_output() {
    let original_result = "Original tool output:\n- item 1\n- item 2";
    let edited_result = "Edited output:\n- modified item";

    let mock = MockEditorBackend::always(edited_result);
    let prompter = prompter_with_mock_editor(mock);

    let result = prompter.edit_result(original_result).unwrap();

    assert_eq!(result, Some(edited_result.to_string()));
}

#[test]
fn test_edit_result_can_completely_replace_content() {
    let original = "This is the original content from the tool";
    let replacement = "Completely different content";

    let mock = MockEditorBackend::always(replacement);
    let prompter = prompter_with_mock_editor(mock);

    let result = prompter.edit_result(original).unwrap();

    assert_eq!(result, Some(replacement.to_string()));
}

#[test]
fn test_edit_result_empty_signals_fallback() {
    // When user empties the content, edit_result returns None
    // This signals the caller to fall back to Ask mode
    let mock = MockEditorBackend::empty();
    let prompter = prompter_with_mock_editor(mock);

    let result = prompter.edit_result("some content").unwrap();

    assert!(
        result.is_none(),
        "Empty content should return None to signal fallback"
    );
}

#[test]
fn test_edit_result_preserves_special_characters() {
    let content_with_special = "Result with special chars: <>&\"'\nAnd unicode: 你好 🎉";

    let mock = MockEditorBackend::always(content_with_special);
    let prompter = prompter_with_mock_editor(mock);

    let result = prompter.edit_result("original").unwrap();

    assert_eq!(result, Some(content_with_special.to_string()));
}

#[test]
fn test_edit_result_preserves_json_in_result() {
    // Tool results often contain JSON - make sure it's preserved
    let json_result = r#"{"status": "success", "data": [1, 2, 3]}"#;

    let mock = MockEditorBackend::always(json_result);
    let prompter = prompter_with_mock_editor(mock);

    let result = prompter.edit_result("{}").unwrap();

    assert_eq!(result, Some(json_result.to_string()));
    // Verify it's valid JSON
    let parsed: serde_json::Value = serde_json::from_str(result.as_ref().unwrap()).unwrap();
    assert_eq!(parsed["status"], "success");
}

#[tokio::test]
async fn test_run_mode_edit_empty_falls_back_to_ask_then_approves() {
    // Flow: Edit → empty → Ask → 'y' → Run with original args
    let original = serde_json::json!({"key": "original"});

    let editor = MockEditorBackend::empty();
    let prompt = MockPromptBackend::new().with_inline_responses(['y']);
    let prompter = prompter_with_mocks(editor, prompt);
    let mcp_client = jp_mcp::Client::default();

    let info = make_permission_info(RunMode::Edit, original.clone());
    let result = prompter
        .prompt_permission(&info, &mcp_client)
        .await
        .unwrap();

    match result {
        PermissionResult::Run { arguments, .. } => assert_eq!(arguments, original),
        PermissionResult::Skip { .. } => panic!("Expected Run, got Skip"),
    }
}

#[tokio::test]
async fn test_run_mode_edit_empty_falls_back_to_ask_then_skips() {
    // Flow: Edit → empty → Ask → 'n' → Skip
    let original = serde_json::json!({"key": "original"});

    let editor = MockEditorBackend::empty();
    let prompt = MockPromptBackend::new().with_inline_responses(['n']);
    let prompter = prompter_with_mocks(editor, prompt);
    let mcp_client = jp_mcp::Client::default();

    let info = make_permission_info(RunMode::Edit, original);
    let result = prompter
        .prompt_permission(&info, &mcp_client)
        .await
        .unwrap();

    assert!(matches!(result, PermissionResult::Skip {
        reason: None,
        ..
    }));
}

#[tokio::test]
async fn test_run_mode_ask_approves() {
    // Flow: Ask → 'y' → Run with original args
    let original = serde_json::json!({"key": "original"});

    let prompt = MockPromptBackend::new().with_inline_responses(['y']);
    let prompter = prompter_with_mock_prompt(prompt);
    let mcp_client = jp_mcp::Client::default();

    let info = make_permission_info(RunMode::Ask, original.clone());
    let result = prompter
        .prompt_permission(&info, &mcp_client)
        .await
        .unwrap();

    match result {
        PermissionResult::Run { arguments, .. } => assert_eq!(arguments, original),
        PermissionResult::Skip { .. } => panic!("Expected Run, got Skip"),
    }
}

#[tokio::test]
async fn test_run_mode_ask_skips() {
    // Flow: Ask → 'n' → Skip
    let prompt = MockPromptBackend::new().with_inline_responses(['n']);
    let prompter = prompter_with_mock_prompt(prompt);
    let mcp_client = jp_mcp::Client::default();

    let info = make_permission_info(RunMode::Ask, serde_json::json!({}));
    let result = prompter
        .prompt_permission(&info, &mcp_client)
        .await
        .unwrap();

    assert!(matches!(result, PermissionResult::Skip {
        reason: None,
        ..
    }));
}

#[tokio::test]
async fn test_run_mode_ask_edit_option_modifies_args() {
    // Flow: Ask → 'e' → Edit (valid JSON) → Run with modified args
    let original = serde_json::json!({"key": "original"});
    let modified = serde_json::json!({"key": "modified"});

    let editor = MockEditorBackend::json(&modified);
    let prompt = MockPromptBackend::new().with_inline_responses(['e']);
    let prompter = prompter_with_mocks(editor, prompt);
    let mcp_client = jp_mcp::Client::default();

    let info = make_permission_info(RunMode::Ask, original);
    let result = prompter
        .prompt_permission(&info, &mcp_client)
        .await
        .unwrap();

    match result {
        PermissionResult::Run { arguments, .. } => assert_eq!(arguments, modified),
        PermissionResult::Skip { .. } => panic!("Expected Run with modified args, got Skip"),
    }
}

#[tokio::test]
async fn test_run_mode_ask_edit_empty_loops_back_then_approves() {
    // Flow: Ask → 'e' → empty → Ask → 'y' → Run with original
    let original = serde_json::json!({"key": "original"});

    let editor = MockEditorBackend::empty();
    // First 'e' to edit, then 'y' to approve after empty
    let prompt = MockPromptBackend::new().with_inline_responses(['e', 'y']);
    let prompter = prompter_with_mocks(editor, prompt);
    let mcp_client = jp_mcp::Client::default();

    let info = make_permission_info(RunMode::Ask, original.clone());
    let result = prompter
        .prompt_permission(&info, &mcp_client)
        .await
        .unwrap();

    match result {
        PermissionResult::Run { arguments, .. } => assert_eq!(arguments, original),
        PermissionResult::Skip { .. } => panic!("Expected Run, got Skip"),
    }
}

#[tokio::test]
async fn test_run_mode_ask_edit_invalid_json_retry_then_valid() {
    // Flow: Ask → 'e' → invalid JSON → 'y' (retry) → valid JSON → Run
    let modified = serde_json::json!({"key": "modified"});

    // First edit returns invalid JSON, second returns valid
    let editor = MockEditorBackend::with_responses([
        "{ invalid json }".to_string(),
        serde_json::to_string_pretty(&modified).unwrap(),
    ]);
    // 'e' to edit, 'y' to retry after invalid JSON
    let prompt = MockPromptBackend::new().with_inline_responses(['e', 'y']);
    let prompter = prompter_with_mocks(editor, prompt);
    let mcp_client = jp_mcp::Client::default();

    let info = make_permission_info(RunMode::Ask, serde_json::json!({}));
    let result = prompter
        .prompt_permission(&info, &mcp_client)
        .await
        .unwrap();

    match result {
        PermissionResult::Run { arguments, .. } => assert_eq!(arguments, modified),
        PermissionResult::Skip { .. } => panic!("Expected Run, got Skip"),
    }
}

#[tokio::test]
async fn test_run_mode_ask_edit_invalid_json_cancel() {
    // Flow: Ask → 'e' → invalid JSON → 'n' (cancel) → Skip
    let editor = MockEditorBackend::invalid_json();
    // 'e' to edit, 'n' to cancel after invalid JSON
    let prompt = MockPromptBackend::new().with_inline_responses(['e', 'n']);
    let prompter = prompter_with_mocks(editor, prompt);
    let mcp_client = jp_mcp::Client::default();

    let info = make_permission_info(RunMode::Ask, serde_json::json!({}));
    let result = prompter
        .prompt_permission(&info, &mcp_client)
        .await
        .unwrap();

    match result {
        PermissionResult::Skip { reason, .. } => {
            assert_eq!(reason, Some("Edit cancelled".to_string()));
        }
        PermissionResult::Run { .. } => panic!("Expected Skip, got Run"),
    }
}

#[test]
fn test_result_confirmation_approves() {
    let prompt = MockPromptBackend::new().with_inline_responses(['y']);
    let prompter = prompter_with_mock_prompt(prompt);

    let result = prompter.prompt_result_confirmation("test_tool").unwrap();

    assert!(result, "Expected confirmation to return true");
}

#[test]
fn test_result_confirmation_skips() {
    let prompt = MockPromptBackend::new().with_inline_responses(['n']);
    let prompter = prompter_with_mock_prompt(prompt);

    let result = prompter.prompt_result_confirmation("test_tool").unwrap();

    assert!(!result, "Expected skip to return false");
}

#[test]
fn test_result_confirmation_edit_requested() {
    // When user presses 'e', prompt_result_confirmation returns an error
    // signaling that the caller should handle editing
    let editor = MockEditorBackend::empty(); // Need editor for 'e' option
    let prompt = MockPromptBackend::new().with_inline_responses(['e']);
    let prompter = prompter_with_mocks(editor, prompt);

    let result = prompter.prompt_result_confirmation("test_tool");

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("edit_requested"));
}

#[test]
fn test_result_confirmation_cancelled_returns_false() {
    // When prompt is cancelled (no response), returns false
    let prompt = MockPromptBackend::new(); // No responses = cancelled
    let prompter = prompter_with_mock_prompt(prompt);

    let result = prompter.prompt_result_confirmation("test_tool").unwrap();

    assert!(!result, "Expected cancelled to return false");
}

#[test]
fn test_prompt_question_boolean_uses_inline_select() {
    let prompt = MockPromptBackend::new().with_inline_responses(['y']);
    let prompter = prompter_with_mock_prompt(prompt);

    let question = jp_tool::Question {
        id: "q1".to_string(),
        text: "Proceed?".to_string(),
        answer_type: jp_tool::AnswerType::Boolean,
        default: None,
    };

    let result = prompter.prompt_question(&question).unwrap();
    assert_eq!(result.answer, Value::Bool(true));
    assert_eq!(result.persist_level, jp_tool::PersistLevel::None);
}

#[test]
fn test_prompt_question_boolean_default() {
    // With default=true (y), entering nothing (simulated by Mock defaulting logic?)
    // MockPromptBackend::inline_select returns the first response.
    // If we want to simulate "default used", we can't easily with current MockPromptBackend
    // unless we interpret a specific char as "default used"?
    // But `inline_select` implementation in MockPromptBackend returns what's in queue.
    // So we just verify `default` is passed to backend.
    // However, MockPromptBackend stores responses, it doesn't inspect args passed to it.
    // To verify default is passed, we might need a spy or just rely on manual verification/Terminal backend correctness.
    // Since MockPromptBackend doesn't capture calls, we can only verify the return value logic.

    let prompt = MockPromptBackend::new().with_inline_responses(['N']);
    let prompter = prompter_with_mock_prompt(prompt);

    let question = jp_tool::Question {
        id: "q1".to_string(),
        text: "Proceed?".to_string(),
        answer_type: jp_tool::AnswerType::Boolean,
        default: Some(Value::Bool(false)),
    };

    let result = prompter.prompt_question(&question).unwrap();
    assert_eq!(result.answer, Value::Bool(false));
    assert_eq!(result.persist_level, jp_tool::PersistLevel::Turn);
}

#[test]
fn test_prompt_question_text_uses_backend() {
    let prompt = MockPromptBackend::new().with_text_responses(["user input"]);
    let prompter = prompter_with_mock_prompt(prompt);

    let question = jp_tool::Question {
        id: "q2".to_string(),
        text: "Input:".to_string(),
        answer_type: jp_tool::AnswerType::Text,
        default: None,
    };

    let result = prompter.prompt_question(&question).unwrap();
    assert_eq!(result.answer, Value::String("user input".to_string()));
}

#[test]
fn test_prompt_question_select_uses_backend() {
    let prompt = MockPromptBackend::new().with_select_responses(["Option B"]);
    let prompter = prompter_with_mock_prompt(prompt);

    let question = jp_tool::Question {
        id: "q3".to_string(),
        text: "Choose:".to_string(),
        answer_type: jp_tool::AnswerType::Select {
            options: vec!["Option A".to_string(), "Option B".to_string()],
        },
        default: None,
    };

    let result = prompter.prompt_question(&question).unwrap();
    assert_eq!(result.answer, Value::String("Option B".to_string()));
}

#[test]
fn test_skip_reasoning_returns_editor_content() {
    let mock = MockEditorBackend::always("This tool is not needed for this task.");
    let prompter = prompter_with_mock_editor(mock);

    let result = prompter.edit_text("").unwrap().unwrap();
    assert_eq!(result, "This tool is not needed for this task.");
}

#[test]
fn test_skip_reasoning_empty_returns_default() {
    let mock = MockEditorBackend::empty();
    let prompter = prompter_with_mock_editor(mock);

    let result = prompter.edit_text("").unwrap();
    assert_eq!(result, None);
}

#[test]
fn test_skip_reasoning_placeholder_unchanged_returns_default() {
    let placeholder = "_Provide reasoning for skipping tool execution_";
    let mock = MockEditorBackend::always(placeholder);
    let prompter = prompter_with_mock_editor(mock);

    let result = prompter.edit_text(placeholder).unwrap();
    assert_eq!(result, None);
}

#[test]
fn test_skip_reasoning_no_editor_returns_error() {
    let prompter = prompter_without_editor();

    let result = prompter.edit_text("");
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("No editor configured")
    );
}

#[tokio::test]
async fn test_run_mode_ask_skip_with_reasoning() {
    // Flow: Ask → 'r' (skip and reply) → editor → Skip with reason
    let editor = MockEditorBackend::always("Not applicable for this commit.");
    let prompt = MockPromptBackend::new().with_inline_responses(['r']);
    let prompter = prompter_with_mocks(editor, prompt);
    let mcp_client = jp_mcp::Client::default();

    let info = make_permission_info(RunMode::Ask, serde_json::json!({}));
    let result = prompter
        .prompt_permission(&info, &mcp_client)
        .await
        .unwrap();

    match result {
        PermissionResult::Skip { reason, .. } => {
            assert_eq!(reason, Some("Not applicable for this commit.".to_string()));
        }
        PermissionResult::Run { .. } => panic!("Expected Skip, got Run"),
    }
}

#[tokio::test]
async fn test_run_mode_ask_skip_with_reasoning_empty_editor() {
    // Flow: Ask → 'r' (skip and reply) → empty editor → Skip with no reason
    // edit_text returns None for empty content, which becomes the skip reason.
    let editor = MockEditorBackend::empty();
    let prompt = MockPromptBackend::new().with_inline_responses(['r']);
    let prompter = prompter_with_mocks(editor, prompt);
    let mcp_client = jp_mcp::Client::default();

    let info = make_permission_info(RunMode::Ask, serde_json::json!({}));
    let result = prompter
        .prompt_permission(&info, &mcp_client)
        .await
        .unwrap();

    match result {
        PermissionResult::Skip { reason, .. } => {
            assert_eq!(reason, None);
        }
        PermissionResult::Run { .. } => panic!("Expected Skip, got Run"),
    }
}
