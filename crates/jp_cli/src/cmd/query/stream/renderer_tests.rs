use jp_config::{AppConfig, types::color::Color};
use jp_printer::{OutputFormat, SharedBuffer};

use super::*;

/// Strip ANSI escape codes from a string for assertion comparisons.
fn strip_ansi(s: &str) -> String {
    let bytes = strip_ansi_escapes::strip(s);
    String::from_utf8(bytes).expect("valid utf-8 after stripping ANSI")
}

fn create_renderer_with_config(config: AppConfig) -> (ChatResponseRenderer, SharedBuffer) {
    let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
    let renderer = ChatResponseRenderer::new(Arc::new(printer), config.style);
    (renderer, out)
}

fn create_renderer() -> (ChatResponseRenderer, SharedBuffer, Printer) {
    let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
    let config = AppConfig::new_test().style;
    let renderer = ChatResponseRenderer::new(Arc::new(printer.clone()), config);
    (renderer, out, printer)
}

#[test]
fn test_renders_message() {
    let (mut renderer, out, printer) = create_renderer();

    renderer.render(&ChatResponse::Message {
        message: "Hello world\n\n".into(),
    });

    printer.flush();
    assert_eq!(*out.lock(), "Hello world\n\n");
}

#[test]
fn test_renders_reasoning_full_mode() {
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Full;
    let (mut renderer, out) = create_renderer_with_config(config);

    renderer.render(&ChatResponse::Reasoning {
        reasoning: "Let me think\n\n".into(),
    });

    renderer.printer.flush();
    assert_eq!(*out.lock(), "Let me think\n\n");
}

#[test]
fn test_hidden_reasoning_produces_no_output() {
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Hidden;
    let (mut renderer, out) = create_renderer_with_config(config);

    renderer.render(&ChatResponse::Reasoning {
        reasoning: "Secret thoughts\n\n".into(),
    });

    renderer.printer.flush();
    assert_eq!(*out.lock(), "");
}

#[test]
fn test_static_reasoning_shows_once() {
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Static;
    let (mut renderer, out) = create_renderer_with_config(config);

    renderer.render(&ChatResponse::Reasoning {
        reasoning: "First chunk\n\n".into(),
    });
    renderer.render(&ChatResponse::Reasoning {
        reasoning: "Second chunk\n\n".into(),
    });

    renderer.printer.flush();
    assert_eq!(*out.lock(), "reasoning...\n\n");
}

#[test]
fn test_progress_reasoning_shows_dots() {
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Progress;
    let (mut renderer, out) = create_renderer_with_config(config);

    renderer.render(&ChatResponse::Reasoning {
        reasoning: "First\n\n".into(),
    });
    renderer.render(&ChatResponse::Reasoning {
        reasoning: "Second\n\n".into(),
    });
    renderer.render(&ChatResponse::Reasoning {
        reasoning: "Third\n\n".into(),
    });

    renderer.printer.flush();
    assert_eq!(*out.lock(), "reasoning.....");
}

#[test]
fn test_truncate_reasoning() {
    let mut config = AppConfig::new_test();
    config.style.reasoning.display =
        ReasoningDisplayConfig::Truncate(TruncateChars { characters: 10 });

    let (mut renderer, out) = create_renderer_with_config(config);

    renderer.render(&ChatResponse::Reasoning {
        reasoning: "This is a very long reasoning that should be truncated\n\n".into(),
    });

    renderer.printer.flush();
    assert_eq!(*out.lock(), "This is a ...\n\n");
}

#[test]
fn test_no_separator_between_reasoning_and_message() {
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Full;
    config.style.reasoning.background = None;
    let (mut renderer, out) = create_renderer_with_config(config);

    renderer.render(&ChatResponse::Reasoning {
        reasoning: "Thinking\n\n".into(),
    });
    renderer.render(&ChatResponse::Message {
        message: "Answer\n\n".into(),
    });

    renderer.printer.flush();
    // No separator — background color distinguishes reasoning from message.
    assert_eq!(*out.lock(), "Thinking\n\nAnswer\n\n");
}

