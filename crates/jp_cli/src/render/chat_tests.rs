use jp_config::{AppConfig, style::typewriter::DelayDuration, types::color::Color};
use jp_printer::{OutputFormat, OutputWidth, SharedBuffer};

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
fn wrap_width_is_capped_by_the_available_columns() {
    // Prose wrapped wider than the output area leaves the host to wrap again on
    // top of it, which double-wraps in a terminal and silently truncates in a
    // pane that clips.
    let mut config = AppConfig::new_test();
    config.style.markdown.wrap_width = 80;

    assert_eq!(wrap_width(&config.style, Some(30)), 30);
}

#[test]
fn wrap_width_keeps_the_configured_preference_on_a_wider_output() {
    // The configured width is a reading-comfort choice, not a limitation: extra
    // columns don't widen it.
    let mut config = AppConfig::new_test();
    config.style.markdown.wrap_width = 80;

    assert_eq!(wrap_width(&config.style, Some(200)), 80);
    assert_eq!(wrap_width(&config.style, None), 80);
}

#[test]
fn reasoning_fill_defers_to_the_terminal_for_a_measured_width() {
    // `\x1b[K` follows the real edge, so a window resized part-way through a
    // response still shades whole lines. Padding to the width sampled at startup
    // would bake that width into the output.
    let mut config = AppConfig::new_test();
    config.style.reasoning.background = Some(Color::Ansi256(236));

    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    let renderer = ChatRenderer::new(
        Arc::new(printer.with_output_width(OutputWidth::Terminal(40))),
        config.style,
    );

    assert_eq!(
        renderer.reasoning_background().map(|bg| bg.fill),
        Some(BackgroundFill::Terminal)
    );
}

#[test]
fn reasoning_fill_pads_with_spaces_for_a_declared_width() {
    // A declared width describes an area some other program lays out, and such a
    // host does not implement the erase, so the fill has to be real characters.
    let mut config = AppConfig::new_test();
    config.style.reasoning.background = Some(Color::Ansi256(236));

    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    let renderer = ChatRenderer::new(
        Arc::new(printer.with_output_width(OutputWidth::Declared(40))),
        config.style,
    );

    assert_eq!(
        renderer.reasoning_background().map(|bg| bg.fill),
        Some(BackgroundFill::Column(40))
    );
}

#[test]
fn reasoning_code_block_in_a_list_stays_within_the_declared_width() {
    // A fenced block inside a list item is indented after its background fill is
    // applied, so the fill has to account for the indent the renderer adds
    // afterwards. Padding to the full width first produced `indent + width`
    // columns, which wraps in a terminal and is clipped in a pane that doesn't.
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Full;
    config.style.reasoning.background = Some(Color::Ansi256(236));
    let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
    let mut renderer = ChatRenderer::new(
        Arc::new(printer.with_output_width(OutputWidth::Declared(40))),
        config.style,
    );

    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: indoc::indoc! {"
            - item

              ```rust
              let x = 1;
              ```
        "}
        .into(),
    });
    renderer.flush();
    renderer.printer.flush();

    let rendered = strip_ansi(&out.lock());
    for line in rendered.lines() {
        assert!(
            line.chars().count() <= 40,
            "line runs past the declared width: {line:?}\nfull output:\n{rendered}"
        );
    }

    let code = rendered
        .lines()
        .find(|line| line.contains("let x = 1;"))
        .expect("the code line should be rendered");
    assert_eq!(
        code.chars().count(),
        40,
        "the code line should be shaded across the full declared width: {code:?}"
    );
}

