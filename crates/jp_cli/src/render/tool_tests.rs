use std::{sync::Arc, time::Duration};

use camino_tempfile::Utf8TempDir;
use jp_config::{
    AppConfig,
    conversation::tool::{CommandConfigOrString, style::ParametersStyle},
};
use jp_conversation::event::ToolCallResponse;
use jp_md::format::{BackgroundFill, DefaultBackground};
use jp_printer::{ErrChannel, OutputFormat, Printer, SharedBuffer};
use serde_json::{Map, Value};

use super::*;

/// A full-width reasoning-region background for the shaded-chrome tests.
fn terminal_region() -> DefaultBackground {
    DefaultBackground {
        param: "48;5;236".into(),
        fill: BackgroundFill::Terminal,
    }
}

/// Strip ANSI escape codes for readable snapshots.
fn strip_ansi(s: &str) -> String {
    let bytes = strip_ansi_escapes::strip(s);
    String::from_utf8(bytes).expect("valid utf-8 after stripping ANSI")
}

/// Resolve cursor-relative output (`\r`, `\n`, and the clear-to-end-of-line
/// escape `\x1b[K`) into the final visible lines, dropping other ANSI (color)
/// sequences.
///
/// `strip_ansi` removes `\r` along with the escapes, which glues together text
/// that a real terminal would have overwritten via cursor moves.
/// This emulates what the terminal actually shows so tests can assert against
/// it.
fn visible_lines(raw: &str) -> Vec<String> {
    let mut lines: Vec<Vec<char>> = vec![Vec::new()];
    let mut row = 0usize;
    let mut col = 0usize;
    let mut chars = raw.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '\n' => {
                row += 1;
                col = 0;
                if row == lines.len() {
                    lines.push(Vec::new());
                }
            }
            '\r' => col = 0,
            '\x1b' if chars.peek() == Some(&'[') => {
                chars.next();
                let mut final_byte = None;
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() || c == '~' {
                        final_byte = Some(c);
                        break;
                    }
                }
                // Clear-to-end-of-line; other CSI (e.g. SGR `m`) are
                // visual-only and ignored.
                if final_byte == Some('K') {
                    lines[row].truncate(col);
                }
            }
            _ => {
                let line = &mut lines[row];
                while line.len() < col {
                    line.push(' ');
                }
                if col < line.len() {
                    line[col] = c;
                } else {
                    line.push(c);
                }
                col += 1;
            }
        }
    }

    lines.into_iter().map(|l| l.into_iter().collect()).collect()
}

fn create_renderer() -> (ToolRenderer, SharedBuffer, SharedBuffer) {
    let (printer, out, err) = Printer::memory(OutputFormat::TextPretty);
    let mut config = AppConfig::new_test().style;
    config.tool_call.show = true;
    let renderer = ToolRenderer::new(
        ErrChannel::new(Arc::new(printer)),
        config,
        "/tmp".into(),
        false,
        jp_llm::tool::InvocationContext::default(),
    );
    (renderer, err, out)
}

fn create_renderer_with_show(show: bool) -> (ToolRenderer, SharedBuffer) {
    let (printer, _out, err) = Printer::memory(OutputFormat::TextPretty);
    let mut config = AppConfig::new_test().style;
    config.tool_call.show = show;
    // The temp line is TTY-gated, so these tests run as a TTY. `preparing.show`
    // stays off to keep `register` from spawning a timer task (sync tests have
    // no tokio runtime); `tick` is exercised by calling it directly.
    config.tool_call.preparing.show = false;
    let renderer = ToolRenderer::new(
        ErrChannel::new(Arc::new(printer)),
        config,
        "/tmp".into(),
        true,
        jp_llm::tool::InvocationContext::default(),
    );
    (renderer, err)
}

