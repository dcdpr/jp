use std::{sync::Arc, time::Duration};

use camino_tempfile::Utf8TempDir;
use jp_config::{
    AppConfig,
    conversation::tool::{CommandConfigOrString, style::ParametersStyle},
};
use jp_conversation::event::ToolCallResponse;
use jp_printer::{OutputFormat, Printer, SharedBuffer};
use serde_json::{Map, Value};

use super::*;

/// Strip ANSI escape codes for readable snapshots.
fn strip_ansi(s: &str) -> String {
    let bytes = strip_ansi_escapes::strip(s);
    String::from_utf8(bytes).expect("valid utf-8 after stripping ANSI")
}

fn create_renderer() -> (ToolRenderer, SharedBuffer, SharedBuffer) {
    let (printer, out, err) = Printer::memory(OutputFormat::TextPretty);
    let mut config = AppConfig::new_test().style;
    config.tool_call.show = true;
    let renderer = ToolRenderer::new(Arc::new(printer), config, "/tmp".into(), false);
    (renderer, out, err)
}

fn create_renderer_with_show(show: bool) -> (ToolRenderer, SharedBuffer) {
    let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
    let mut config = AppConfig::new_test().style;
    config.tool_call.show = show;
    config.tool_call.preparing.show = true;
    config.tool_call.preparing.delay_secs = 0;
    config.tool_call.preparing.interval_ms = 100;
    let renderer = ToolRenderer::new(Arc::new(printer), config, "/tmp".into(), false);
    (renderer, out)
}

/// Helper: render a tool call, flush, and return stripped output.
fn render_and_capture(
    arguments: &Map<String, Value>,
    style: &ParametersStyle,
    tool_name: &str,
) -> String {
    let (renderer, out, _) = create_renderer();
    renderer.render_tool_call(tool_name, arguments, style);
    renderer.printer.flush();
    strip_ansi(&out.lock())
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
    // The custom command is only run via render_custom_arguments after approval.
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
    let (printer, out, _) = Printer::memory(OutputFormat::TextPretty);
    let config = AppConfig::new_test().style;
    let renderer = ToolRenderer::new(Arc::new(printer), config, root.path().to_owned(), false);

    let mut args = Map::new();
    args.insert("host".into(), Value::String("myhost".into()));
    let cmd = CommandConfigOrString::String("echo custom-output".into()).command();

    renderer
        .render_custom_arguments("ssh_run", &args, cmd)
        .await;

    renderer.printer.flush();
    let output = strip_ansi(&out.lock());
    insta::assert_snapshot!(output);
}

#[test]
fn test_render_result_basic() {
    let (renderer, out, _) = create_renderer();
    let response = ToolCallResponse {
        id: "call_1".into(),
        result: Ok("Hello, world!".into()),
    };
    renderer.render_result(&response, &InlineResults::Full, &LinkStyle::Off);
    renderer.printer.flush();
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
    renderer.printer.flush();
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
    renderer.printer.flush();
    let output = strip_ansi(&out.lock());
    insta::assert_snapshot!(output);
}

#[test]
fn test_progress() {
    let (renderer, out, _) = create_renderer();
    renderer.render_progress(Duration::from_secs(5));
    renderer.printer.flush();
    let output = strip_ansi(&out.lock());
    insta::assert_snapshot!(output, @"⏱ Running… 5.0s");
}

#[test]
fn test_clear_progress() {
    let (renderer, out, _) = create_renderer();
    renderer.clear_progress();
    renderer.printer.flush();
    // Raw output is \r\x1b[K which strips to empty after ANSI removal
    assert!(strip_ansi(&out.lock()).is_empty());
}