/// A table is laid out rather than soft-wrapped, so the renderer has to fit it
/// to the terminal width the printer reports.
#[test]
fn test_table_is_fitted_to_the_printers_terminal_width() {
    let mut config = AppConfig::new_test();
    config.style.markdown.wrap_width = 80;
    config.style.markdown.table_max_column_width = 40;

    let (printer, out, _err) = Printer::memory(OutputFormat::Text);
    let mut renderer = ChatRenderer::new(
        Arc::new(printer.with_output_width(OutputWidth::Terminal(30))),
        config.style,
    );

    renderer.render_response(&ChatResponse::Message {
        message: "| Alpha heading | Beta heading |\n| --- | --- |\n| first cell content | second \
                  cell content |\n\n"
            .into(),
    });
    renderer.flush();
    renderer.printer.flush();

    let rendered = strip_ansi(&out.lock());
    // Wrapped rows continue on a line opening with `┆` rather than `|`.
    let rows: Vec<&str> = rendered
        .lines()
        .filter(|l| l.starts_with('|') || l.starts_with('┆'))
        .collect();
    assert!(rows.len() > 3, "expected wrapped rows:\n{rendered}");
    for row in rows {
        assert_eq!(
            row.chars().count(),
            30,
            "row should fit the reported terminal width: {row:?}"
        );
    }
}

/// The continuation edge is configurable, so the rendered table has to follow
/// `style.markdown.table_continuation_edge` rather than a hardcoded default.
#[test]
fn test_table_continuation_edge_follows_the_config() {
    let mut config = AppConfig::new_test();
    config.style.markdown.wrap_width = 80;
    config.style.markdown.table_max_column_width = 40;
    config.style.markdown.table_continuation_edge = false;

    let (printer, out, _err) = Printer::memory(OutputFormat::Text);
    let mut renderer = ChatRenderer::new(
        Arc::new(printer.with_output_width(OutputWidth::Terminal(30))),
        config.style,
    );

    renderer.render_response(&ChatResponse::Message {
        message: "| Alpha heading | Beta heading |\n| --- | --- |\n| first cell content | second \
                  cell content |\n\n"
            .into(),
    });
    renderer.flush();
    renderer.printer.flush();

    // The fourth line means the data row wrapped, so there is a continuation
    // line for the setting to act on.
    let rendered = strip_ansi(&out.lock());
    let rows: Vec<&str> = rendered.lines().filter(|l| l.contains('|')).collect();
    assert_eq!(rows, vec![
        "| Alpha headi… | Beta headi… |",
        "|--------------|-------------|",
        "| first cell   | second cell |",
        "| content      | content     |",
    ]);
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

/// The timer line and the tool chrome share the terminal row, so entering a
/// tool call must erase the timer.
/// Nothing else pins this: the timer writes to stderr, so a leaked line is
/// invisible to assertions on rendered stdout.
#[tokio::test]
async fn test_timer_reasoning_erased_when_entering_tool_call() {
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Timer;
    let (mut renderer, _out, err) = create_renderer_with_config(config);

    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "Thinking hard\n\n".into(),
    });
    renderer.enter_tool_call();

    renderer.printer.flush();
    assert_eq!(
        *err.lock(),
        "\r\x1b[K",
        "the tool-call boundary must erase the timer line"
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

/// Consecutive reasoning events form one markdown region: text that ends
/// mid-word in one event and resumes in the next joins into a single word.
///
/// This is what a provider relies on when it splits one region of reasoning
/// across several events — Anthropic interrupts a thinking block with an
/// opaque `redacted_thinking` block, which reaches the renderer as a reasoning
/// event holding no text.
#[test]
fn test_consecutive_reasoning_events_form_one_region() {
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Full;
    config.style.reasoning.background = None;
    let (mut renderer, out, _err) = create_renderer_with_config(config);

    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "I can test this directly by ver".into(),
    });
    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: String::new(),
    });
    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "ifying the return value.".into(),
    });
    renderer.flush();
    renderer.printer.flush();

    assert_eq!(
        strip_ansi(&out.lock()),
        "I can test this directly by verifying the return value.\n\n"
    );
}

