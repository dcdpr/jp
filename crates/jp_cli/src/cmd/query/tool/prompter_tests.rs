use std::sync::Arc;

use jp_editor::MockEditorBackend;
use jp_inquire::{ReplyOutcome, prompt::MockPromptBackend};
use jp_printer::OutputFormat;
use serde_json::json;

use super::*;

fn printer() -> Arc<Printer> {
    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    Arc::new(printer)
}

/// Prompter with a mock prompt backend and no editor.
///
/// The inline widget (the prompt backend), not the editor, drives edits now;
/// the editor is only the `Ctrl+X` escape.
fn prompter(prompt: MockPromptBackend) -> ToolPrompter {
    ToolPrompter::with_backends(printer(), None, Arc::new(prompt))
}

/// Prompter with a mock prompt backend and a mock editor for the `Ctrl+X`
/// escape.
fn prompter_with_editor(prompt: MockPromptBackend, editor: MockEditorBackend) -> ToolPrompter {
    ToolPrompter::with_backends(printer(), Some(Arc::new(editor)), Arc::new(prompt))
}

fn make_permission_info(run_mode: RunMode, arguments: Value) -> PermissionInfo {
    PermissionInfo {
        tool_id: "call_123".to_string(),
        tool_name: "test_tool".to_string(),
        tool_source: ToolSource::Builtin { tool: None },
        run_mode,
        arguments,
    }
}

#[test]
fn permission_result_variants() {
    let run = PermissionResult::Run {
        arguments: json!({"key": "value"}),
        persist: false,
    };
    assert!(matches!(run, PermissionResult::Run { .. }));

    let skip = PermissionResult::Skip {
        reason: Some("User cancelled".to_string()),
        persist: false,
    };
    assert!(matches!(skip, PermissionResult::Skip {
        reason: Some(_),
        ..
    }));
}

// --- Argument editing (`try_edit_arguments`) ------------------------------

#[test]
fn edit_arguments_returns_modified_json() {
    let modified = json!({"key": "modified"});
    let prompt = MockPromptBackend::new().with_reply_outcomes([ReplyOutcome::Submit(
        serde_json::to_string(&modified).unwrap(),
    )]);

    let result = prompter(prompt)
        .try_edit_arguments(&json!({"key": "original"}))
        .unwrap();

    match result {
        EditResult::Edited(v) => assert_eq!(v, modified),
        other => panic!("expected Edited, got {other:?}"),
    }
}

#[test]
fn edit_arguments_empty_returns_emptied() {
    let prompt =
        MockPromptBackend::new().with_reply_outcomes([ReplyOutcome::Submit(String::new())]);
    let result = prompter(prompt)
        .try_edit_arguments(&json!({"key": "value"}))
        .unwrap();
    assert!(matches!(result, EditResult::Emptied));
}

#[test]
fn edit_arguments_cancel_returns_cancelled() {
    let prompt = MockPromptBackend::new().with_reply_outcomes([ReplyOutcome::Cancelled]);
    let result = prompter(prompt)
        .try_edit_arguments(&json!({"key": "value"}))
        .unwrap();
    assert!(matches!(result, EditResult::Cancelled));
}

#[test]
fn edit_arguments_invalid_json_reprompts_then_accepts() {
    // Invalid JSON re-seeds the inline prompt (no retry menu); a second submit
    // with valid JSON succeeds.
    let valid = json!({"key": "fixed"});
    let prompt = MockPromptBackend::new().with_reply_outcomes([
        ReplyOutcome::Submit("{ not json }".into()),
        ReplyOutcome::Submit(serde_json::to_string(&valid).unwrap()),
    ]);

    let result = prompter(prompt).try_edit_arguments(&json!({})).unwrap();
    match result {
        EditResult::Edited(v) => assert_eq!(v, valid),
        other => panic!("expected Edited, got {other:?}"),
    }
}

#[test]
fn edit_arguments_invalid_json_then_cancel() {
    let prompt = MockPromptBackend::new().with_reply_outcomes([
        ReplyOutcome::Submit("{ not json }".into()),
        ReplyOutcome::Cancelled,
    ]);
    let result = prompter(prompt).try_edit_arguments(&json!({})).unwrap();
    assert!(matches!(result, EditResult::Cancelled));
}