/// Helper: render a tool call, flush, and return stripped output.
fn render_and_capture(
    arguments: &Map<String, Value>,
    style: &ParametersStyle,
    tool_name: &str,
) -> String {
    let (renderer, err, _) = create_renderer();
    renderer.render_tool_call(tool_name, arguments, style);
    renderer.channel.flush();
    strip_ansi(&err.lock())
}

#[test]
fn test_render_tool_call_json() {
    let mut args = Map::new();
    args.insert("path".into(), Value::String("/tmp/test.txt".into()));

    let output = render_and_capture(&args, &ParametersStyle::Json, "fs_read_file");
    insta::assert_snapshot!(output);
}

#[test]
fn test_render_tool_call_function_call() {
    let mut args = Map::new();
    args.insert("package".into(), Value::String("jp_cli".into()));

    let output = render_and_capture(&args, &ParametersStyle::FunctionCall, "cargo_check");
    insta::assert_snapshot!(output);
}

#[test]
fn test_render_tool_call_off() {
    let mut args = Map::new();
    args.insert("key".into(), Value::String("value".into()));

    let output = render_and_capture(&args, &ParametersStyle::Off, "my_tool");
    insta::assert_snapshot!(output);
}

#[test]
fn test_render_tool_call_empty_args() {
    let output = render_and_capture(&Map::new(), &ParametersStyle::FunctionCall, "my_tool");
    insta::assert_snapshot!(output);
}

#[test]
fn test_render_tool_call_custom_does_not_run_command() {
    // Custom style with render_tool_call should NOT execute the command.
    // The custom command is only run via render_approved after approval.
    let mut args = Map::new();
    args.insert("host".into(), Value::String("myhost".into()));
    let style = ParametersStyle::Custom(CommandConfigOrString::String("echo LEAKED".into()));

    let output = render_and_capture(&args, &style, "ssh_run");
    assert!(
        !output.contains("LEAKED"),
        "Custom formatter should not run during render_tool_call"
    );
    insta::assert_snapshot!(output);
}

#[tokio::test]
async fn test_render_custom_arguments_after_approval() {
    let root = Utf8TempDir::new().unwrap();
    let (printer, _out, err) = Printer::memory(OutputFormat::TextPretty);
    let config = AppConfig::new_test().style;
    let renderer = ToolRenderer::new(
        ErrChannel::new(Arc::new(printer)),
        config,
        root.path().to_owned(),
        false,
        jp_llm::tool::InvocationContext::default(),
    );

    let mut args = Map::new();
    args.insert("host".into(), Value::String("myhost".into()));
    let style = ParametersStyle::Custom(CommandConfigOrString::String("echo custom-output".into()));

    let outcome = renderer.render_approved("ssh_run", &args, &style).await;

    assert!(matches!(outcome, RenderOutcome::Rendered {
        content: Some(_)
    }));
    renderer.channel.flush();
    let output = strip_ansi(&err.lock());
    // Explicit assertion rather than a snapshot: the spacing contract is the
    // point here, and `insta` normalizes away leading/trailing blank lines, so
    // a snapshot can't actually guard it. The header is followed by a blank
    // line, the custom output, and the trailing newline the lazy separator
    // later turns into the blank line before the next header.
    assert_eq!(output, "Calling tool ssh_run\n\ncustom-output\n");
}

#[test]
fn test_consecutive_plain_headers_are_grouped() {
    // Plain (non-custom) headers carry no owed separator, so a batch of tool
    // calls renders as a tight group without blank lines between them.
    let (renderer, err, _) = create_renderer();
    let args = Map::new();
    renderer.render_tool_call("foo", &args, &ParametersStyle::FunctionCall);
    renderer.render_tool_call("bar", &args, &ParametersStyle::FunctionCall);
    renderer.channel.flush();
    assert_eq!(
        strip_ansi(&err.lock()),
        "Calling tool foo\nCalling tool bar\n"
    );
}