#[test]
fn test_reasoning_buffer_flushed_on_message_transition() {
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Full;
    config.style.reasoning.background = None;
    let (mut renderer, out) = create_renderer_with_config(config);

    // Reasoning without a trailing block boundary (no double newline)
    renderer.render(&ChatResponse::Reasoning {
        reasoning: "Partial reasoning".into(),
    });

    renderer.printer.flush();
    assert_eq!(*out.lock(), "", "Should not flush incomplete block yet");

    // Message arrives — should force-flush the buffered reasoning first
    renderer.render(&ChatResponse::Message {
        message: "Answer\n\n".into(),
    });

    renderer.printer.flush();
    let output = out.lock().clone();
    assert!(
        output.starts_with("Partial reasoning"),
        "Buffered reasoning should be flushed before message, got: {output:?}"
    );
    assert!(output.contains("Answer"), "Message content should follow");
}

#[test]
fn test_message_buffer_flushed_on_explicit_flush() {
    let (mut renderer, out, printer) = create_renderer();

    // Partial message with no block boundary
    renderer.render(&ChatResponse::Message {
        message: "Incomplete line".into(),
    });

    printer.flush();
    assert_eq!(*out.lock(), "");

    // Explicit flush forces remaining content out
    renderer.flush();
    printer.flush();
    assert!(
        out.lock().contains("Incomplete line"),
        "flush() should emit buffered content"
    );
}

#[test]
fn test_whitespace_only_block_not_printed() {
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Full;
    let (mut renderer, out) = create_renderer_with_config(config);

    // Simulate Anthropic emitting "\n\n" before a thinking block
    renderer.render(&ChatResponse::Message {
        message: "\n\n".into(),
    });
    // Transition to reasoning triggers flush of the buffered "\n\n"
    renderer.render(&ChatResponse::Reasoning {
        reasoning: "Thinking about it\n\n".into(),
    });

    renderer.printer.flush();
    let output = out.lock().clone();
    // The "\n\n" should not produce any output; only reasoning appears
    assert!(
        !output.starts_with('\n'),
        "Whitespace-only block should be suppressed, got: {output:?}"
    );
    assert!(
        output.contains("Thinking about it"),
        "Reasoning content should still render"
    );
}

#[test]
fn test_reasoning_background_color_applied() {
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Full;
    config.style.reasoning.background = Some(Color::Ansi256(236));
    let (mut renderer, out) = create_renderer_with_config(config);

    renderer.render(&ChatResponse::Reasoning {
        reasoning: "Deep thought\n\n".into(),
    });

    renderer.printer.flush();
    let output = out.lock().clone();
    assert!(
        output.contains("\x1b[48;5;236m"),
        "Reasoning should have background color set, got: {output:?}"
    );
    assert!(
        output.contains("\x1b[K"),
        "Lines should use erase-to-EOL for full-width background, got: {output:?}"
    );
    assert!(
        output.contains("Deep thought"),
        "Content should still be present"
    );
}

#[test]
fn test_reasoning_background_not_applied_to_messages() {
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Full;
    config.style.reasoning.background = Some(Color::Ansi256(236));
    let (mut renderer, out) = create_renderer_with_config(config);

    renderer.render(&ChatResponse::Message {
        message: "Plain answer\n\n".into(),
    });

    renderer.printer.flush();
    let output = out.lock().clone();
    assert!(
        !output.contains("\x1b[48;5;236m"),
        "Message should not have reasoning background, got: {output:?}"
    );
}

#[test]
fn test_fenced_code_block_streams_without_double_fence() {
    let (mut renderer, out, printer) = create_renderer();

    // Simulate a fenced code block arriving in chunks
    renderer.render(&ChatResponse::Message {
        message: "```json\n".into(),
    });
    renderer.render(&ChatResponse::Message {
        message: "{\"key\": \"value\"}\n".into(),
    });
    renderer.render(&ChatResponse::Message {
        message: "```\n".into(),
    });
    renderer.flush();
    printer.flush();

    let output = out.lock().clone();
    let plain = strip_ansi(&output);
    // The opening fence should appear exactly once.
    assert_eq!(
        plain.matches("```").count(),
        2,
        "Should have exactly one opening and one closing fence, got: {plain:?}"
    );
    assert!(
        plain.contains("{\"key\": \"value\"}"),
        "Code content should be present, got: {plain:?}"
    );
}

#[test]
fn test_fenced_code_block_with_language_tag() {
    let (mut renderer, out, printer) = create_renderer();

    renderer.render(&ChatResponse::Message {
        message: "```rust\nfn main() {}\n```\n".into(),
    });
    renderer.flush();
    printer.flush();

    let output = out.lock().clone();
    let plain = strip_ansi(&output);
    assert!(
        plain.contains("```rust"),
        "Opening fence with language should be present, got: {plain:?}"
    );
    assert!(
        plain.contains("fn main()"),
        "Code content should be present, got: {plain:?}"
    );
}

