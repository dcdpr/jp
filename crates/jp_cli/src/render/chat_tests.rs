use jp_config::{AppConfig, style::typewriter::DelayDuration, types::color::Color};
use jp_printer::{OutputFormat, SharedBuffer};

use super::*;

/// Strip ANSI escape codes from a string for assertion comparisons.
fn strip_ansi(s: &str) -> String {
    let bytes = strip_ansi_escapes::strip(s);
    String::from_utf8(bytes).expect("valid utf-8 after stripping ANSI")
}

fn create_renderer_with_config(config: AppConfig) -> (ChatRenderer, SharedBuffer, SharedBuffer) {
    let (printer, out, err) = Printer::memory(OutputFormat::TextPretty);
    let renderer = ChatRenderer::new(Arc::new(printer), config.style);
    (renderer, out, err)
}

fn create_renderer() -> (ChatRenderer, SharedBuffer, SharedBuffer) {
    create_renderer_with_config(AppConfig::new_test())
}

#[test]
fn test_renders_message() {
    let (mut renderer, out, _err) = create_renderer();

    renderer.render_response(&ChatResponse::Message {
        message: "Hello world\n\n".into(),
    });

    renderer.printer.flush();
    assert_eq!(*out.lock(), "Hello world\n\n");
}

/// A streamed message ending in a tight list, then flushed (e.g. because a tool
/// call follows), must still emit its trailing blank-line separator.
/// The "Calling tool" header is chrome on stderr and emits no leading blank
/// line of its own, so the gap has to come from the flushed markdown.
#[test]
fn test_terminal_list_flush_emits_trailing_separator() {
    let mut config = AppConfig::new_test();
    config.style.reasoning.background = None;
    let (mut renderer, out, _err) = create_renderer_with_config(config);

    renderer.render_response(&ChatResponse::Message {
        message: "- first\n- second\n".into(),
    });
    renderer.flush();
    renderer.printer.flush();

    let rendered = strip_ansi(&out.lock());
    assert!(
        rendered.ends_with("\n\n"),
        "list-terminated message should end with a blank-line separator, got: {rendered:?}"
    );
}

#[test]
fn test_renders_reasoning_full_mode() {
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Full;
    config.style.reasoning.background = None;
    let (mut renderer, out, _err) = create_renderer_with_config(config);

    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "Let me think\n\n".into(),
    });

    renderer.flush();
    renderer.printer.flush();
    assert_eq!(*out.lock(), "Let me think\n\n");
}

#[test]
fn test_hidden_reasoning_produces_no_output() {
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Hidden;
    let (mut renderer, out, _err) = create_renderer_with_config(config);

    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "Secret thoughts\n\n".into(),
    });

    renderer.printer.flush();
    assert_eq!(*out.lock(), "");
}

#[test]
fn test_static_reasoning_shows_once() {
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Static;
    let (mut renderer, out, err) = create_renderer_with_config(config);

    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "First chunk\n\n".into(),
    });
    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "Second chunk\n\n".into(),
    });

    renderer.printer.flush();
    assert_eq!(
        *out.lock(),
        "",
        "static reasoning is chrome, not assistant output"
    );
    assert_eq!(*err.lock(), "reasoning...\n\n");
}

#[test]
fn test_progress_reasoning_shows_dots() {
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Progress;
    let (mut renderer, out, err) = create_renderer_with_config(config);

    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "First\n\n".into(),
    });
    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "Second\n\n".into(),
    });
    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "Third\n\n".into(),
    });

    renderer.printer.flush();
    assert_eq!(
        *out.lock(),
        "",
        "progress dots are chrome, not assistant output"
    );
    assert_eq!(*err.lock(), "reasoning.....");
}

#[tokio::test]
async fn test_timer_reasoning_suppresses_output() {
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Timer;
    let (mut renderer, out, _err) = create_renderer_with_config(config);

    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "First chunk\n\n".into(),
    });
    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "Second chunk\n\n".into(),
    });

    renderer.printer.flush();
    assert_eq!(
        *out.lock(),
        "",
        "timer reasoning should not produce stdout output"
    );
}

#[tokio::test]
async fn test_timer_reasoning_then_message() {
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Timer;
    let (mut renderer, out, _err) = create_renderer_with_config(config);

    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "Thinking hard\n\n".into(),
    });
    renderer.render_response(&ChatResponse::Message {
        message: "Answer\n\n".into(),
    });

    renderer.printer.flush();
    assert_eq!(
        *out.lock(),
        "Answer\n\n",
        "message content should render after timer reasoning"
    );
}