#[test]
fn test_custom_arguments_separate_following_header() {
    // Custom argument output owes a blank-line separator before the next tool
    // call header.
    let (renderer, err, _) = create_renderer();
    let args = Map::new();
    renderer.render_formatted_arguments("plan output");
    renderer.render_tool_call("foo", &args, &ParametersStyle::FunctionCall);
    renderer.channel.flush();
    assert_eq!(
        strip_ansi(&err.lock()),
        "\nplan output\n\nCalling tool foo\n"
    );
}

#[test]
fn test_result_separates_following_header() {
    // A rendered result owes a blank-line separator before the next tool call.
    let (renderer, err, _) = create_renderer();
    let args = Map::new();
    let response = ToolCallResponse {
        id: "call_1".into(),
        result: Ok("done".into()),
    };
    renderer.render_result(&response, &InlineResults::Full, &LinkStyle::Off);
    renderer.render_tool_call("foo", &args, &ParametersStyle::FunctionCall);
    renderer.channel.flush();
    assert_eq!(strip_ansi(&err.lock()), "\ndone\n\nCalling tool foo\n");
}

#[test]
fn test_render_result_basic() {
    let (renderer, out, _) = create_renderer();
    let response = ToolCallResponse {
        id: "call_1".into(),
        result: Ok("Hello, world!".into()),
    };
    renderer.render_result(&response, &InlineResults::Full, &LinkStyle::Off);
    renderer.channel.flush();
    let output = strip_ansi(&out.lock());
    insta::assert_snapshot!(output);
}

#[test]
fn test_render_result_off() {
    let (renderer, out, _) = create_renderer();
    let response = ToolCallResponse {
        id: "call_1".into(),
        result: Ok("This should not appear".into()),
    };
    renderer.render_result(&response, &InlineResults::Off, &LinkStyle::Off);
    renderer.channel.flush();
    assert!(out.lock().is_empty());
}

#[test]
fn test_render_result_truncated() {
    let (renderer, out, _) = create_renderer();
    let response = ToolCallResponse {
        id: "call_1".into(),
        result: Ok("line1\nline2\nline3\nline4\nline5".into()),
    };
    renderer.render_result(
        &response,
        &InlineResults::Truncate(TruncateLines { lines: 2 }),
        &LinkStyle::Off,
    );
    renderer.channel.flush();
    let output = strip_ansi(&out.lock());
    // Explicit assertion rather than a snapshot, for the same reason as
    // `test_render_custom_arguments_after_approval`: the leading blank line and
    // the trailing newline after the truncation marker are part of the
    // contract, and `insta` would trim both before comparing.
    assert_eq!(output, "\nline1\nline2\n _(truncated to 2 lines)_\n");
}

#[test]
fn test_empty_result_does_not_separate_following_header() {
    // An empty result writes nothing, so it owes no separator: the next tool
    // header must land directly under it rather than below a stray blank line.
    let (renderer, err, _) = create_renderer();
    let args = Map::new();
    let response = ToolCallResponse {
        id: "call_1".into(),
        result: Ok(String::new()),
    };
    renderer.render_result(&response, &InlineResults::Full, &LinkStyle::Off);
    renderer.render_tool_call("foo", &args, &ParametersStyle::FunctionCall);
    renderer.channel.flush();
    assert_eq!(strip_ansi(&err.lock()), "Calling tool foo\n");
}

#[test]
fn test_progress() {
    let (renderer, out, _) = create_renderer();
    renderer.render_progress(Duration::from_secs(5));
    renderer.channel.flush();
    let output = strip_ansi(&out.lock());
    insta::assert_snapshot!(output, @"⏱ Running… 5.0s");
}

#[test]
fn test_clear_progress() {
    let (renderer, out, _) = create_renderer();
    renderer.clear_progress();
    renderer.channel.flush();
    // Raw output is \r\x1b[K which strips to empty after ANSI removal
    assert!(strip_ansi(&out.lock()).is_empty());
}