#[test]
fn test_text_before_and_after_code_block() {
    let (mut renderer, out, printer) = create_renderer();

    renderer.render(&ChatResponse::Message {
        message: "Before\n\n```\ncode\n```\nAfter\n\n".into(),
    });
    renderer.flush();
    printer.flush();

    let output = out.lock().clone();
    assert!(
        output.contains("Before"),
        "Text before code block should render, got: {output:?}"
    );
    assert!(
        output.contains("code"),
        "Code content should render, got: {output:?}"
    );
    assert!(
        output.contains("After"),
        "Text after code block should render, got: {output:?}"
    );
}

#[test]
fn test_fenced_code_block_syntax_highlighting() {
    let (mut renderer, out, printer) = create_renderer();

    renderer.render(&ChatResponse::Message {
        message: indoc::indoc! {"
            ```rust
            fn main() {
                println!(\"test\");
            }
            ```
        "}
        .into(),
    });
    renderer.flush();
    printer.flush();

    let output = out.lock().clone();
    // Monokai Extended theme highlighting for the Rust snippet.
    //
    // Each line is highlighted individually by the streaming code path,
    // so each line ends with a \x1b[0m reset before the next line's
    // escape sequences begin.
    let expected = concat!(
        "```rust\n",
        "\x1b[38;2;102;217;239mfn",
        "\x1b[38;2;248;248;242m ",
        "\x1b[38;2;166;226;46mmain",
        "\x1b[38;2;248;248;242m(",
        "\x1b[38;2;248;248;242m)",
        "\x1b[38;2;248;248;242m ",
        "\x1b[38;2;248;248;242m{",
        "\x1b[38;2;248;248;242m\n",
        "\x1b[0m",
        "\x1b[38;2;248;248;242m    ",
        "\x1b[38;2;248;248;242mprintln!",
        "\x1b[38;2;248;248;242m(",
        "\x1b[38;2;230;219;116m\"",
        "\x1b[38;2;230;219;116mtest",
        "\x1b[38;2;230;219;116m\"",
        "\x1b[38;2;248;248;242m)",
        "\x1b[38;2;248;248;242m;",
        "\x1b[38;2;248;248;242m\n",
        "\x1b[0m",
        "\x1b[38;2;248;248;242m}",
        "\x1b[38;2;248;248;242m\n",
        "\x1b[0m",
        "```\n",
    );
    assert_eq!(output, expected);
}

#[test]
fn test_no_separator_for_consecutive_messages() {
    let (mut renderer, out, printer) = create_renderer();

    renderer.render(&ChatResponse::Message {
        message: "First ".into(),
    });

    // Flush does not print anything, until a "block" is complete
    printer.flush();
    assert_eq!(*out.lock(), "");

    renderer.render(&ChatResponse::Message {
        message: " Second\n\n".into(),
    });

    // Flush prints the paragraph "block".
    // The double space between "First" and "Second" is preserved from
    // the source ("First " + " Second") — CommonMark doesn't collapse
    // interior spaces.
    printer.flush();
    assert_eq!(*out.lock(), "First  Second\n\n");
}

#[test]
fn test_blank_line_after_tool_calls_before_message() {
    let (mut renderer, out, printer) = create_renderer();

    renderer.render(&ChatResponse::Message {
        message: "Before tools\n\n".into(),
    });
    printer.flush();

    // Simulate tool calls being rendered between message chunks.
    // The turn loop calls set_tool_call_kind() when tool calls arrive.
    renderer.transition_to_tool_call();

    // Next message content should be separated by a blank line.
    renderer.render(&ChatResponse::Message {
        message: "After tools\n\n".into(),
    });
    printer.flush();

    let output = out.lock().clone();
    assert_eq!(output, "Before tools\n\n\nAfter tools\n\n");
}

#[test]
fn test_no_blank_line_for_consecutive_messages_without_tool_calls() {
    let (mut renderer, out, printer) = create_renderer();

    renderer.render(&ChatResponse::Message {
        message: "First paragraph\n\n".into(),
    });
    renderer.render(&ChatResponse::Message {
        message: "Second paragraph\n\n".into(),
    });
    printer.flush();

    let output = out.lock().clone();
    // No extra blank line between consecutive messages.
    assert_eq!(output, "First paragraph\n\nSecond paragraph\n\n");
}