/// A provider-supplied blank line splits one reasoning region into two blocks.
///
/// This is the channel a provider uses to segment reasoning it delivers as one
/// continuous text stream: without the blank line, the next part's leading
/// `**Header**` parses as bold continuing the previous part's last sentence
/// instead of opening a block of its own.
#[test]
fn test_provider_supplied_separator_splits_reasoning_blocks() {
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Full;
    config.style.reasoning.background = None;
    let (mut renderer, out, _err) = create_renderer_with_config(config);

    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "First section.".into(),
    });
    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "\n\n".into(),
    });
    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "**Second section**".into(),
    });
    renderer.flush();
    renderer.printer.flush();

    assert_eq!(
        strip_ansi(&out.lock()),
        "First section.\n\n**Second section**\n\n"
    );
}

/// The gap between two reasoning blocks stays inside the reasoning region and
/// carries its background; the gap where reasoning gives way to a message does
/// not.
#[test]
fn test_reasoning_block_gap_is_shaded() {
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Full;
    config.style.reasoning.background = Some(Color::Ansi256(236));
    let (mut renderer, out, _err) = create_renderer_with_config(config);

    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "First section.\n\nSecond section.".into(),
    });
    renderer.render_response(&ChatResponse::Message {
        message: "Answer\n\n".into(),
    });
    renderer.flush();
    renderer.printer.flush();

    let output = out.lock().clone();
    assert_eq!(
        output.matches("\x1b[48;5;236m\x1b[K\x1b[49m").count(),
        1,
        "expected one shaded separator between the reasoning blocks and an unshaded one before \
         the message, got: {output:?}"
    );
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
fn test_gap_between_tool_call_and_next_reasoning_is_shaded() {
    // Reasoning → tool call → reasoning: the blank line separating the tool
    // chrome from the resumed reasoning sits inside the region, so it must
    // carry the reasoning background just like the separator before the tool
    // call.
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Full;
    config.style.reasoning.background = Some(Color::Ansi256(236));
    let (mut renderer, out, _err) = create_renderer_with_config(config);

    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "Thinking\n\n".into(),
    });
    renderer.enter_tool_call();
    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "More\n\n".into(),
    });
    renderer.printer.flush();

    let output = out.lock().clone();
    assert_eq!(
        output.matches("\x1b[48;5;236m\x1b[K\x1b[49m").count(),
        2,
        "both the separator before the tool call and the gap after it stay inside the region, \
         got: {output:?}"
    );
}

#[test]
fn test_truncate_marks_the_cut_when_whitespace_fills_the_budget() {
    // The chunk carries no text of its own, but it consumes the last of the
    // budget, so the elision marker still lands — this is the render the
    // separation predicate has to agree with.
    let mut config = AppConfig::new_test();
    config.style.reasoning.display =
        ReasoningDisplayConfig::Truncate(TruncateChars { characters: 2 });
    config.style.reasoning.background = None;
    let (mut renderer, out, _err) = create_renderer_with_config(config);

    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "\n\n".into(),
    });
    renderer.flush();
    renderer.printer.flush();

    assert_eq!(*out.lock(), "...\n\n");
}

#[test]
fn test_gap_after_tool_call_without_background_is_a_plain_blank_line() {
    // With no reasoning background configured there is nothing to shade, so the
    // deferred gap resolves to a plain blank line and the tool chrome stays
    // separated from the resumed reasoning by exactly one empty line.
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Full;
    config.style.reasoning.background = None;
    let (mut renderer, out, _err) = create_renderer_with_config(config);

    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "Thinking\n\n".into(),
    });
    renderer.enter_tool_call();
    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "More\n\n".into(),
    });
    renderer.flush();
    renderer.printer.flush();

    assert_eq!(*out.lock(), "Thinking\n\n\nMore\n\n");
}