#[test]
fn test_register_single_tool() {
    let (mut renderer, out) = create_renderer_with_show(true);
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    renderer.register("id1", "fs_read_file", &tx);
    renderer.channel.flush();
    let output = strip_ansi(&out.lock());
    assert!(output.contains("Calling tool"), "output: {output:?}");
    assert!(output.contains("fs_read_file"), "output: {output:?}");
    assert!(
        !output.contains("tools"),
        "singular for one tool: {output:?}"
    );
}

#[test]
fn test_register_multiple_tools_uses_plural() {
    let (mut renderer, out) = create_renderer_with_show(true);
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    renderer.register("id1", "fs_read_file", &tx);
    renderer.register("id2", "cargo_check", &tx);
    renderer.channel.flush();
    let output = strip_ansi(&out.lock());
    assert!(output.contains("Calling tools"), "output: {output:?}");
}

#[test]
fn test_temp_line_separated_from_previous_tool_output() {
    // While arguments stream, the temp line is the first thing rendered after a
    // preceding result or custom-argument block. It must carry the owed
    // blank-line separator so it isn't glued to that output on a TTY.
    let (mut renderer, err) = create_renderer_with_show(true);
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    renderer.render_formatted_arguments("plan output");
    renderer.register("id1", "fs_read_file", &tx);
    renderer.channel.flush();

    let lines = visible_lines(&strip_ansi(&err.lock()));
    let idx = lines
        .iter()
        .position(|l| l == "plan output")
        .unwrap_or_else(|| panic!("plan output present: {lines:?}"));
    assert_eq!(lines[idx + 1], "", "blank line before temp line: {lines:?}");
    assert!(
        lines[idx + 2].contains("Calling tool"),
        "temp line follows the blank: {lines:?}"
    );
}

#[test]
fn test_register_duplicate_ignored() {
    let (mut renderer, _out) = create_renderer_with_show(true);
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    renderer.register("id1", "fs_read_file", &tx);
    renderer.register("id1", "fs_read_file", &tx);
    assert_eq!(renderer.pending.len(), 1);
}

#[test]
fn test_complete_removes_from_pending() {
    let (mut renderer, _out) = create_renderer_with_show(true);
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    renderer.register("id1", "fs_read_file", &tx);

    renderer.complete("id1");

    assert!(!renderer.has_pending());
}

#[test]
fn test_complete_does_not_render_permanent_line() {
    // Verify complete() only manages the temp line and doesn't print a
    // permanent "Calling tool X(args)" header. In a memory buffer we
    // can't distinguish the temp line from permanent output, so we test
    // by completing *without* registering first — any output would be
    // from complete() itself.
    let (mut renderer, out) = create_renderer_with_show(true);
    renderer.complete("id1");
    assert!(!renderer.has_pending());
    renderer.channel.flush();
    let output = strip_ansi(&out.lock());
    assert!(
        !output.contains("Calling tool"),
        "complete() should not render: {output:?}"
    );
}

#[test]
fn test_completing_one_pending_tool_does_not_collide_with_header() {
    // Reproduces the fast-multi-tool streaming bug: a second tool-call start
    // registers (still pending) before the first request is committed. The
    // turn loop then completes the first tool and immediately prints its
    // permanent header. The still-pending tool must not leave a temp line
    // glued to that header (the observed `…fs_read_fileCalling tool…`).
    let (printer, _out, err) = Printer::memory(OutputFormat::TextPretty);
    let mut config = AppConfig::new_test().style;
    config.tool_call.show = true;
    // Disable the animated suffix so `register` doesn't spawn a timer task
    // (this is a sync test with no tokio runtime).
    config.tool_call.preparing.show = false;
    let mut renderer = ToolRenderer::new(
        ErrChannel::new(Arc::new(printer)),
        config,
        "/tmp".into(),
        true,
        jp_llm::tool::InvocationContext::default(),
    );
    let (tx, _rx) = tokio::sync::mpsc::channel(1);

    renderer.register("id1", "fs_read_file", &tx);
    renderer.register("id2", "fs_read_file", &tx);

    renderer.complete("id1");
    let mut args = Map::new();
    args.insert("path".into(), Value::String("/a".into()));
    renderer.render_tool_call("fs_read_file", &args, &ParametersStyle::FunctionCall);

    renderer.channel.flush();
    let visible = visible_lines(&err.lock());
    for line in &visible {
        assert!(
            line.matches("Calling tool").count() <= 1,
            "two tool headers collided on one visible line: {visible:?}"
        );
    }
}

