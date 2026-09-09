use std::{sync::Arc, time::Duration};

use jp_editor::MockEditorBackend;
use jp_inquire::{ReplyOutcome, prompt::MockPromptBackend};
use jp_md::format::BackgroundFill;
use jp_printer::{OutputFormat, PrintableExt as _, SharedBuffer};
use serde_json::json;

use super::*;

fn printer() -> Arc<Printer> {
    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    Arc::new(printer)
}

/// A prompter whose printer's prompt stream is readable.
///
/// `Printer::memory` has no tty, so prompts fall back to `out`.
fn prompter_with_output(prompt: MockPromptBackend) -> (ToolPrompter, SharedBuffer) {
    let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
    let prompter = ToolPrompter::with_backends(Arc::new(printer), None, Arc::new(prompt));

    (prompter, out)
}

/// A full-width reasoning-region background.
fn terminal_region() -> DefaultBackground {
    DefaultBackground {
        param: "48;5;236".into(),
        fill: BackgroundFill::Terminal,
    }
}

#[test]
fn a_prompt_inside_a_reasoning_block_carries_its_background() {
    let (prompter, out) = prompter_with_output(MockPromptBackend::new());
    prompter.set_background(Some(terminal_region()));

    {
        let mut canvas = prompter.canvas();
        write!(canvas, "Run local shell tool?").unwrap();
    }
    prompter.printer.flush();

    // The background is asserted before the text and closed once the widget is
    // done with the terminal, so the row is shaded and nothing after it is.
    assert_eq!(*out.lock(), "\x1b[48;5;236mRun local shell tool?\x1b[49m");
}

#[test]
fn a_prompt_outside_a_reasoning_block_is_unshaded() {
    let (prompter, out) = prompter_with_output(MockPromptBackend::new());

    {
        let mut canvas = prompter.canvas();
        write!(canvas, "Run local shell tool?").unwrap();
    }
    prompter.printer.flush();

    assert_eq!(*out.lock(), "Run local shell tool?");
}

#[test]
fn a_cancelled_prompt_still_closes_its_background() {
    // A widget can end by `Ctrl+C` or by an error, neither of which returns
    // through the normal path. The close lives in `Drop` so the background
    // cannot outlive the prompt and paint whatever is printed next.
    let (prompter, out) = prompter_with_output(MockPromptBackend::new());
    prompter.set_background(Some(terminal_region()));

    {
        let mut canvas = prompter.canvas();
        write!(canvas, "Deliver result?").unwrap();
        // No further writes: the prompt is abandoned mid-session.
    }
    prompter.printer.flush();

    let rendered = out.lock().clone();
    assert!(
        rendered.ends_with("\x1b[49m"),
        "an abandoned prompt must still close its background, got {rendered:?}"
    );
}

#[test]
fn prompting_a_question_shades_through_the_canvas() {
    // The tests above drive the canvas directly, so they hold even if a prompt
    // method still reached for a bare prompt writer. This one goes through
    // `prompt_question`, whose pre-amble is the one part of a prompt written by
    // the prompter rather than by the widget.
    let (prompter, out) =
        prompter_with_output(MockPromptBackend::new().with_inline_responses(['y']));
    prompter.set_background(Some(terminal_region()));

    let mut question = jp_tool::Question::boolean("confirm", "Proceed?").expect("valid question");
    question.pre_amble = Some("About to run a shell command".to_owned());

    prompter
        .prompt_question(&question)
        .expect("the mock answers the question");
    prompter.printer.flush();

    // Background asserted, text, fill to the right edge, then the background
    // closed *before* the line break — a `\n` written under an active
    // background paints the row the terminal scrolls in.
    assert_eq!(
        *out.lock(),
        "\x1b[48;5;236mAbout to run a shell command\x1b[K\x1b[49m\n"
    );
}