#[test]
fn edit_arguments_open_editor_re_seeds_then_submits() {
    // Ctrl+X opens the editor; its output is re-seeded and submitted inline.
    let modified = json!({"key": "from editor"});
    let prompt = MockPromptBackend::new().with_reply_outcomes([
        ReplyOutcome::OpenEditor {
            current_text: "{}".into(),
        },
        ReplyOutcome::Submit(serde_json::to_string(&modified).unwrap()),
    ]);
    let editor = MockEditorBackend::always(serde_json::to_string(&modified).unwrap());

    let result = prompter_with_editor(prompt, editor)
        .try_edit_arguments(&json!({}))
        .unwrap();
    match result {
        EditResult::Edited(v) => assert_eq!(v, modified),
        other => panic!("expected Edited, got {other:?}"),
    }
}

// --- Result editing (`edit_result`) --------------------------------------

#[test]
fn edit_result_returns_modified_content() {
    let prompt = MockPromptBackend::new()
        .with_reply_outcomes([ReplyOutcome::Submit("edited result".into())]);
    let result = prompter(prompt).edit_result("original").unwrap();
    assert_eq!(result, Some("edited result".to_string()));
}

#[test]
fn edit_result_empty_returns_none() {
    let prompt =
        MockPromptBackend::new().with_reply_outcomes([ReplyOutcome::Submit(String::new())]);
    let result = prompter(prompt).edit_result("original").unwrap();
    assert_eq!(result, None);
}

#[test]
fn edit_result_cancel_returns_none() {
    let prompt = MockPromptBackend::new().with_reply_outcomes([ReplyOutcome::Cancelled]);
    let result = prompter(prompt).edit_result("original").unwrap();
    assert_eq!(result, None);
}

#[test]
fn edit_result_preserves_multiline_content() {
    let multiline = "line 1\nline 2\nline 3";
    let prompt =
        MockPromptBackend::new().with_reply_outcomes([ReplyOutcome::Submit(multiline.into())]);
    let result = prompter(prompt).edit_result("original").unwrap();
    assert_eq!(result, Some(multiline.to_string()));
}