#[test]
fn test_cancel_all_clears_pending() {
    let (mut renderer, _out) = create_renderer_with_show(true);
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    renderer.register("id1", "tool_a", &tx);
    renderer.register("id2", "tool_b", &tx);
    renderer.complete("id1");
    renderer.cancel_all();
    assert!(!renderer.has_pending(), "pending should be cleared");
}

#[test]
fn test_reset_clears_everything() {
    let (mut renderer, _out) = create_renderer_with_show(true);
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    renderer.register("id1", "tool_a", &tx);
    renderer.complete("id1");
    renderer.reset();
    assert!(!renderer.has_pending());
}

#[test]
fn test_reset_clears_visible_temp_line() {
    // A temp line is on screen (registered, not yet completed) when a new
    // streaming cycle begins. `reset` must clear it rather than leave it
    // stranded on screen.
    let (mut renderer, err) = create_renderer_with_show(true);
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    renderer.register("id1", "fs_read_file", &tx);
    renderer.reset();

    renderer.channel.flush();
    let visible = visible_lines(&err.lock());
    assert!(
        visible.iter().all(|l| !l.contains("Calling tool")),
        "reset left a stale temp line on screen: {visible:?}"
    );
}

#[test]
fn test_tick_with_pending_tools() {
    let (mut renderer, out) = create_renderer_with_show(true);
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    renderer.register("id1", "fs_read_file", &tx);
    renderer.tick(Duration::from_millis(1500));
    renderer.channel.flush();
    let output = strip_ansi(&out.lock());
    assert!(output.contains("receiving arguments"), "output: {output:?}");
    assert!(output.contains("1.5s"), "output: {output:?}");
}

#[test]
fn test_show_false_suppresses_preparing_output() {
    let config = AppConfig::new_test().style;
    let mut renderer = ToolRenderer::new(
        ErrChannel::new(Arc::new(Printer::sink())),
        config,
        "/tmp".into(),
        false,
        jp_llm::tool::InvocationContext::default(),
    );
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    renderer.register("id1", "tool_a", &tx);

    renderer.complete("id1");
    // Should not panic; output goes to sink.
    assert!(!renderer.has_pending());
}

#[test]
fn test_tool_call_show_false_suppresses_output() {
    let config = AppConfig::new_test().style;
    let renderer = ToolRenderer::new(
        ErrChannel::new(Arc::new(Printer::sink())),
        config,
        "/tmp".into(),
        false,
        jp_llm::tool::InvocationContext::default(),
    );
    let mut args = Map::new();
    args.insert("key".into(), Value::String("value".into()));

    // Should not panic even though output goes nowhere.
    renderer.render_tool_call("hidden_tool", &args, &ParametersStyle::FunctionCall);
}

#[test]
fn test_format_args_off() {
    let args = Map::new();
    let result = format_args(&args, &ParametersStyle::Off);
    assert_eq!(result, "");
}

#[test]
fn test_format_args_json() {
    let mut args = Map::new();
    args.insert("path".into(), Value::String("/tmp/test.txt".into()));
    let result = format_args(&args, &ParametersStyle::Json);
    insta::assert_snapshot!(result);
}

#[test]
fn test_format_args_function_call() {
    let mut args = Map::new();
    args.insert("a".into(), Value::Number(1.into()));
    args.insert("b".into(), Value::String("hello".into()));
    let result = format_args(&args, &ParametersStyle::FunctionCall);
    let plain = strip_ansi(&result);
    insta::assert_snapshot!(plain);
}

