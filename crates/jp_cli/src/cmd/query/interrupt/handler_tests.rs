use std::sync::Arc;

use jp_editor::MockEditorBackend;
use jp_inquire::{ReplyEditMode, ReplyOutcome, prompt::MockPromptBackend};
use jp_printer::{OutputFormat, Printer};

use super::*;

fn make_printer() -> Printer {
    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    printer
}

/// Streaming config with the given action and no straight-to-editor opt-in.
fn streaming(action: StreamingInterruptAction) -> StreamingInterruptConfig {
    StreamingInterruptConfig {
        action,
        compose_in_editor: ComposeInEditor::Inline,
    }
}

/// Tool config with the given action and no straight-to-editor opt-in.
fn tool(action: ToolInterruptAction) -> ToolInterruptConfig {
    ToolInterruptConfig {
        action,
        compose_in_editor: ComposeInEditor::Inline,
    }
}

/// Build a handler with no editor configured (emacs inline editing).
fn handler(backend: MockPromptBackend) -> InterruptHandler<MockPromptBackend> {
    InterruptHandler::with_backend(backend, None, ReplyEditMode::Emacs)
}

/// Build a handler with a (mock) editor for the reply escape hatch.
fn handler_with_editor(
    backend: MockPromptBackend,
    editor: MockEditorBackend,
) -> InterruptHandler<MockPromptBackend> {
    InterruptHandler::with_backend(backend, Some(Arc::new(editor)), ReplyEditMode::Emacs)
}

#[test]
fn streaming_interrupt_stop() {
    let handler = handler(MockPromptBackend::new().with_inline_responses(['s']));
    let action = handler.handle_streaming_interrupt(
        &streaming(StreamingInterruptAction::Prompt),
        &make_printer(),
        true,
    );
    assert_eq!(action, InterruptAction::Stop);
}

#[test]
fn streaming_interrupt_abort() {
    let handler = handler(MockPromptBackend::new().with_inline_responses(['a']));
    let action = handler.handle_streaming_interrupt(
        &streaming(StreamingInterruptAction::Prompt),
        &make_printer(),
        true,
    );
    assert_eq!(action, InterruptAction::Abort);
}

#[test]
fn streaming_interrupt_reply_submits() {
    let backend = MockPromptBackend::new()
        .with_inline_responses(['r'])
        .with_reply_outcomes([ReplyOutcome::Submit("my reply message".into())]);
    let action = handler(backend).handle_streaming_interrupt(
        &streaming(StreamingInterruptAction::Prompt),
        &make_printer(),
        true,
    );
    assert_eq!(action, InterruptAction::Reply("my reply message".into()));
}

#[test]
fn streaming_interrupt_reply_cancel_returns_to_menu() {
    // `r` → cancel the reply → back to the menu → `s` → Stop.
    let backend = MockPromptBackend::new()
        .with_inline_responses(['r', 's'])
        .with_reply_outcomes([ReplyOutcome::Cancelled]);
    let action = handler(backend).handle_streaming_interrupt(
        &streaming(StreamingInterruptAction::Prompt),
        &make_printer(),
        true,
    );
    assert_eq!(action, InterruptAction::Stop);
}

#[test]
fn streaming_interrupt_reply_empty_returns_to_menu_then_submits() {
    // `r` → empty submit → back to menu → `r` → submit "second try".
    let backend = MockPromptBackend::new()
        .with_inline_responses(['r', 'r'])
        .with_reply_outcomes([
            ReplyOutcome::Submit(String::new()),
            ReplyOutcome::Submit("second try".into()),
        ]);
    let action = handler(backend).handle_streaming_interrupt(
        &streaming(StreamingInterruptAction::Prompt),
        &make_printer(),
        true,
    );
    assert_eq!(action, InterruptAction::Reply("second try".into()));
}

#[test]
fn streaming_interrupt_open_editor_re_seeds_then_submits() {
    // `r` → Ctrl+X → editor returns text → re-seeded inline prompt → submit.
    let backend = MockPromptBackend::new()
        .with_inline_responses(['r'])
        .with_reply_outcomes([
            ReplyOutcome::OpenEditor {
                current_text: "draft".into(),
            },
            ReplyOutcome::Submit("from the editor, edited inline".into()),
        ]);
    let editor = MockEditorBackend::always("from the editor");
    let action = handler_with_editor(backend, editor).handle_streaming_interrupt(
        &streaming(StreamingInterruptAction::Prompt),
        &make_printer(),
        true,
    );
    assert_eq!(
        action,
        InterruptAction::Reply("from the editor, edited inline".into())
    );
}

