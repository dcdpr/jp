use std::{sync::Arc, time::Duration};

use jp_config::{
    AppConfig,
    conversation::tool::{CommandConfigOrString, style::ParametersStyle},
};
use jp_conversation::event::ToolCallResponse;
use jp_printer::{OutputFormat, Printer, SharedBuffer};
use serde_json::{Map, Value};

use super::*;

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

#[tokio::test]
async fn test_call_start_with_json_args() {
    let (renderer, out, _) = create_renderer();
    let mut arguments = Map::new();
    arguments.insert("path".into(), Value::String("/tmp/test.txt".into()));

    renderer.render_call_header("fs_read_file");
    renderer
        .render_arguments("fs_read_file", &arguments, &ParametersStyle::Json)
        .await
        .unwrap();

    renderer.printer.flush();
    let output = out.lock();
    assert!(output.contains("Calling tool"), "output: {output}");
    assert!(output.contains("fs_read_file"), "output: {output}");
    assert!(output.contains("/tmp/test.txt"), "output: {output}");
}

#[tokio::test]
async fn test_call_start_with_function_call_args() {
    let (renderer, out, _) = create_renderer();
    let mut arguments = Map::new();
    arguments.insert("package".into(), Value::String("jp_cli".into()));

    renderer.render_call_header("cargo_check");
    renderer
        .render_arguments("cargo_check", &arguments, &ParametersStyle::FunctionCall)
        .await
        .unwrap();

    renderer.printer.flush();
    let output = out.lock();
    assert!(output.contains("cargo_check"), "output: {output}");
    assert!(output.contains(": \"jp_cli\""), "output: {output}");
}

#[tokio::test]
async fn test_call_start_empty_args() {
    let (renderer, out, _) = create_renderer();
    renderer.render_call_header("my_tool");
    renderer
        .render_arguments("my_tool", &Map::new(), &ParametersStyle::Json)
        .await
        .unwrap();

    renderer.printer.flush();
    let output = out.lock();
    assert!(output.contains("Calling tool"), "output: {output}");
    assert!(!output.contains('.'), "output: {output}");
}

#[tokio::test]
async fn test_call_start_style_off() {
    let (renderer, out, _) = create_renderer();
    let mut arguments = Map::new();
    arguments.insert("key".into(), Value::String("value".into()));

    renderer.render_call_header("my_tool");
    renderer
        .render_arguments("my_tool", &arguments, &ParametersStyle::Off)
        .await
        .unwrap();

    renderer.printer.flush();
    let output = out.lock();
    assert!(output.contains("Calling tool"), "output: {output}");
    assert!(!output.contains("key"), "output: {output}");
}

#[test]
fn test_progress() {
    let (renderer, out, _) = create_renderer();
    renderer.render_progress(Duration::from_secs(5));
    renderer.printer.flush();
    assert_eq!(*out.lock(), "\r\x1b[K⏱ Running… 5.0s");
}

#[test]
fn test_clear_progress() {
    let (renderer, out, _) = create_renderer();
    renderer.clear_progress();
    renderer.printer.flush();
    assert_eq!(*out.lock(), "\r\x1b[K");
}

#[tokio::test]
async fn test_tool_call_show_false_suppresses_output() {
    // When show=false, the caller passes a sink printer so all writes
    // are silently discarded.
    let config = AppConfig::new_test().style;
    let renderer = ToolRenderer::new(Arc::new(Printer::sink()), config, "/tmp".into(), false);
    let mut arguments = Map::new();
    arguments.insert("key".into(), Value::String("value".into()));

    // These should not panic even though output goes nowhere.
    renderer.render_call_header("hidden_tool");
    renderer
        .render_arguments("hidden_tool", &arguments, &ParametersStyle::FunctionCall)
        .await
        .unwrap();
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
    let output = out.lock();
    assert!(output.contains("Tool call result"));
    assert!(output.contains("Hello, world!"));
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
    assert_eq!(*out.lock(), "");
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
    let output = out.lock();
    assert!(output.contains("truncated to 2 lines"));
    assert!(output.contains("line1"));
    assert!(!output.contains("line3"));
}