#[test]
fn test_format_args_custom_returns_empty() {
    // format_args for Custom always returns "" — the actual command is run
    // separately via render_approved.
    let mut args = Map::new();
    args.insert("key".into(), Value::String("value".into()));
    let style = ParametersStyle::Custom(CommandConfigOrString::String("echo custom-output".into()));
    let result = format_args(&args, &style);
    assert_eq!(result, "");
}

#[tokio::test]
async fn test_format_custom_content_returns_raw_content() {
    let root = Utf8TempDir::new().unwrap();
    let mut args = Map::new();
    args.insert("key".into(), Value::String("value".into()));
    let cmd = CommandConfigOrString::String("echo hello-world".into()).command();
    let result = format_args_custom(
        "my_tool",
        &args,
        cmd,
        root.path(),
        &jp_llm::tool::InvocationContext::default(),
    )
    .await
    .unwrap();
    assert_eq!(result, "hello-world");
}

/// Regression: the `format_arguments` path must surface the invocation's
/// workspace and conversation IDs to a custom formatter command via
/// `context.workspace_id` and `context.conversation_id`.
/// A non-empty `InvocationContext` pins the wiring — the other tests pass the
/// empty default, which would still pass if the fields were dropped or wired to
/// empty strings.
#[tokio::test]
async fn test_format_args_custom_exposes_invocation_ids() {
    let root = Utf8TempDir::new().unwrap();
    let args = Map::new();
    let cmd = CommandConfigOrString::String(
        "echo {{context.workspace_id}}/{{context.conversation_id}}".into(),
    )
    .command();
    let invocation = jp_llm::tool::InvocationContext {
        workspace_id: "ws-abc".into(),
        conversation_id: "conv-xyz".into(),
    };
    let result = format_args_custom("my_tool", &args, cmd, root.path(), &invocation)
        .await
        .unwrap();
    assert_eq!(result, "ws-abc/conv-xyz");
}

#[test]
fn test_format_args_hides_empty_object_value() {
    let mut args = Map::new();
    args.insert("path".into(), Value::String("/tmp/test.txt".into()));
    args.insert("options".into(), Value::Object(Map::new()));

    let plain = strip_ansi(&format_args(&args, &ParametersStyle::FunctionCall));
    assert!(plain.contains("/tmp/test.txt"));
    assert!(
        !plain.contains("options"),
        "empty object should be hidden: {plain}"
    );
}

#[test]
fn test_format_args_hides_null_value() {
    let mut args = Map::new();
    args.insert("path".into(), Value::String("/tmp/test.txt".into()));
    args.insert("optional_field".into(), Value::Null);

    let plain = strip_ansi(&format_args(&args, &ParametersStyle::FunctionCall));
    assert!(plain.contains("/tmp/test.txt"));
    assert!(
        !plain.contains("optional_field"),
        "null should be hidden: {plain}"
    );
}

#[test]
fn test_format_args_hides_empty_array_value() {
    let mut args = Map::new();
    args.insert("path".into(), Value::String("/tmp/test.txt".into()));
    args.insert("tags".into(), Value::Array(vec![]));

    let plain = strip_ansi(&format_args(&args, &ParametersStyle::FunctionCall));
    assert!(plain.contains("/tmp/test.txt"));
    assert!(
        !plain.contains("tags"),
        "empty array should be hidden: {plain}"
    );
}

#[test]
fn test_format_args_keeps_nonempty_object_value() {
    let mut inner = Map::new();
    inner.insert("key".into(), Value::String("value".into()));
    let mut args = Map::new();
    args.insert("config".into(), Value::Object(inner));

    let plain = strip_ansi(&format_args(&args, &ParametersStyle::FunctionCall));
    assert!(
        plain.contains("config"),
        "non-empty object should be shown: {plain}"
    );
}