/// Regression: tool call → Timer reasoning → tool call must not leave a stray
/// blank line on stdout.
///
/// Timer reasoning is ephemeral chrome on stderr; it produces no persistent
/// stdout output.
/// The previous implementation routed Timer through `flush_on_transition`,
/// which eagerly committed a blank-line separator on stdout when leaving a
/// `ToolCall` block.
/// Subsequent tool calls (or other ephemeral content) never "earned" that
/// separator back, leaving an orphan blank line between consecutive tool calls.
#[tokio::test]
async fn test_no_separator_for_tool_call_timer_reasoning_tool_call() {
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Timer;
    let (mut renderer, out, _err) = create_renderer_with_config(config);

    // Tool call 1: chat renderer enters ToolCall mode. (The tool
    // renderer itself writes "Calling tool …" to stderr; nothing on
    // stdout from this side.)
    renderer.transition_to_tool_call();

    // Reasoning chunk under Timer style — no persistent stdout output.
    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "Thinking hard\n\n".into(),
    });

    // Tool call 2: the real flow flushes (cancelling the timer) before
    // re-entering ToolCall mode — mirror that here.
    renderer.flush();
    renderer.transition_to_tool_call();

    renderer.printer.flush();
    assert_eq!(
        *out.lock(),
        "",
        "ephemeral Timer reasoning between tool calls must not emit a stray separator"
    );
}

#[test]
fn test_truncate_reasoning() {
    let mut config = AppConfig::new_test();
    config.style.reasoning.display =
        ReasoningDisplayConfig::Truncate(TruncateChars { characters: 10 });
    config.style.reasoning.background = None;

    let (mut renderer, out, _err) = create_renderer_with_config(config);

    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "This is a very long reasoning that should be truncated\n\n".into(),
    });

    renderer.flush();
    renderer.printer.flush();
    assert_eq!(*out.lock(), "This is a ...\n\n");
}