#[test]
fn flushing_a_shaded_canvas_waits_for_the_printer() {
    // A widget flushes its writer before it reads a key, and on this path a
    // flush is a barrier rather than a buffer drain: it is what makes the bytes
    // have landed before the widget takes the cursor and the terminal's mode.
    // Shading is a decoration over that writer and has no business swallowing
    // it.
    let (prompter, out) = prompter_with_output(MockPromptBackend::new());
    prompter.set_background(Some(terminal_region()));

    let mut canvas = prompter.canvas();

    // Queued after acquisition drained the printer, and slow enough that the
    // worker is certainly still inside it: without a real barrier the prompt's
    // own text cannot have reached the terminal yet.
    prompter
        .printer
        .print("slow".typewriter(Duration::from_millis(100)));

    write!(canvas, "Run local shell tool?").unwrap();
    canvas.flush().unwrap();

    // Deliberately no `printer.flush()`, which is the assertion.
    assert!(
        out.lock().contains("Run local shell tool?"),
        "flushing the canvas must drain the printer, got {:?}",
        *out.lock()
    );
}

#[test]
fn clearing_the_background_unshades_later_prompts() {
    let (prompter, out) = prompter_with_output(MockPromptBackend::new());

    prompter.set_background(Some(terminal_region()));
    drop(prompter.canvas());
    prompter.set_background(None);

    {
        let mut canvas = prompter.canvas();
        write!(canvas, "after").unwrap();
    }
    prompter.printer.flush();

    assert!(
        out.lock().ends_with("after"),
        "a prompt after the block closed carries no background: {:?}",
        *out.lock()
    );
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

#[test]
fn an_editor_failure_reaches_the_user_while_the_permission_prompt_is_open() {
    // `prompt_ask` keeps its canvas alive across the edit, so the notice has to
    // land while that session still owns the terminal. Held until the prompt
    // returns, it arrives after the widget the user is looking at has already
    // re-opened with their text and no reason for it.
    let (printer, _out, err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let prompt = MockPromptBackend::new().with_reply_outcomes([
        ReplyOutcome::OpenEditor {
            current_text: "{}".into(),
        },
        ReplyOutcome::Submit(r#"{"key":"value"}"#.into()),
    ]);
    let prompter = ToolPrompter::with_backends(
        printer.clone(),
        Some(Arc::new(MockEditorBackend::failing())),
        Arc::new(prompt),
    );

    let canvas = printer.prompt_writer();
    let result = prompter.try_edit_arguments(&json!({})).unwrap();
    printer.flush();

    assert!(matches!(result, EditResult::Edited(_)));
    assert_eq!(
        *err.lock(),
        "\n⚠ Couldn't open your editor: failed to spawn editor. Keeping your text.\n"
    );

    drop(canvas);
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
    assert_eq!(
        *err.lock(),
        "\n⚠ Couldn't open your editor: failed to spawn editor. Keeping your text.\n"
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
    let question = jp_tool::Question::boolean("q1", "Proceed?").unwrap();
    let result = prompter(prompt).prompt_question(&question).unwrap();
    assert_eq!(result.answer, Value::Bool(true));
    assert_eq!(result.persist_level, jp_tool::PersistLevel::None);
}

#[test]
fn question_text_uses_backend() {
    let prompt = MockPromptBackend::new().with_text_responses(["user input"]);
    let question = jp_tool::Question::text("q2", "Input:").unwrap();
    let result = prompter(prompt).prompt_question(&question).unwrap();
    assert_eq!(result.answer, Value::String("user input".to_string()));
}

#[test]
fn question_select_uses_backend() {
    let prompt = MockPromptBackend::new().with_select_responses(["Option B"]);
    let question = jp_tool::Question::select("q3", "Choose:")
        .unwrap()
        .with_options(vec!["Option A".to_string(), "Option B".to_string()]);
    let result = prompter(prompt).prompt_question(&question).unwrap();
    assert_eq!(result.answer, Value::String("Option B".to_string()));
}