#[test]
fn test_format_args_keeps_nonempty_array_value() {
    let mut args = Map::new();
    args.insert(
        "tags".into(),
        Value::Array(vec![Value::String("foo".into())]),
    );

    let plain = strip_ansi(&format_args(&args, &ParametersStyle::FunctionCall));
    assert!(
        plain.contains("tags"),
        "non-empty array should be shown: {plain}"
    );
}

#[test]
fn set_region_shades_the_tool_call_header() {
    let (mut renderer, chrome, _) = create_renderer();
    renderer.set_region("id1", Some(terminal_region()));

    let mut args = Map::new();
    args.insert("path".into(), Value::String("/tmp/x".into()));
    renderer.render_tool_call("fs_read_file", &args, &ParametersStyle::FunctionCall);
    renderer.channel.flush();

    let raw = chrome.lock().clone();
    assert!(
        raw.contains("\x1b[48;5;236m"),
        "header carries the region background: {raw:?}"
    );
    assert!(raw.contains("\x1b[49m"), "region is closed: {raw:?}");
    assert!(
        strip_ansi(&raw).contains("Calling tool fs_read_file"),
        "header text is preserved: {raw:?}"
    );
}

#[test]
fn set_region_shades_the_result() {
    let (mut renderer, chrome, _) = create_renderer();
    renderer.set_region("call_1", Some(terminal_region()));

    let response = ToolCallResponse {
        id: "call_1".into(),
        result: Ok("done".into()),
    };
    renderer.render_result(&response, &InlineResults::Full, &LinkStyle::Off);
    renderer.channel.flush();

    let raw = chrome.lock().clone();
    assert!(
        raw.contains("\x1b[48;5;236m"),
        "result carries the region background: {raw:?}"
    );
    assert!(
        strip_ansi(&raw).contains("done"),
        "result text is preserved: {raw:?}"
    );
}

#[test]
fn result_uses_its_own_tool_region_keyed_by_id() {
    // The region is captured per tool-call ID: a result for a tool that never
    // entered a region stays unshaded even while another tool's region is set
    // as the current one.
    let (mut renderer, chrome, _) = create_renderer();
    renderer.set_region("shaded", Some(terminal_region()));

    let response = ToolCallResponse {
        id: "plain".into(),
        result: Ok("done".into()),
    };
    renderer.render_result(&response, &InlineResults::Full, &LinkStyle::Off);
    renderer.channel.flush();

    let raw = chrome.lock().clone();
    assert!(
        !raw.contains("\x1b[48;5;236m"),
        "a result for an unshaded tool must stay plain: {raw:?}"
    );
}

#[test]
fn clearing_a_region_with_none_unshades_following_chrome() {
    let (mut renderer, chrome, _) = create_renderer();
    renderer.set_region("id1", Some(terminal_region()));
    renderer.set_region("id1", None);

    let args = Map::new();
    renderer.render_tool_call("foo", &args, &ParametersStyle::FunctionCall);
    renderer.channel.flush();

    let raw = chrome.lock().clone();
    assert!(
        !raw.contains("\x1b[48;5;236m"),
        "a None region must leave the header unshaded: {raw:?}"
    );
}

#[test]
fn preparing_line_fills_to_the_edge_under_a_region() {
    // The initial temp line can sit on screen for seconds before the first
    // tick rewrites it; under a reasoning region it must erase to the right
    // edge so the whole row is shaded, not just the span behind the text.
    let (mut renderer, err) = create_renderer_with_show(true);
    renderer.set_region("id1", Some(terminal_region()));

    let (tick_tx, _tick_rx) = tokio::sync::mpsc::channel(1);
    renderer.register("id1", "fs_read_file", &tick_tx);
    renderer.channel.flush();

    let raw = err.lock().clone();
    assert!(
        raw.contains("\x1b[48;5;236m"),
        "the preparing line carries the region background: {raw:?}"
    );
    assert!(
        raw.ends_with("\x1b[K\x1b[49m"),
        "the preparing line must erase to the edge before the region closes: {raw:?}"
    );
}