#[tokio::test]
async fn test_format_args_off() {
    let args = Map::new();
    let result = format_args(
        "my_tool",
        &args,
        &ParametersStyle::Off,
        Utf8Path::new("/tmp"),
    )
    .await;
    assert_eq!(result.unwrap(), "");
}

#[tokio::test]
async fn test_format_args_json() {
    let mut args = Map::new();
    args.insert("path".into(), Value::String("/tmp/test.txt".into()));
    let result = format_args(
        "my_tool",
        &args,
        &ParametersStyle::Json,
        Utf8Path::new("/tmp"),
    )
    .await
    .unwrap();
    assert!(result.contains("```json"));
    assert!(result.contains("/tmp/test.txt"));
}

#[tokio::test]
async fn test_format_args_function_call() {
    let mut args = Map::new();
    args.insert("a".into(), Value::Number(1.into()));
    args.insert("b".into(), Value::String("hello".into()));
    let result = format_args(
        "my_tool",
        &args,
        &ParametersStyle::FunctionCall,
        Utf8Path::new("/tmp"),
    )
    .await
    .unwrap();
    assert!(result.starts_with('('));
    assert!(result.ends_with(')'));
    assert!(result.contains(": 1"));
    assert!(result.contains(": \"hello\""));
}

#[tokio::test]
async fn test_format_args_custom_with_echo() {
    let mut args = Map::new();
    args.insert("path".into(), Value::String("/tmp/test.txt".into()));
    let style = ParametersStyle::Custom(CommandConfigOrString::String("echo custom-output".into()));
    let result = format_args("my_tool", &args, &style, Utf8Path::new("/tmp"))
        .await
        .unwrap();
    assert_eq!(result, ":\n\ncustom-output");
}

#[tokio::test]
async fn test_format_args_custom_empty_output_returns_empty() {
    let mut args = Map::new();
    args.insert("path".into(), Value::String("/tmp/test.txt".into()));
    let style = ParametersStyle::Custom(CommandConfigOrString::String("true".into()));
    let result = format_args("my_tool", &args, &style, Utf8Path::new("/tmp"))
        .await
        .unwrap();
    assert_eq!(result, "");
}

#[tokio::test]
async fn test_format_args_custom_bad_command_returns_err() {
    let mut args = Map::new();
    args.insert("key".into(), Value::String("value".into()));
    let style =
        ParametersStyle::Custom(CommandConfigOrString::String("/nonexistent/binary".into()));
    let result = format_args("my_tool", &args, &style, Utf8Path::new("/tmp")).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("/nonexistent/binary"));
}

#[tokio::test]
async fn test_format_args_non_custom_styles_always_succeed() {
    let mut args = Map::new();
    args.insert("key".into(), Value::String("value".into()));
    let root = Utf8Path::new("/tmp");
    assert!(
        format_args("t", &args, &ParametersStyle::Off, root)
            .await
            .is_ok()
    );
    assert!(
        format_args("t", &args, &ParametersStyle::Json, root)
            .await
            .is_ok()
    );
    assert!(
        format_args("t", &args, &ParametersStyle::FunctionCall, root)
            .await
            .is_ok()
    );
}