#[test]
fn streaming_interrupt_open_editor_empty_re_prompts_then_menu() {
    // `r` → Ctrl+X → editor emptied → re-prompt inline (empty) → empty submit →
    // back to menu → `s` → Stop. The editor escape is never terminal.
    let backend = MockPromptBackend::new()
        .with_inline_responses(['r', 's'])
        .with_reply_outcomes([
            ReplyOutcome::OpenEditor {
                current_text: String::new(),
            },
            ReplyOutcome::Submit(String::new()),
        ]);
    let editor = MockEditorBackend::empty();
    let action = handler_with_editor(backend, editor).handle_streaming_interrupt(
        &streaming(StreamingInterruptAction::Prompt),
        &make_printer(),
        true,
    );
    assert_eq!(action, InterruptAction::Stop);
}

#[test]
fn streaming_interrupt_continue_stream_alive() {
    let handler = handler(MockPromptBackend::new().with_inline_responses(['c']));
    let action = handler.handle_streaming_interrupt(
        &streaming(StreamingInterruptAction::Prompt),
        &make_printer(),
        true,
    );
    assert_eq!(action, InterruptAction::Resume);
}

#[test]
fn streaming_interrupt_continue_stream_dead() {
    let handler = handler(MockPromptBackend::new().with_inline_responses(['c']));
    let action = handler.handle_streaming_interrupt(
        &streaming(StreamingInterruptAction::Prompt),
        &make_printer(),
        false,
    );
    assert_eq!(action, InterruptAction::Continue);
}

#[test]
fn streaming_interrupt_defaults_to_stop_on_error() {
    // No responses: the menu errors and falls back to Stop.
    let action = handler(MockPromptBackend::new()).handle_streaming_interrupt(
        &streaming(StreamingInterruptAction::Prompt),
        &make_printer(),
        true,
    );
    assert_eq!(action, InterruptAction::Stop);
}

#[test]
fn tool_interrupt_stop_with_custom_response() {
    let backend = MockPromptBackend::new()
        .with_inline_responses(['r'])
        .with_reply_outcomes([ReplyOutcome::Submit("don't run this tool".into())]);
    let action =
        handler(backend).handle_tool_interrupt(&tool(ToolInterruptAction::Prompt), &make_printer());
    assert_eq!(action, InterruptAction::ToolCancelled {
        response: "don't run this tool".into()
    });
}

#[test]
fn tool_interrupt_stop_empty_uses_canned_response() {
    // An empty reply falls through to the canned rejection message.
    let backend = MockPromptBackend::new()
        .with_inline_responses(['r'])
        .with_reply_outcomes([ReplyOutcome::Submit(String::new())]);
    let action =
        handler(backend).handle_tool_interrupt(&tool(ToolInterruptAction::Prompt), &make_printer());
    assert!(
        matches!(action, InterruptAction::ToolCancelled { response } if response.contains("intentionally rejected"))
    );
}

#[test]
fn tool_interrupt_reply_cancel_returns_to_menu() {
    // `Ctrl+C` in the reply prompt backs out to the tool menu (it does NOT send
    // the canned message); a follow-up `c` then continues.
    let backend = MockPromptBackend::new()
        .with_inline_responses(['r', 'c'])
        .with_reply_outcomes([ReplyOutcome::Cancelled]);
    let action =
        handler(backend).handle_tool_interrupt(&tool(ToolInterruptAction::Prompt), &make_printer());
    assert_eq!(action, InterruptAction::Resume);
}

#[test]
fn tool_interrupt_stop_whitespace_uses_canned_response() {
    // A whitespace-only reply is treated as blank and falls through to the
    // canned message, rather than sending a blank-looking tool response.
    let backend = MockPromptBackend::new()
        .with_inline_responses(['r'])
        .with_reply_outcomes([ReplyOutcome::Submit("   \n\t ".into())]);
    let action =
        handler(backend).handle_tool_interrupt(&tool(ToolInterruptAction::Prompt), &make_printer());
    assert!(
        matches!(action, InterruptAction::ToolCancelled { response } if response.contains("intentionally rejected"))
    );
}

#[test]
fn tool_interrupt_restart() {
    let handler = handler(MockPromptBackend::new().with_inline_responses(['t']));
    let action = handler.handle_tool_interrupt(&tool(ToolInterruptAction::Prompt), &make_printer());
    assert_eq!(action, InterruptAction::RestartTool);
}

#[test]
fn tool_interrupt_continue() {
    let handler = handler(MockPromptBackend::new().with_inline_responses(['c']));
    let action = handler.handle_tool_interrupt(&tool(ToolInterruptAction::Prompt), &make_printer());
    assert_eq!(action, InterruptAction::Resume);
}

#[test]
fn tool_interrupt_defaults_to_continue_on_error() {
    let action = handler(MockPromptBackend::new())
        .handle_tool_interrupt(&tool(ToolInterruptAction::Prompt), &make_printer());
    assert_eq!(action, InterruptAction::Resume);
}