#[test]
fn test_gap_after_tool_call_is_unshaded_when_reasoning_renders_nothing() {
    // Reasoning → tool call → a whitespace-only reasoning chunk → message.
    // Interleaved thinking routinely emits such a chunk, which renders nothing,
    // so the gap after the tool chrome ends up between the chrome and the
    // message: it sits outside the reasoning region and must not be shaded.
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Full;
    config.style.reasoning.background = Some(Color::Ansi256(236));
    let (mut renderer, out, _err) = create_renderer_with_config(config);

    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "Thinking\n\n".into(),
    });
    renderer.enter_tool_call();
    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "\n\n".into(),
    });
    renderer.render_response(&ChatResponse::Message {
        message: "Answer\n\n".into(),
    });
    renderer.flush();
    renderer.printer.flush();

    let output = out.lock().clone();
    assert_eq!(
        strip_ansi(&output),
        "Thinking\n\n\nAnswer\n\n",
        "the gap after the tool chrome must survive as a plain blank line, got: {output:?}"
    );
    assert_eq!(
        output.matches("\x1b[48;5;236m\x1b[K\x1b[49m").count(),
        1,
        "only the separator before the tool call is shaded; the gap before the message is not, \
         got: {output:?}"
    );
}

#[test]
fn test_tool_calls_stay_adjacent_when_reasoning_between_them_renders_nothing() {
    // A whitespace-only interleaved-thinking chunk between two tool calls marks
    // the region as reasoning but puts nothing on screen, so the two headers are
    // adjacent and the chat renderer contributes no gap between them. (The
    // headers themselves are chrome on stderr, written by the `ToolRenderer`.)
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Full;
    config.style.reasoning.background = Some(Color::Ansi256(236));
    let (mut renderer, out, _err) = create_renderer_with_config(config);

    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "\n\n".into(),
    });
    renderer.enter_tool_call();
    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "\n\n".into(),
    });
    renderer.enter_tool_call();
    renderer.flush();
    renderer.printer.flush();

    assert_eq!(*out.lock(), "");
}

#[test]
fn test_gap_between_tool_calls_survives_when_reasoning_between_them_renders() {
    // The counterpart: reasoning that does render between two tool calls keeps
    // its gaps on both sides, all three inside the shaded region.
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Full;
    config.style.reasoning.background = Some(Color::Ansi256(236));
    let (mut renderer, out, _err) = create_renderer_with_config(config);

    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "Thinking\n\n".into(),
    });
    renderer.enter_tool_call();
    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "More\n\n".into(),
    });
    renderer.enter_tool_call();
    renderer.printer.flush();

    let output = out.lock().clone();
    assert_eq!(
        strip_ansi(&output),
        "Thinking\n\n\nMore\n\n",
        "got: {output:?}"
    );
    assert_eq!(
        output.matches("\x1b[48;5;236m\x1b[K\x1b[49m").count(),
        3,
        "the gaps before, after, and following the resumed reasoning all stay inside the region, \
         got: {output:?}"
    );
}

#[test]
fn test_role_header_ends_the_reasoning_region() {
    // A role boundary (a new turn's header, or a user header) ends any
    // reasoning region: a tool call at the start of the next turn must not
    // continue the previous turn's reasoning.
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Full;
    config.style.reasoning.background = Some(Color::Ansi256(236));
    let (mut renderer, _out, _err) = create_renderer_with_config(config);

    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "Thinking\n\n".into(),
    });
    renderer.render_role_header("alice", None, None);

    assert!(
        renderer.enter_tool_call().is_none(),
        "a reasoning region must not survive a role boundary"
    );
}

#[test]
fn test_user_request_ends_the_reasoning_region() {
    // A user message ends any reasoning region, even on the headerless echo
    // path that renders the request without a preceding role header.
    let mut config = AppConfig::new_test();
    config.style.reasoning.display = ReasoningDisplayConfig::Full;
    config.style.reasoning.background = Some(Color::Ansi256(236));
    let (mut renderer, _out, _err) = create_renderer_with_config(config);

    renderer.render_response(&ChatResponse::Reasoning {
        reasoning: "Thinking\n\n".into(),
    });
    renderer.render_request("now act");

    assert!(
        renderer.enter_tool_call().is_none(),
        "a reasoning region must not survive a user request"
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