#[test]
fn test_register_single_tool() {
    let (mut renderer, out) = create_renderer_with_show(true);
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    renderer.register("id1", "fs_read_file", &tx);
    renderer.printer.flush();
    let output = out.lock().clone();
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
    let output = out.lock().clone();
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
    renderer.complete("id1", "fs_read_file", r#"(path: "/tmp/test.rs")"#);
    renderer.printer.flush();
    let output = out.lock().clone();
    assert!(output.contains("Calling tool"), "output: {output:?}");
    assert!(output.contains("/tmp/test.rs"), "output: {output:?}");
    assert!(renderer.is_rendered("id1"));
    assert!(!renderer.has_pending());
}

#[test]
fn test_cancel_all_clears_pending_preserves_rendered() {
    let (mut renderer, _out) = create_renderer_with_show(true);
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    renderer.register("id1", "tool_a", &tx);
    renderer.register("id2", "tool_b", &tx);
    renderer.complete("id1", "tool_a", "()");
    renderer.cancel_all();
    assert!(renderer.is_rendered("id1"), "rendered should be preserved");
    assert!(!renderer.has_pending(), "pending should be cleared");
}

#[test]
fn test_reset_clears_everything() {
    let (mut renderer, _out) = create_renderer_with_show(true);
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    renderer.register("id1", "tool_a", &tx);
    renderer.complete("id1", "tool_a", "()");
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
    let output = out.lock().clone();
    assert!(output.contains("receiving arguments"), "output: {output:?}");
    assert!(output.contains("1.5s"), "output: {output:?}");
}

#[test]
fn test_show_false_suppresses_preparing_output() {
    // When show=false, the caller passes a sink printer.
    let config = AppConfig::new_test().style;
    let mut renderer = ToolRenderer::new(Arc::new(Printer::sink()), config, "/tmp".into(), false);
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    renderer.register("id1", "tool_a", &tx);
    renderer.complete("id1", "tool_a", "(x: 1)");
    // Should not panic; output goes to sink.
    assert!(renderer.is_rendered("id1"));
}

#[tokio::test]
async fn test_format_args_hides_empty_object_value() {
    let mut args = Map::new();
    args.insert("path".into(), Value::String("/tmp/test.txt".into()));
    args.insert("options".into(), Value::Object(Map::new()));

    let result = format_args(
        "my_tool",
        &args,
        &ParametersStyle::FunctionCall,
        Utf8Path::new("/tmp"),
    )
    .await
    .unwrap();

    assert!(result.contains("/tmp/test.txt"));
    assert!(
        !result.contains("options"),
        "empty object should be hidden: {result}"
    );
}

#[tokio::test]
async fn test_format_args_hides_null_value() {
    let mut args = Map::new();
    args.insert("path".into(), Value::String("/tmp/test.txt".into()));
    args.insert("optional_field".into(), Value::Null);

    let result = format_args(
        "my_tool",
        &args,
        &ParametersStyle::FunctionCall,
        Utf8Path::new("/tmp"),
    )
    .await
    .unwrap();

    assert!(result.contains("/tmp/test.txt"));
    assert!(
        !result.contains("optional_field"),
        "null should be hidden: {result}"
    );
}

#[tokio::test]
async fn test_format_args_hides_empty_array_value() {
    let mut args = Map::new();
    args.insert("path".into(), Value::String("/tmp/test.txt".into()));
    args.insert("tags".into(), Value::Array(vec![]));

    let result = format_args(
        "my_tool",
        &args,
        &ParametersStyle::FunctionCall,
        Utf8Path::new("/tmp"),
    )
    .await
    .unwrap();

    assert!(result.contains("/tmp/test.txt"));
    assert!(
        !result.contains("tags"),
        "empty array should be hidden: {result}"
    );
}

#[tokio::test]
async fn test_format_args_keeps_nonempty_object_value() {
    let mut inner = Map::new();
    inner.insert("key".into(), Value::String("value".into()));

    let mut args = Map::new();
    args.insert("config".into(), Value::Object(inner));

    let result = format_args(
        "my_tool",
        &args,
        &ParametersStyle::FunctionCall,
        Utf8Path::new("/tmp"),
    )
    .await
    .unwrap();

    assert!(
        result.contains("config"),
        "non-empty object should be shown: {result}"
    );
}

#[tokio::test]
async fn test_format_args_keeps_nonempty_array_value() {
    let mut args = Map::new();
    args.insert(
        "tags".into(),
        Value::Array(vec![Value::String("foo".into())]),
    );

    let result = format_args(
        "my_tool",
        &args,
        &ParametersStyle::FunctionCall,
        Utf8Path::new("/tmp"),
    )
    .await
    .unwrap();

    assert!(
        result.contains("tags"),
        "non-empty array should be shown: {result}"
    );
}