#[test]
fn test_register_single_tool() {
    let (mut renderer, out) = create_renderer_with_show(true);
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    renderer.register("id1", "fs_read_file", &tx);
    renderer.printer.flush();
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
    renderer.printer.flush();
    let output = strip_ansi(&out.lock());
    assert!(output.contains("Calling tools"), "output: {output:?}");
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
fn test_complete_prints_permanent_line() {
    let (mut renderer, out) = create_renderer_with_show(true);
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    renderer.register("id1", "fs_read_file", &tx);

    let mut args = Map::new();
    args.insert("path".into(), Value::String("/tmp/test.rs".into()));
    renderer.complete(
        "id1",
        "fs_read_file",
        &args,
        &ParametersStyle::FunctionCall,
        true,
    );

    renderer.printer.flush();
    let output = strip_ansi(&out.lock());
    assert!(output.contains("Calling tool"), "output: {output:?}");
    assert!(output.contains("/tmp/test.rs"), "output: {output:?}");
    assert!(renderer.is_rendered("id1"));
    assert!(!renderer.has_pending());
}

#[test]
fn test_complete_hidden_removes_from_pending_without_rendering() {
    let (mut renderer, out) = create_renderer_with_show(true);
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    renderer.register("id1", "fs_read_file", &tx);
    renderer.complete(
        "id1",
        "fs_read_file",
        &Map::new(),
        &ParametersStyle::Off,
        false,
    );
    assert!(!renderer.has_pending());
    assert!(!renderer.is_rendered("id1"));
    // No permanent "Calling tool" line should have been printed.
    renderer.printer.flush();
    let output = strip_ansi(&out.lock());
    assert!(
        !output.contains("Calling tool fs_read_file("),
        "hidden tool should not print permanent line: {output:?}"
    );
}

#[test]
fn test_cancel_all_clears_pending_preserves_rendered() {
    let (mut renderer, _out) = create_renderer_with_show(true);
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    renderer.register("id1", "tool_a", &tx);
    renderer.register("id2", "tool_b", &tx);
    renderer.complete("id1", "tool_a", &Map::new(), &ParametersStyle::Off, true);
    renderer.cancel_all();
    assert!(renderer.is_rendered("id1"), "rendered should be preserved");
    assert!(!renderer.has_pending(), "pending should be cleared");
}

#[test]
fn test_reset_clears_everything() {
    let (mut renderer, _out) = create_renderer_with_show(true);
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    renderer.register("id1", "tool_a", &tx);
    renderer.complete("id1", "tool_a", &Map::new(), &ParametersStyle::Off, true);
    renderer.reset();
    assert!(!renderer.is_rendered("id1"), "rendered cleared after reset");
    assert!(!renderer.has_pending());
}

#[test]
fn test_tick_with_pending_tools() {
    let (mut renderer, out) = create_renderer_with_show(true);
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    renderer.register("id1", "fs_read_file", &tx);
    renderer.tick(Duration::from_millis(1500));
    renderer.printer.flush();
    let output = strip_ansi(&out.lock());
    assert!(output.contains("receiving arguments"), "output: {output:?}");
    assert!(output.contains("1.5s"), "output: {output:?}");
}

#[test]
fn test_show_false_suppresses_preparing_output() {
    let config = AppConfig::new_test().style;
    let mut renderer = ToolRenderer::new(Arc::new(Printer::sink()), config, "/tmp".into(), false);
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    renderer.register("id1", "tool_a", &tx);

    let mut args = Map::new();
    args.insert("x".into(), Value::Number(1.into()));
    renderer.complete("id1", "tool_a", &args, &ParametersStyle::FunctionCall, true);
    // Should not panic; output goes to sink.
    assert!(renderer.is_rendered("id1"));
}

#[test]
fn test_tool_call_show_false_suppresses_output() {
    let config = AppConfig::new_test().style;
    let renderer = ToolRenderer::new(Arc::new(Printer::sink()), config, "/tmp".into(), false);
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
    // separately via render_custom_arguments.
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
    let result = format_args_custom("my_tool", &args, cmd, root.path())
        .await
        .unwrap();
    assert_eq!(result, "hello-world");
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