#[test]
fn edit_result_editor_escape_failure_keeps_buffer_and_notifies_chrome() {
    // Ctrl+X -> editor can't start -> the typed buffer is kept and the widget
    // re-prompts, so a second submit still returns the text (the spawn failure
    // must NOT propagate as a fatal prompt error — the old `?` behavior). The
    // failure is surfaced on the chrome channel (stderr), not just the tracing
    // log, so the user knows their editor didn't open.
    let (printer, _out, err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let prompt = MockPromptBackend::new().with_reply_outcomes([
        ReplyOutcome::OpenEditor {
            current_text: "draft".into(),
        },
        ReplyOutcome::Submit("draft, then more".into()),
    ]);
    let prompter = ToolPrompter::with_backends(
        printer.clone(),
        Some(Arc::new(MockEditorBackend::failing())),
        Arc::new(prompt),
    );

    let result = prompter.edit_result("seed").unwrap();
    assert_eq!(result, Some("draft, then more".to_string()));

    printer.flush();
    let output = err.lock();
    assert!(
        output.contains("Couldn't open your editor"),
        "editor failure must be surfaced on chrome. Output: {output}"
    );
}

// --- Skip reasoning (`edit_text`) ----------------------------------------

#[test]
fn skip_reasoning_returns_text() {
    let prompt = MockPromptBackend::new()
        .with_reply_outcomes([ReplyOutcome::Submit("not needed here".into())]);
    let result = prompter(prompt).edit_text("placeholder").unwrap();
    assert_eq!(result, Some("not needed here".to_string()));
}

#[test]
fn skip_reasoning_empty_returns_none() {
    let prompt =
        MockPromptBackend::new().with_reply_outcomes([ReplyOutcome::Submit(String::new())]);
    let result = prompter(prompt).edit_text("placeholder").unwrap();
    assert_eq!(result, None);
}

#[test]
fn skip_reasoning_unchanged_placeholder_returns_none() {
    let placeholder = "_Provide reasoning_";
    let prompt =
        MockPromptBackend::new().with_reply_outcomes([ReplyOutcome::Submit(placeholder.into())]);
    let result = prompter(prompt).edit_text(placeholder).unwrap();
    assert_eq!(result, None);
}

#[test]
fn skip_reasoning_cancel_returns_none() {
    let prompt = MockPromptBackend::new().with_reply_outcomes([ReplyOutcome::Cancelled]);
    let result = prompter(prompt).edit_text("placeholder").unwrap();
    assert_eq!(result, None);
}

// --- Permission prompts (`prompt_permission`) ----------------------------

#[test]
fn permission_unattended_returns_original_args() {
    let original = json!({"key": "original"});
    let info = make_permission_info(RunMode::Unattended, original.clone());
    let result = prompter(MockPromptBackend::new())
        .prompt_permission(&info)
        .unwrap();
    match result {
        PermissionResult::Run { arguments, .. } => assert_eq!(arguments, original),
        other @ PermissionResult::Skip { .. } => panic!("expected Run, got {other:?}"),
    }
}

#[test]
fn permission_skip_returns_skip() {
    let info = make_permission_info(RunMode::Skip, json!({}));
    let result = prompter(MockPromptBackend::new())
        .prompt_permission(&info)
        .unwrap();
    assert!(matches!(result, PermissionResult::Skip {
        reason: None,
        ..
    }));
}

#[test]
fn permission_edit_returns_modified_args() {
    let modified = json!({"key": "modified", "extra": true});
    let prompt = MockPromptBackend::new().with_reply_outcomes([ReplyOutcome::Submit(
        serde_json::to_string(&modified).unwrap(),
    )]);
    let info = make_permission_info(RunMode::Edit, json!({"key": "original"}));

    let result = prompter(prompt).prompt_permission(&info).unwrap();
    match result {
        PermissionResult::Run { arguments, .. } => assert_eq!(arguments, modified),
        other @ PermissionResult::Skip { .. } => panic!("expected Run, got {other:?}"),
    }
}

#[test]
fn ask_then_run() {
    let original = json!({"key": "original"});
    let prompt = MockPromptBackend::new().with_inline_responses(['y']);
    let info = make_permission_info(RunMode::Ask, original.clone());

    let result = prompter(prompt).prompt_permission(&info).unwrap();
    match result {
        PermissionResult::Run { arguments, .. } => assert_eq!(arguments, original),
        other @ PermissionResult::Skip { .. } => panic!("expected Run, got {other:?}"),
    }
}

#[test]
fn ask_then_skip() {
    let prompt = MockPromptBackend::new().with_inline_responses(['n']);
    let info = make_permission_info(RunMode::Ask, json!({}));
    let result = prompter(prompt).prompt_permission(&info).unwrap();
    assert!(matches!(result, PermissionResult::Skip {
        reason: None,
        ..
    }));
}

#[test]
fn ask_edit_modifies_args() {
    // Ask -> 'e' -> inline edit -> Run with modified args.
    let modified = json!({"key": "modified"});
    let prompt = MockPromptBackend::new()
        .with_inline_responses(['e'])
        .with_reply_outcomes([ReplyOutcome::Submit(
            serde_json::to_string(&modified).unwrap(),
        )]);
    let info = make_permission_info(RunMode::Ask, json!({"key": "original"}));

    let result = prompter(prompt).prompt_permission(&info).unwrap();
    match result {
        PermissionResult::Run { arguments, .. } => assert_eq!(arguments, modified),
        other @ PermissionResult::Skip { .. } => panic!("expected Run, got {other:?}"),
    }
}

#[test]
fn ask_edit_empty_loops_back_then_approves() {
    // Ask -> 'e' -> empty (back to Ask) -> 'y' -> Run with original args.
    let original = json!({"key": "original"});
    let prompt = MockPromptBackend::new()
        .with_inline_responses(['e', 'y'])
        .with_reply_outcomes([ReplyOutcome::Submit(String::new())]);
    let info = make_permission_info(RunMode::Ask, original.clone());

    let result = prompter(prompt).prompt_permission(&info).unwrap();
    match result {
        PermissionResult::Run { arguments, .. } => assert_eq!(arguments, original),
        other @ PermissionResult::Skip { .. } => panic!("expected Run, got {other:?}"),
    }
}

#[test]
fn ask_skip_with_reasoning() {
    // Ask -> 'r' -> reason -> Skip with that reason.
    let prompt = MockPromptBackend::new()
        .with_inline_responses(['r'])
        .with_reply_outcomes([ReplyOutcome::Submit("not applicable".into())]);
    let info = make_permission_info(RunMode::Ask, json!({}));

    let result = prompter(prompt).prompt_permission(&info).unwrap();
    match result {
        PermissionResult::Skip { reason, .. } => {
            assert_eq!(reason, Some("not applicable".to_string()));
        }
        other @ PermissionResult::Run { .. } => panic!("expected Skip, got {other:?}"),
    }
}

#[test]
fn ask_skip_with_empty_reasoning() {
    // Ask -> 'r' -> empty -> Skip with no reason.
    let prompt = MockPromptBackend::new()
        .with_inline_responses(['r'])
        .with_reply_outcomes([ReplyOutcome::Submit(String::new())]);
    let info = make_permission_info(RunMode::Ask, json!({}));

    let result = prompter(prompt).prompt_permission(&info).unwrap();
    match result {
        PermissionResult::Skip { reason, .. } => assert_eq!(reason, None),
        other @ PermissionResult::Run { .. } => panic!("expected Skip, got {other:?}"),
    }
}

// --- Result delivery confirmation (`prompt_result_confirmation`) ----------

#[test]
fn result_confirmation_approves() {
    let prompt = MockPromptBackend::new().with_inline_responses(['y']);
    assert!(
        prompter(prompt)
            .prompt_result_confirmation("test_tool")
            .unwrap()
    );
}

#[test]
fn result_confirmation_skips() {
    let prompt = MockPromptBackend::new().with_inline_responses(['n']);
    assert!(
        !prompter(prompt)
            .prompt_result_confirmation("test_tool")
            .unwrap()
    );
}

#[test]
fn result_confirmation_edit_requested() {
    // 'e' is always offered now (un-gated); it signals edit via an error.
    let prompt = MockPromptBackend::new().with_inline_responses(['e']);
    let result = prompter(prompt).prompt_result_confirmation("test_tool");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("edit_requested"));
}

