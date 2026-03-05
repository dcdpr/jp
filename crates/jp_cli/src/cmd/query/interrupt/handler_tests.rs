use jp_inquire::prompt::MockPromptBackend;
use jp_printer::{OutputFormat, Printer};

use super::*;

fn make_printer() -> Printer {
    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    printer
}

#[test]
fn test_streaming_interrupt_stop() {
    let backend = MockPromptBackend::new().with_inline_responses(['s']);
    let handler = InterruptHandler::with_backend(backend);
    let printer = make_printer();
    let mut writer = printer.out_writer();

    let action = handler.handle_streaming_interrupt(&mut writer, true);
    assert_eq!(action, InterruptAction::Stop);
}

#[test]
fn test_streaming_interrupt_abort() {
    let backend = MockPromptBackend::new().with_inline_responses(['a']);
    let handler = InterruptHandler::with_backend(backend);
    let printer = make_printer();
    let mut writer = printer.out_writer();

    let action = handler.handle_streaming_interrupt(&mut writer, true);
    assert_eq!(action, InterruptAction::Abort);
}

#[test]
fn test_streaming_interrupt_reply() {
    let backend = MockPromptBackend::new()
        .with_inline_responses(['r'])
        .with_text_responses(["my reply message"]);
    let handler = InterruptHandler::with_backend(backend);
    let printer = make_printer();
    let mut writer = printer.out_writer();

    let action = handler.handle_streaming_interrupt(&mut writer, true);
    assert_eq!(action, InterruptAction::Reply("my reply message".into()));
}

#[test]
fn test_streaming_interrupt_reply_empty_on_cancel() {
    // No text response - simulates user canceling the text input
    let backend = MockPromptBackend::new().with_inline_responses(['r']);
    let handler = InterruptHandler::with_backend(backend);
    let printer = make_printer();
    let mut writer = printer.out_writer();

    let action = handler.handle_streaming_interrupt(&mut writer, true);
    assert_eq!(action, InterruptAction::Reply(String::new()));
}

#[test]
fn test_streaming_interrupt_continue_stream_alive() {
    let backend = MockPromptBackend::new().with_inline_responses(['c']);
    let handler = InterruptHandler::with_backend(backend);
    let printer = make_printer();
    let mut writer = printer.out_writer();

    let action = handler.handle_streaming_interrupt(&mut writer, true);
    assert_eq!(action, InterruptAction::Resume);
}

#[test]
fn test_streaming_interrupt_continue_stream_dead() {
    let backend = MockPromptBackend::new().with_inline_responses(['c']);
    let handler = InterruptHandler::with_backend(backend);
    let printer = make_printer();
    let mut writer = printer.out_writer();

    let action = handler.handle_streaming_interrupt(&mut writer, false);
    assert_eq!(action, InterruptAction::Continue);
}

#[test]
fn test_streaming_interrupt_defaults_to_stop_on_error() {
    // No responses - will error and default to Stop
    let backend = MockPromptBackend::new();
    let handler = InterruptHandler::with_backend(backend);
    let printer = make_printer();
    let mut writer = printer.out_writer();

    let action = handler.handle_streaming_interrupt(&mut writer, true);
    assert_eq!(action, InterruptAction::Stop);
}

#[test]
fn test_tool_interrupt_stop_with_custom_response() {
    let backend = MockPromptBackend::new()
        .with_inline_responses(['s'])
        .with_text_responses(["don't run this tool"]);
    let handler = InterruptHandler::with_backend(backend);
    let printer = make_printer();
    let mut writer = printer.out_writer();

    let action = handler.handle_tool_interrupt(&mut writer);
    assert_eq!(action, InterruptAction::ToolCancelled {
        response: "don't run this tool".into()
    });
}

#[test]
fn test_tool_interrupt_stop_empty_uses_canned_response() {
    // No text response — simulates user pressing Enter on empty input
    let backend = MockPromptBackend::new().with_inline_responses(['s']);
    let handler = InterruptHandler::with_backend(backend);
    let printer = make_printer();
    let mut writer = printer.out_writer();

    let action = handler.handle_tool_interrupt(&mut writer);
    assert!(
        matches!(action, InterruptAction::ToolCancelled { response } if response.contains("intentionally rejected"))
    );
}

#[test]
fn test_tool_interrupt_restart() {
    let backend = MockPromptBackend::new().with_inline_responses(['r']);
    let handler = InterruptHandler::with_backend(backend);
    let printer = make_printer();
    let mut writer = printer.out_writer();

    let action = handler.handle_tool_interrupt(&mut writer);
    assert_eq!(action, InterruptAction::RestartTool);
}

#[test]
fn test_tool_interrupt_continue() {
    let backend = MockPromptBackend::new().with_inline_responses(['c']);
    let handler = InterruptHandler::with_backend(backend);
    let printer = make_printer();
    let mut writer = printer.out_writer();

    let action = handler.handle_tool_interrupt(&mut writer);
    assert_eq!(action, InterruptAction::Resume);
}

#[test]
fn test_tool_interrupt_defaults_to_continue_on_error() {
    // No responses - will error and default to Continue
    let backend = MockPromptBackend::new();
    let handler = InterruptHandler::with_backend(backend);
    let printer = make_printer();
    let mut writer = printer.out_writer();

    let action = handler.handle_tool_interrupt(&mut writer);
    assert_eq!(action, InterruptAction::Resume);
}