// A configured (non-prompt) action runs without consulting the prompt backend:
// the empty backend would error if the menu were shown.

#[test]
fn configured_streaming_stop_skips_menu() {
    let action = handler(MockPromptBackend::new()).handle_streaming_interrupt(
        &streaming(StreamingInterruptAction::Stop),
        &make_printer(),
        true,
    );
    assert_eq!(action, InterruptAction::Stop);
}

#[test]
fn configured_streaming_abort_skips_menu() {
    let action = handler(MockPromptBackend::new()).handle_streaming_interrupt(
        &streaming(StreamingInterruptAction::Abort),
        &make_printer(),
        true,
    );
    assert_eq!(action, InterruptAction::Abort);
}

#[test]
fn configured_streaming_continue_tracks_stream_liveness() {
    let alive = handler(MockPromptBackend::new()).handle_streaming_interrupt(
        &streaming(StreamingInterruptAction::Continue),
        &make_printer(),
        true,
    );
    assert_eq!(alive, InterruptAction::Resume);

    let dead = handler(MockPromptBackend::new()).handle_streaming_interrupt(
        &streaming(StreamingInterruptAction::Continue),
        &make_printer(),
        false,
    );
    assert_eq!(dead, InterruptAction::Continue);
}

#[test]
fn configured_streaming_reply_uses_inline_prompt() {
    // A menu-less `reply` collects through the inline widget; backing out
    // resumes rather than returning to a (non-existent) menu.
    let backend = MockPromptBackend::new()
        .with_reply_outcomes([ReplyOutcome::Submit("changed my mind".into())]);
    let action = handler(backend).handle_streaming_interrupt(
        &streaming(StreamingInterruptAction::Reply),
        &make_printer(),
        true,
    );
    assert_eq!(action, InterruptAction::Reply("changed my mind".into()));
}

#[test]
fn configured_streaming_reply_cancel_resumes() {
    let backend = MockPromptBackend::new().with_reply_outcomes([ReplyOutcome::Cancelled]);
    let action = handler(backend).handle_streaming_interrupt(
        &streaming(StreamingInterruptAction::Reply),
        &make_printer(),
        true,
    );
    assert_eq!(action, InterruptAction::Resume);
}

#[test]
fn configured_streaming_reply_cancel_continues_when_stream_dead() {
    // A menu-less `reply` that backs out has no menu to return to. With the
    // stream already finished it must continue from the partial response rather
    // than resuming a stream that no longer exists.
    let backend = MockPromptBackend::new().with_reply_outcomes([ReplyOutcome::Cancelled]);
    let action = handler(backend).handle_streaming_interrupt(
        &streaming(StreamingInterruptAction::Reply),
        &make_printer(),
        false,
    );
    assert_eq!(action, InterruptAction::Continue);
}

#[test]
fn configured_tool_restart_skips_menu() {
    let action = handler(MockPromptBackend::new())
        .handle_tool_interrupt(&tool(ToolInterruptAction::Restart), &make_printer());
    assert_eq!(action, InterruptAction::RestartTool);
}

#[test]
fn configured_tool_respond_uses_inline_prompt() {
    let backend =
        MockPromptBackend::new().with_reply_outcomes([ReplyOutcome::Submit("use ripgrep".into())]);
    let action = handler(backend)
        .handle_tool_interrupt(&tool(ToolInterruptAction::Respond), &make_printer());
    assert_eq!(action, InterruptAction::ToolCancelled {
        response: "use ripgrep".into()
    });
}

#[test]
fn configured_tool_respond_cancel_uses_canned() {
    // A menu-less `respond` has no menu to return to, so `Ctrl+C` falls
    // through to the canned message (it must not loop).
    let backend = MockPromptBackend::new().with_reply_outcomes([ReplyOutcome::Cancelled]);
    let action = handler(backend)
        .handle_tool_interrupt(&tool(ToolInterruptAction::Respond), &make_printer());
    assert!(
        matches!(action, InterruptAction::ToolCancelled { response } if response.contains("intentionally rejected"))
    );
}

#[test]
fn compose_in_editor_opens_editor_directly() {
    // With `compose_in_editor`, `r` skips the inline widget and sends the editor
    // result directly. No reply outcomes are scripted on the prompt backend.
    let backend = MockPromptBackend::new().with_inline_responses(['r']);
    let editor = MockEditorBackend::always("written in the editor");
    let config = StreamingInterruptConfig {
        action: StreamingInterruptAction::Prompt,
        compose_in_editor: ComposeInEditor::Editor,
    };
    let action = handler_with_editor(backend, editor).handle_streaming_interrupt(
        &config,
        &make_printer(),
        true,
    );
    assert_eq!(
        action,
        InterruptAction::Reply("written in the editor".into())
    );
}