#[test]
fn test_no_separator_between_reasoning_and_message() {
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Full;
    config.style.reasoning.background = None;
    let (mut renderer, out, _err) = create_renderer_with_config(config);

    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "Thinking\n\n".into(),
    });
    renderer.render_response(&ChatResponse::Message {
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
    let (mut renderer, out, _err) = create_renderer_with_config(config);

    // Reasoning without a trailing block boundary (no double newline)
    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "Partial reasoning".into(),
    });

    renderer.printer.flush();
    assert_eq!(*out.lock(), "", "Should not flush incomplete block yet");

    // Message arrives — should force-flush the buffered reasoning first
    renderer.render_response(&ChatResponse::Message {
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
    let (mut renderer, out, _err) = create_renderer();

    // Partial message with no block boundary
    renderer.render_response(&ChatResponse::Message {
        message: "Incomplete line".into(),
    });

    renderer.printer.flush();
    assert_eq!(*out.lock(), "");

    // Explicit flush forces remaining content out
    renderer.flush();
    renderer.printer.flush();
    assert!(
        out.lock().contains("Incomplete line"),
        "flush() should emit buffered content"
    );
}

#[test]
fn test_whitespace_only_block_not_printed() {
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Full;
    let (mut renderer, out, _err) = create_renderer_with_config(config);

    // Simulate Anthropic emitting "\n\n" before a thinking block
    renderer.render_response(&ChatResponse::Message {
        message: "\n\n".into(),
    });
    // Transition to reasoning triggers flush of the buffered "\n\n"
    renderer.render_response(&ChatResponse::Reasoning {
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
    let (mut renderer, out, _err) = create_renderer_with_config(config);

    renderer.render_response(&ChatResponse::Reasoning {
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
    let (mut renderer, out, _err) = create_renderer_with_config(config);

    renderer.render_response(&ChatResponse::Message {
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
fn test_reasoning_background_separator_unshaded_before_message() {
    // Regression: the blank line between reasoning and the following message
    // must not carry the reasoning background. The shading ends at the last
    // line with actual reasoning content.
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Full;
    config.style.reasoning.background = Some(Color::Ansi256(236));
    let (mut renderer, out, _err) = create_renderer_with_config(config);

    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "Thinking\n\n".into(),
    });
    renderer.render_response(&ChatResponse::Message {
        message: "Answer\n\n".into(),
    });
    renderer.flush();
    renderer.printer.flush();

    let output = out.lock().clone();
    assert!(
        output.contains("\x1b[48;5;236m"),
        "reasoning content should still be shaded, got: {output:?}"
    );
    // A shaded blank separator is `<bg><erase-to-EOL><reset-bg>`. With a single
    // reasoning paragraph the only gap is the one before the message, so none
    // should appear.
    assert!(
        !output.contains("\x1b[48;5;236m\x1b[K\x1b[49m"),
        "the separator before the message must be unshaded, got: {output:?}"
    );
}

#[test]
fn test_reasoning_background_shades_gap_between_paragraphs() {
    // Multi-paragraph reasoning stays a contiguous shaded region: the blank
    // line between two reasoning paragraphs keeps the background, while the gap
    // to the following message does not.
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Full;
    config.style.reasoning.background = Some(Color::Ansi256(236));
    let (mut renderer, out, _err) = create_renderer_with_config(config);

    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "First paragraph\n\nSecond paragraph\n\n".into(),
    });
    renderer.render_response(&ChatResponse::Message {
        message: "Answer\n\n".into(),
    });
    renderer.flush();
    renderer.printer.flush();

    let output = out.lock().clone();
    // Exactly one shaded separator: between the two reasoning paragraphs, not
    // after the last one.
    assert_eq!(
        output.matches("\x1b[48;5;236m\x1b[K\x1b[49m").count(),
        1,
        "expected one shaded inter-paragraph separator, got: {output:?}"
    );
    assert!(output.contains("First paragraph"), "got: {output:?}");
    assert!(output.contains("Second paragraph"), "got: {output:?}");
    assert!(output.contains("Answer"), "got: {output:?}");
}

#[test]
fn test_fenced_code_block_streams_without_double_fence() {
    let (mut renderer, out, _err) = create_renderer();

    // Simulate a fenced code block arriving in chunks
    renderer.render_response(&ChatResponse::Message {
        message: "```json\n".into(),
    });
    renderer.render_response(&ChatResponse::Message {
        message: "{\"key\": \"value\"}\n".into(),
    });
    renderer.render_response(&ChatResponse::Message {
        message: "```\n".into(),
    });
    renderer.flush();
    renderer.printer.flush();

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
    let (mut renderer, out, _err) = create_renderer();

    renderer.render_response(&ChatResponse::Message {
        message: "```rust\nfn main() {}\n```\n".into(),
    });
    renderer.flush();
    renderer.printer.flush();

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
fn test_code_block_without_trailing_newline_is_balanced() {
    let (mut renderer, out, _err) = create_renderer();

    // The common LLM shape: a message ending on its closing fence with no
    // trailing newline. The close previously fell into the flush path, got
    // re-parsed by comrak into a stray fence pair, and left the escalated
    // opening fence unmatched.
    renderer.render_response(&ChatResponse::Message {
        message: "```sh\necho hi\n```".into(),
    });
    renderer.flush();
    renderer.printer.flush();

    let plain = strip_ansi(&out.lock());
    assert!(
        plain.contains("echo hi"),
        "code content should render, got: {plain:?}"
    );
    // One escalated opening fence and one matching escalated closing fence.
    assert_eq!(
        plain.matches("`````").count(),
        2,
        "expected a matched pair of escalated fences, got: {plain:?}"
    );
    // No leftover comrak-generated bare fence pair.
    assert!(
        !plain.contains("```\n```"),
        "should not emit a duplicated 3-backtick fence pair, got: {plain:?}"
    );
}

#[test]
fn test_text_before_and_after_code_block() {
    let (mut renderer, out, _err) = create_renderer();

    renderer.render_response(&ChatResponse::Message {
        message: "Before\n\n```\ncode\n```\nAfter\n\n".into(),
    });
    renderer.flush();
    renderer.printer.flush();

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

/// Regression for two bugs in the fence-inside-list-item render path:
///
/// 1. Visible content in syntax-highlighted code lines was indented N columns
///    too far right, because `indent_lines` treated the syntect-appended
///    `\x1b[0m` (reset emitted *after* the trailing `\n`) as the start of a new
///    line and added an extra prefix to it.
/// 2. The closing fence inside a list item was followed by a spurious blank
///    line, breaking the visual flow of the surrounding list.
#[test]
fn test_fence_inside_list_item_indents_correctly_and_no_trailing_blank() {
    let mut config = AppConfig::new_test();
    config.style.markdown.theme = None;
    let (mut renderer, out, _err) = create_renderer_with_config(config);

    renderer.render_response(&ChatResponse::Message {
        message: "\
1. Workspace config grants:
   ```toml
   [[conversation.tools.fs_modify_file.access.fs]]
      path = \".\"
      read = true
   ```
2. Conversation adds a mount.
"
        .into(),
    });
    renderer.flush();
    renderer.printer.flush();

    let plain = strip_ansi(&out.lock());
    let lines: Vec<&str> = plain.lines().collect();

    // Code content lines inside the list item stay at the list's
    // content_column (3) + their own intra-block indent. The TOML
    // table content was at column 6 in the source; it must render at
    // column 6, not 9.
    assert!(
        lines.contains(&"      path = \".\""),
        "`path = \".\"` should render at column 6. Got:\n{plain}"
    );
    assert!(
        lines.contains(&"      read = true"),
        "`read = true` should render at column 6. Got:\n{plain}"
    );

    // Closing fence sits at the opening fence's column (3).
    assert!(
        lines.contains(&"   `````"),
        "closing fence should render at column 3. Got:\n{plain}"
    );

    // No blank line between the closing fence and the next list item.
    let fence_idx = lines
        .iter()
        .position(|l| *l == "   `````")
        .expect("closing fence missing");
    assert_eq!(
        lines.get(fence_idx + 1),
        Some(&"2. Conversation adds a mount."),
        "next list item should sit directly under the closing fence. Got:\n{plain}"
    );
}

#[test]
fn test_fenced_code_block_syntax_highlighting() {
    let mut config = AppConfig::new_test();
    config.style.markdown.theme = None;
    let (mut renderer, out, _err) = create_renderer_with_config(config);

    renderer.render_response(&ChatResponse::Message {
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
    renderer.printer.flush();

    let output = out.lock().clone();
    // Monokai Extended theme highlighting for the Rust snippet.
    //
    // Each line is highlighted individually by the streaming code path,
    // so each line ends with a \x1b[0m reset before the next line's
    // escape sequences begin.
    let expected = concat!(
        "`````rust\n",
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
        "`````\n",
        // render_closing_fence appends a block separator after the fence
        "\n",
    );
    assert_eq!(output, expected);
}

#[test]
fn test_no_separator_for_consecutive_messages() {
    let mut config = AppConfig::new_test();
    config.style.markdown.wrap_width = 0;
    let (mut renderer, out, _err) = create_renderer_with_config(config);

    renderer.render_response(&ChatResponse::Message {
        message: "First ".into(),
    });

    // Flush does not print anything, until a "block" is complete
    renderer.printer.flush();
    assert_eq!(*out.lock(), "");

    renderer.render_response(&ChatResponse::Message {
        message: " Second\n\n".into(),
    });

    // Flush prints the paragraph "block".
    // The double space between "First" and "Second" is preserved from
    // the source ("First " + " Second") — CommonMark doesn't collapse
    // interior spaces.
    renderer.printer.flush();
    assert_eq!(*out.lock(), "First  Second\n\n");
}

#[test]
fn test_blank_line_after_tool_calls_before_message() {
    let (mut renderer, out, _err) = create_renderer();

    renderer.render_response(&ChatResponse::Message {
        message: "Before tools\n\n".into(),
    });
    renderer.printer.flush();

    // Simulate tool calls being rendered between message chunks.
    // The turn loop calls set_tool_call_kind() when tool calls arrive.
    renderer.transition_to_tool_call();

    // Next message content should be separated by a blank line.
    renderer.render_response(&ChatResponse::Message {
        message: "After tools\n\n".into(),
    });
    renderer.printer.flush();

    let output = out.lock().clone();
    assert_eq!(output, "Before tools\n\n\nAfter tools\n\n");
}

#[test]
fn test_no_blank_line_for_consecutive_messages_without_tool_calls() {
    let (mut renderer, out, _err) = create_renderer();

    renderer.render_response(&ChatResponse::Message {
        message: "First paragraph\n\n".into(),
    });
    renderer.render_response(&ChatResponse::Message {
        message: "Second paragraph\n\n".into(),
    });
    renderer.printer.flush();

    let output = out.lock().clone();
    // No extra blank line between consecutive messages.
    assert_eq!(output, "First paragraph\n\nSecond paragraph\n\n");
}

/// Latency shape a streamed paragraph must exhibit, asserted alongside
/// byte-identity (which alone would pass a paragraph that never streams).
#[derive(Clone, Copy)]
enum Latency {
    /// Word-wrapped prose with an unambiguous lead: at least one committed line
    /// is printed before the terminator.
    Streams,
    /// Nothing is printed before the first source newline: an unbreakable run
    /// or non-wrapping paragraph the renderer cannot commit, or a single-line
    /// ambiguous-lead paragraph the buffer does not classify until its newline.
    Holds,
}

struct StreamingFixture {
    name: &'static str,
    /// The streaming part, fed before the terminator.
    body: &'static str,
    /// Everything fed after `body`: the terminator and any trailing blocks.
    rest: &'static str,
    wrap_width: usize,
    reasoning: bool,
    latency: Latency,
}

fn streaming_config(fx: &StreamingFixture) -> AppConfig {
    let mut config = AppConfig::new_test();
    config.style.markdown.wrap_width = fx.wrap_width;
    // Disable typewriter pacing: per-character feeding stays fast and output is
    // deterministic. Pacing only affects timing, never the emitted bytes.
    config.style.typewriter.text_delay = DelayDuration::instant();
    config.style.typewriter.code_delay = DelayDuration::instant();
    if fx.reasoning {
        config.style.reasoning.display = ReasoningDisplayConfig::Full;
        config.style.reasoning.background = Some(Color::Ansi256(236));
    } else {
        config.style.reasoning.background = None;
    }
    config
}

/// Feed `text` one character at a time, maximally fragmenting inline constructs
/// across input chunks.
fn feed_chars(renderer: &mut ChatRenderer, reasoning: bool, text: &str) {
    for ch in text.chars() {
        let piece = ch.to_string();
        if reasoning {
            renderer.render_response(&ChatResponse::Reasoning { reasoning: piece });
        } else {
            renderer.render_response(&ChatResponse::Message { message: piece });
        }
    }
}

/// Render `body + rest` in a single push.
/// With the terminator present the buffer emits a `Block`, never a
/// `ParagraphChunk`, so this is the non-streaming reference output.
fn render_whole(fx: &StreamingFixture) -> String {
    let (mut r, out, _e) = create_renderer_with_config(streaming_config(fx));
    let full = format!("{}{}", fx.body, fx.rest);
    if fx.reasoning {
        r.render_response(&ChatResponse::Reasoning { reasoning: full });
    } else {
        r.render_response(&ChatResponse::Message { message: full });
    }
    r.flush();
    r.printer.flush();
    out.lock().clone()
}

#[test]
#[expect(clippy::too_many_lines, reason = "flat fixture table")]
fn test_streaming_byte_identity_corpus() {
    let fixtures = [
        StreamingFixture {
            name: "plain_wrap",
            wrap_width: 40,
            reasoning: false,
            latency: Latency::Streams,
            rest: "\n\n",
            body: "This is a generously long paragraph of ordinary prose that comfortably crosses \
                   the streaming threshold and then keeps right on going so the renderer commits \
                   several wrapped lines well before the terminator ever arrives.",
        },
        StreamingFixture {
            name: "no_wrap",
            wrap_width: 0,
            reasoning: false,
            latency: Latency::Holds,
            rest: "\n\n",
            body: "This is a generously long paragraph of ordinary prose that comfortably crosses \
                   the streaming threshold but renders with wrapping disabled, so no visual line \
                   is ever committed before the terminator.",
        },
        StreamingFixture {
            name: "inline_code",
            wrap_width: 40,
            reasoning: false,
            latency: Latency::Streams,
            rest: "\n\n",
            body: "Here is a fair amount of leading prose to cross the threshold and then an \
                   `inline code span` followed by a good deal more trailing prose so the lines \
                   keep wrapping along.",
        },
        StreamingFixture {
            name: "strong",
            wrap_width: 40,
            reasoning: false,
            latency: Latency::Streams,
            rest: "\n\n",
            body: "Here is a fair amount of leading prose to cross the threshold and then a \
                   **strongly emphasized phrase** followed by a good deal more trailing prose so \
                   the lines keep wrapping.",
        },
        StreamingFixture {
            name: "link",
            wrap_width: 40,
            reasoning: false,
            latency: Latency::Streams,
            rest: "\n\n",
            body: "Here is a fair amount of leading prose to cross the threshold and then a \
                   [labelled link](https://example.com/path) followed by a good deal more \
                   trailing prose to keep going.",
        },
        StreamingFixture {
            name: "image",
            wrap_width: 40,
            reasoning: false,
            latency: Latency::Streams,
            rest: "\n\n",
            body: "Here is a fair amount of leading prose to cross the threshold and then an \
                   ![image alt](https://example.com/i.png) followed by a good deal more trailing \
                   prose to keep going.",
        },
        StreamingFixture {
            name: "superscript",
            wrap_width: 40,
            reasoning: false,
            latency: Latency::Streams,
            rest: "\n\n",
            body: "Here is a fair amount of leading prose to cross the threshold and then a \
                   superscript such as x^2^ sitting mid sentence followed by more trailing prose \
                   to keep wrapping.",
        },
        StreamingFixture {
            name: "subscript",
            wrap_width: 40,
            reasoning: false,
            latency: Latency::Streams,
            rest: "\n\n",
            body: "Here is a fair amount of leading prose to cross the threshold and then a \
                   subscript such as H~2~O sitting mid sentence followed by more trailing prose \
                   to keep wrapping.",
        },
        StreamingFixture {
            name: "orphaned_fence",
            wrap_width: 40,
            reasoning: false,
            latency: Latency::Streams,
            rest: "\n\n```\n\n",
            body: "Let me very carefully re-read the entire file from top to bottom before making \
                   any edit at all, here is exactly the command snippet that I am about to run \
                   for you:```rust",
        },
        StreamingFixture {
            name: "reasoning_bg",
            wrap_width: 40,
            reasoning: true,
            latency: Latency::Streams,
            rest: "\n\n",
            body: "Let me reason at some length about this particular problem in one long \
                   paragraph that runs well past the streaming threshold so the renderer must \
                   commit it line by line.",
        },
        StreamingFixture {
            name: "unbreakable_token",
            wrap_width: 40,
            reasoning: false,
            latency: Latency::Holds,
            rest: "\n\n",
            body: "Supercalifragilisticexpialidocioussupercalifragilisticexpialidocioussupercalif\
                   ragilisticexpialidocioussupercalifragilisticexpialidociousextrapaddinghere",
        },
        StreamingFixture {
            name: "long_url",
            wrap_width: 40,
            reasoning: false,
            latency: Latency::Holds,
            rest: "\n\n",
            body: "https://example.com/a/very/long/path/that/just/keeps/going/segment/after/segme\
                   nt/with/no/spaces/at/all/until/it/passes/both/the/threshold/and/the/width",
        },
        StreamingFixture {
            name: "lead_bracket",
            wrap_width: 40,
            reasoning: false,
            latency: Latency::Holds,
            rest: "\n\n",
            body: "[some-label] followed by a good amount of ordinary prose text that continues \
                   well past the streaming threshold on a single line so it is never classified \
                   early.",
        },
        StreamingFixture {
            name: "lead_angle",
            wrap_width: 40,
            reasoning: false,
            latency: Latency::Holds,
            rest: "\n\n",
            body: "<3 is the little symbol that opens this single long line of prose which then \
                   runs on well past the streaming threshold without ever wrapping or streaming \
                   early.",
        },
        StreamingFixture {
            name: "lead_digit",
            wrap_width: 40,
            reasoning: false,
            latency: Latency::Holds,
            rest: "\n\n",
            body: "100 distinct reasons are spread across this single long line of prose that runs \
                   on well past the streaming threshold so the buffer simply waits for its \
                   newline.",
        },
        StreamingFixture {
            name: "table_wide_later_row",
            wrap_width: 80,
            reasoning: false,
            latency: Latency::Holds,
            rest: "\n\n",
            // A GFM table is not prefix-stable: the wide cell on the fifth row
            // re-pads the columns of the header and earlier rows. It must stay
            // on the whole-block path, never streaming, or byte-identity breaks.
            body: concat!(
                "| Name | Value |\n",
                "| ---- | ----- |\n",
                "| a | 1 |\n",
                "| bb | 22 |\n",
                "| a very wide cell that widens this column well beyond the header | 333 |\n",
                "| c | 4 |",
            ),
        },
    ];

    for fx in &fixtures {
        let whole = render_whole(fx);

        let (mut streamed, out, _e) = create_renderer_with_config(streaming_config(fx));
        feed_chars(&mut streamed, fx.reasoning, fx.body);
        streamed.printer.flush();
        let before_terminator = out.lock().clone();
        match fx.latency {
            Latency::Streams => assert!(
                !before_terminator.is_empty(),
                "{}: expected committed output before the terminator, got nothing",
                fx.name
            ),
            Latency::Holds => assert!(
                before_terminator.is_empty(),
                "{}: expected nothing before the first source newline, got: {before_terminator:?}",
                fx.name
            ),
        }

        feed_chars(&mut streamed, fx.reasoning, fx.rest);
        streamed.flush();
        streamed.printer.flush();

        assert_eq!(*out.lock(), whole, "byte-identity failed for {}", fx.name);
    }
}

#[test]
fn test_streaming_ambiguous_lead_streams_after_first_newline() {
    // An ambiguous block-start lead (`[`) is not classified as a paragraph
    // until its first source newline: nothing streams before that newline, but
    // the paragraph streams normally afterward. This pins the precise boundary
    // of the documented limitation — it is the first newline, not a wholesale
    // failure to stream.
    let mut config = AppConfig::new_test();
    config.style.reasoning.background = None;
    config.style.markdown.wrap_width = 40;
    config.style.typewriter.text_delay = DelayDuration::instant();
    config.style.typewriter.code_delay = DelayDuration::instant();

    let first_line = "[ref] this opening line begins with an ambiguous bracket lead and runs long \
                      enough to comfortably exceed the streaming threshold all by itself here";
    let rest = "and this continues the very same paragraph across a second line of prose that \
                itself wraps several times before the paragraph finally ends.";

    let (mut r, out, _e) = create_renderer_with_config(config.clone());

    feed_chars(&mut r, false, first_line);
    r.printer.flush();
    let before_newline = out.lock().clone();
    assert!(
        before_newline.is_empty(),
        "nothing should stream before the first source newline, got: {before_newline:?}"
    );

    feed_chars(&mut r, false, &format!("\n{rest}"));
    r.printer.flush();
    let after_newline = out.lock().clone();
    assert!(
        !after_newline.is_empty(),
        "the paragraph should stream after its first source newline"
    );

    feed_chars(&mut r, false, "\n\n");
    r.flush();
    r.printer.flush();
    let streamed = out.lock().clone();

    let (mut w, out_w, _e) = create_renderer_with_config(config);
    w.render_response(&ChatResponse::Message {
        message: format!("{first_line}\n{rest}\n\n"),
    });
    w.flush();
    w.printer.flush();

    assert_eq!(streamed, *out_w.lock());
}

#[test]
fn test_streaming_byte_identity_documents() {
    // Whole multi-block documents: long paragraphs (which stream) interspersed
    // with headings, lists, and fenced code (which do not). Streaming a document
    // character by character must produce the same bytes as rendering it whole.
    let documents = [
        (
            "heading_para_list_para",
            concat!(
                "# Section Heading\n",
                "\n",
                "This is the first long paragraph of the document and it runs comfortably ",
                "past the streaming threshold so it streams as chunks while it is fed in.\n",
                "\n",
                "- first list item\n",
                "- second list item\n",
                "- third list item\n",
                "\n",
                "And here is a second long paragraph that also exceeds the threshold so it ",
                "likewise streams in pieces rather than waiting for its terminator to arrive.\n",
                "\n",
            ),
        ),
        (
            "para_code_para",
            concat!(
                "Here is a long introductory paragraph that comfortably exceeds the streaming ",
                "threshold and therefore streams in chunks before the fenced code block below.\n",
                "\n",
                "```rust\n",
                "fn main() {\n",
                "    println!(\"hello\");\n",
                "}\n",
                "```\n",
                "\n",
                "And a closing long paragraph after the code block that also exceeds the ",
                "threshold so it streams in pieces just like the introduction did up above.\n",
                "\n",
            ),
        ),
    ];

    for (name, doc) in documents {
        let mut config = AppConfig::new_test();
        config.style.reasoning.background = None;
        config.style.markdown.wrap_width = 40;
        config.style.typewriter.text_delay = DelayDuration::instant();
        config.style.typewriter.code_delay = DelayDuration::instant();

        let (mut whole, out_whole, _e) = create_renderer_with_config(config.clone());
        whole.render_response(&ChatResponse::Message {
            message: doc.to_string(),
        });
        whole.flush();
        whole.printer.flush();

        let (mut streamed, out_streamed, _e) = create_renderer_with_config(config);
        feed_chars(&mut streamed, false, doc);
        streamed.flush();
        streamed.printer.flush();

        assert_eq!(
            *out_streamed.lock(),
            *out_whole.lock(),
            "byte-identity failed for document {name}"
        );
    }
}

#[test]
fn test_enter_tool_call_after_reasoning_shades_separator_and_returns_background() {
    // A tool call whose immediately preceding chat response was reasoning
    // continues the reasoning region: the deferred separator before it is
    // shaded, and the region background is returned for the chrome to extend.
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Full;
    config.style.reasoning.background = Some(Color::Ansi256(236));
    let (mut renderer, out, _err) = create_renderer_with_config(config);

    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "Thinking\n\n".into(),
    });
    let background = renderer.enter_tool_call();
    renderer.printer.flush();

    assert!(
        background.is_some(),
        "a tool call after reasoning continues the shaded region"
    );
    let output = out.lock().clone();
    assert_eq!(
        output.matches("\x1b[48;5;236m\x1b[K\x1b[49m").count(),
        1,
        "the deferred separator before the tool call should be shaded, got: {output:?}"
    );
}

#[test]
fn test_enter_tool_call_after_message_returns_none_and_stays_unshaded() {
    // A tool call after ordinary message content does not continue a reasoning
    // region, so there is nothing to shade and no background to extend.
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Full;
    config.style.reasoning.background = Some(Color::Ansi256(236));
    let (mut renderer, out, _err) = create_renderer_with_config(config);

    renderer.render_response(&ChatResponse::Message {
        message: "Answer\n\n".into(),
    });
    let background = renderer.enter_tool_call();
    renderer.printer.flush();

    assert!(
        background.is_none(),
        "a tool call after a message does not continue a reasoning region"
    );
    let output = out.lock().clone();
    assert!(
        !output.contains("\x1b[48;5;236m"),
        "a message and the following tool boundary must not be shaded, got: {output:?}"
    );
}

#[test]
fn test_reasoning_region_survives_tool_call_for_following_tool() {
    // Entering tool-call mode must not erase the memory that the region is
    // reasoning: a second back-to-back tool call still continues the region.
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Full;
    config.style.reasoning.background = Some(Color::Ansi256(236));
    let (mut renderer, _out, _err) = create_renderer_with_config(config);

    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "Thinking\n\n".into(),
    });
    let first = renderer.enter_tool_call();
    let second = renderer.enter_tool_call();

    assert!(
        first.is_some(),
        "first tool call continues the reasoning region"
    );
    assert!(
        second.is_some(),
        "a second tool call still continues the region; the transition into tool-call mode must \
         not clobber the last chat-response kind"
    );
}

#[test]
fn test_enter_tool_call_after_reasoning_without_background_returns_none() {
    // With no reasoning background configured there is no fill to extend, even
    // though the tool call follows reasoning.
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Full;
    config.style.reasoning.background = None;
    let (mut renderer, _out, _err) = create_renderer_with_config(config);

    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "Thinking\n\n".into(),
    });

    assert!(
        renderer.enter_tool_call().is_none(),
        "no reasoning background means no region fill to extend"
    );
}

#[test]
fn test_extend_across_tool_calls_disabled_ends_the_region_at_the_tool_call() {
    // With the flag off, a tool call after reasoning does not continue the
    // region: the separator before it is unshaded and no chrome background is
    // returned, restoring the per-block behaviour. The reasoning content itself
    // stays shaded — only the *extension* is gated.
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Full;
    config.style.reasoning.background = Some(Color::Ansi256(236));
    config.style.reasoning.extend_across_tool_calls = false;
    let (mut renderer, out, _err) = create_renderer_with_config(config);

    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "Thinking\n\n".into(),
    });
    let background = renderer.enter_tool_call();
    renderer.printer.flush();

    assert!(
        background.is_none(),
        "with the extension disabled the tool call does not continue the region"
    );
    let output = out.lock().clone();
    assert!(
        output.contains("\x1b[48;5;236m"),
        "the reasoning content itself is still shaded, got: {output:?}"
    );
    assert!(
        !output.contains("\x1b[48;5;236m\x1b[K\x1b[49m"),
        "the separator before the tool call must be unshaded, got: {output:?}"
    );
}