#[test]
fn result_confirmation_cancelled_returns_false() {
    let prompt = MockPromptBackend::new();
    assert!(
        !prompter(prompt)
            .prompt_result_confirmation("test_tool")
            .unwrap()
    );
}

// --- Tool questions (`prompt_question`) ----------------------------------

#[test]
fn question_boolean_uses_inline_select() {
    let prompt = MockPromptBackend::new().with_inline_responses(['y']);
    let question = jp_tool::Question::boolean("q1", "Proceed?");
    let result = prompter(prompt).prompt_question(&question).unwrap();
    assert_eq!(result.answer, Value::Bool(true));
    assert_eq!(result.persist_level, jp_tool::PersistLevel::None);
}

#[test]
fn question_text_uses_backend() {
    let prompt = MockPromptBackend::new().with_text_responses(["user input"]);
    let question = jp_tool::Question::text("q2", "Input:");
    let result = prompter(prompt).prompt_question(&question).unwrap();
    assert_eq!(result.answer, Value::String("user input".to_string()));
}

#[test]
fn question_select_uses_backend() {
    let prompt = MockPromptBackend::new().with_select_responses(["Option B"]);
    let question = jp_tool::Question::select("q3", "Choose:")
        .with_options(vec!["Option A".to_string(), "Option B".to_string()]);
    let result = prompter(prompt).prompt_question(&question).unwrap();
    assert_eq!(result.answer, Value::String("Option B".to_string()));
}