#[test]
fn compose_in_editor_empty_returns_to_menu() {
    // `true` (Editor): emptying the editor is a bail-out, so it returns to the
    // menu (not the inline widget) → `s` → Stop.
    let backend = MockPromptBackend::new().with_inline_responses(['r', 's']);
    let editor = MockEditorBackend::empty();
    let config = StreamingInterruptConfig {
        action: StreamingInterruptAction::Prompt,
        compose_in_editor: ComposeInEditor::Editor,
    };
    let action = handler_with_editor(backend, editor).handle_streaming_interrupt(
        &config,
        &make_printer(),
        true,
    );
    assert_eq!(action, InterruptAction::Stop);
}

#[test]
fn compose_always_spawn_failure_returns_to_menu() {
    // `"always"`: a broken editor must NOT fall back to the inline widget — it
    // returns to the menu. `r` → editor fails → menu → `s` → Stop. The scripted
    // inline reply is never consumed (no inline widget appears); landing on
    // Stop rather than Reply proves the inline path was not taken.
    let backend = MockPromptBackend::new()
        .with_inline_responses(['r', 's'])
        .with_reply_outcomes([ReplyOutcome::Submit("must not be used".into())]);
    let editor = MockEditorBackend::failing();
    let config = StreamingInterruptConfig {
        action: StreamingInterruptAction::Prompt,
        compose_in_editor: ComposeInEditor::Always,
    };
    let action = handler_with_editor(backend, editor).handle_streaming_interrupt(
        &config,
        &make_printer(),
        true,
    );
    assert_eq!(action, InterruptAction::Stop);
}

#[test]
fn compose_never_uses_inline_widget() {
    // `"never"`: inline widget only (the `Ctrl+X` escape is disabled — verified
    // in `jp_inquire`'s keymap tests). `r` → inline → submit.
    let backend = MockPromptBackend::new()
        .with_inline_responses(['r'])
        .with_reply_outcomes([ReplyOutcome::Submit("inline only".into())]);
    let config = StreamingInterruptConfig {
        action: StreamingInterruptAction::Prompt,
        compose_in_editor: ComposeInEditor::Never,
    };
    let action = handler(backend).handle_streaming_interrupt(&config, &make_printer(), true);
    assert_eq!(action, InterruptAction::Reply("inline only".into()));
}

#[test]
fn compose_in_editor_without_editor_falls_back_to_inline() {
    // `compose_in_editor` is set but no editor configured: fall back to the
    // inline widget rather than doing nothing.
    let backend = MockPromptBackend::new()
        .with_inline_responses(['r'])
        .with_reply_outcomes([ReplyOutcome::Submit("typed inline".into())]);
    let config = StreamingInterruptConfig {
        action: StreamingInterruptAction::Prompt,
        compose_in_editor: ComposeInEditor::Editor,
    };
    let action = handler(backend).handle_streaming_interrupt(&config, &make_printer(), true);
    assert_eq!(action, InterruptAction::Reply("typed inline".into()));
}

#[test]
fn compose_in_editor_spawn_failure_falls_back_to_inline() {
    // `compose_in_editor` set, but the editor can't start: fall back to the inline
    // widget (a spawn error is not a user cancellation) rather than silently
    // backing out.
    let backend = MockPromptBackend::new()
        .with_inline_responses(['r'])
        .with_reply_outcomes([ReplyOutcome::Submit("typed inline".into())]);
    let editor = MockEditorBackend::failing();
    let config = StreamingInterruptConfig {
        action: StreamingInterruptAction::Prompt,
        compose_in_editor: ComposeInEditor::Editor,
    };
    let action = handler_with_editor(backend, editor).handle_streaming_interrupt(
        &config,
        &make_printer(),
        true,
    );
    assert_eq!(action, InterruptAction::Reply("typed inline".into()));
}

#[test]
fn inline_editor_escape_spawn_failure_keeps_buffer() {
    // `r` -> Ctrl+X (OpenEditor) -> editor can't start -> the typed buffer is
    // kept and the widget re-prompts, so a second submit still sends the text
    // (rather than discarding it as a cancellation).
    let backend = MockPromptBackend::new()
        .with_inline_responses(['r'])
        .with_reply_outcomes([
            ReplyOutcome::OpenEditor {
                current_text: "draft".into(),
            },
            ReplyOutcome::Submit("draft, then more".into()),
        ]);
    let editor = MockEditorBackend::failing();
    let action = handler_with_editor(backend, editor).handle_streaming_interrupt(
        &streaming(StreamingInterruptAction::Prompt),
        &make_printer(),
        true,
    );
    assert_eq!(action, InterruptAction::Reply("draft, then more".into()));
}
