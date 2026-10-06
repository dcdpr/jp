use jp_printer::OutputFormat;

use super::*;

fn create_renderer() -> (StructuredRenderer, jp_printer::SharedBuffer, Printer) {
    create_sanitizing_renderer(SanitizeMode::Strip)
}

/// A renderer showing structured output the way `mode` asks.
fn create_sanitizing_renderer(
    mode: SanitizeMode,
) -> (StructuredRenderer, jp_printer::SharedBuffer, Printer) {
    let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
    let renderer = StructuredRenderer::new(Arc::new(printer.clone()), mode);
    (renderer, out, printer)
}

#[test]
fn renders_json_in_code_fence() {
    let (mut renderer, out, printer) = create_renderer();

    renderer.render_chunk(&ChatResponse::Structured {
        data: Value::String("{\"name\"".into()),
    });
    renderer.render_chunk(&ChatResponse::Structured {
        data: Value::String(": \"Alice\"}".into()),
    });
    renderer.flush();
    printer.flush();

    assert_eq!(*out.lock(), "```json\n{\"name\": \"Alice\"}\n```\n");
}

#[test]
fn flush_without_chunks_is_noop() {
    let (mut renderer, out, printer) = create_renderer();

    renderer.flush();
    printer.flush();

    assert_eq!(*out.lock(), "");
}

#[test]
fn ignores_non_structured_variants() {
    let (mut renderer, out, printer) = create_renderer();

    renderer.render_chunk(&ChatResponse::Message {
        message: "hello".into(),
    });
    renderer.flush();
    printer.flush();

    assert_eq!(*out.lock(), "");
}

#[test]
fn pretty_prints_parsed_value() {
    let (mut renderer, out, printer) = create_renderer();

    renderer.render_chunk(&ChatResponse::Structured {
        data: serde_json::json!({"name": "Alice", "age": 30}),
    });
    renderer.flush();
    printer.flush();

    let output = out.lock().clone();
    assert!(output.starts_with("```json\n"), "got: {output:?}");
    assert!(output.ends_with("\n```\n"), "got: {output:?}");
    assert!(output.contains("\"name\": \"Alice\""), "got: {output:?}");
    assert!(output.contains("\"age\": 30"), "got: {output:?}");
}

#[test]
fn reset_allows_new_code_fence() {
    let (mut renderer, out, printer) = create_renderer();

    renderer.render_chunk(&ChatResponse::Structured {
        data: Value::String("{}".into()),
    });
    renderer.flush();

    // Reset and render again — should produce a second code fence
    renderer.reset();
    renderer.render_chunk(&ChatResponse::Structured {
        data: Value::String("[1,2]".into()),
    });
    renderer.flush();
    printer.flush();

    assert_eq!(*out.lock(), "```json\n{}\n```\n```json\n[1,2]\n```\n");
}

#[test]
fn a_replayed_string_value_cannot_color_the_terminal() {
    // A schema asking for a string, or a response that failed to parse, is
    // stored as a string holding the escape the model wrote.
    let (mut renderer, out, printer) = create_renderer();

    renderer.render_chunk(&ChatResponse::Structured {
        data: Value::String("\x1b[31mred\x1b[0m".into()),
    });
    renderer.flush();
    printer.flush();

    assert_eq!(*out.lock(), "```json\nred\n```\n");
}

#[test]
fn a_sequence_split_across_streamed_chunks_is_recognized_whole() {
    let (mut renderer, out, printer) = create_renderer();

    renderer.render_chunk(&ChatResponse::Structured {
        data: Value::String("{\"a\": \"x\x1b[".into()),
    });
    renderer.render_chunk(&ChatResponse::Structured {
        data: Value::String("2Jy\"}".into()),
    });
    renderer.flush();
    printer.flush();

    assert_eq!(*out.lock(), "```json\n{\"a\": \"xy\"}\n```\n");
}

#[test]
fn an_unfinished_sequence_is_marked_before_the_fence_closes() {
    // Held back until the stream ends, then dropped rather than left to
    // swallow the closing fence.
    let (mut renderer, out, printer) = create_sanitizing_renderer(SanitizeMode::Visualize);

    renderer.render_chunk(&ChatResponse::Structured {
        data: Value::String("a\x1b[".into()),
    });
    renderer.flush();
    printer.flush();

    assert_eq!(*out.lock(), "```json\na\u{241b}\n```\n");
}

#[test]
fn a_parsed_value_loses_the_controls_json_leaves_raw() {
    // JSON escapes C0 characters in a string, but not DEL or C1, and a
    // terminal reads U+009B as the start of a control sequence.
    let (mut renderer, out, printer) = create_renderer();

    renderer.render_chunk(&ChatResponse::Structured {
        data: serde_json::json!({"x": "a\u{9b}2Jb\x7f"}),
    });
    renderer.flush();
    printer.flush();

    assert_eq!(*out.lock(), "```json\n{\n  \"x\": \"a2Jb\"\n}\n```\n");
}

#[test]
fn off_shows_structured_output_as_written() {
    let (mut renderer, out, printer) = create_sanitizing_renderer(SanitizeMode::Off);

    renderer.render_chunk(&ChatResponse::Structured {
        data: Value::String("\x1b[31mred".into()),
    });
    renderer.flush();
    printer.flush();

    assert_eq!(*out.lock(), "```json\n\x1b[31mred\n```\n");
}

#[test]
fn reset_discards_an_unfinished_sequence() {
    // The next stream starts clean instead of finishing the old sequence.
    let (mut renderer, out, printer) = create_renderer();

    renderer.render_chunk(&ChatResponse::Structured {
        data: Value::String("a\x1b[".into()),
    });
    renderer.reset();
    renderer.render_chunk(&ChatResponse::Structured {
        data: Value::String("2Jb".into()),
    });
    renderer.flush();
    printer.flush();

    assert_eq!(*out.lock(), "```json\na```json\n2Jb\n```\n");
}
