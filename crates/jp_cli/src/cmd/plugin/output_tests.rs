use jp_config::AppConfig;
use jp_printer::Chrome;
use pretty_assertions::assert_eq;

use super::*;

fn message(channel: &str, format: &str, text: &str) -> PrintMessage {
    PrintMessage {
        text: text.to_owned(),
        channel: channel.to_owned(),
        format: format.to_owned(),
        language: None,
    }
}

fn style() -> MarkdownConfig {
    AppConfig::new_test().style.markdown
}

/// Print through a memory printer and return what reached stdout and stderr.
fn printed(format: OutputFormat, chrome: Chrome, messages: &[PrintMessage]) -> (String, String) {
    let (printer, out, err) = Printer::memory(format);
    let printer = printer.with_chrome(chrome);

    for message in messages {
        print(&printer, message, &style());
    }
    printer.flush();

    let out = out.lock().clone();
    let err = err.lock().clone();
    (out, err)
}

#[test]
fn each_channel_reaches_its_stream() {
    let (out, err) = printed(OutputFormat::Text, Chrome::Shown, &[
        message("content", "plain", "data\n"),
        message("chrome", "plain", "status\n"),
        message("tool_call", "plain", "call\n"),
        message("error", "plain", "failed\n"),
    ]);

    assert_eq!(out, "data\n");
    assert_eq!(err, "status\ncall\nfailed\n");
}

/// `--quiet` removes the plugin's commentary, not its data or its errors.
#[test]
fn quiet_silences_chrome_but_not_content_or_errors() {
    let (out, err) = printed(OutputFormat::Text, Chrome::Silenced, &[
        message("content", "plain", "data\n"),
        message("reasoning", "plain", "thinking\n"),
        message("error", "plain", "failed\n"),
    ]);

    assert_eq!(out, "data\n");
    assert_eq!(err, "failed\n");
}

/// The RFD's example for `jp --format json titles`.
#[test]
fn under_json_a_json_print_is_its_own_record_and_the_rest_are_wrapped() {
    let (out, err) = printed(OutputFormat::Json, Chrome::Shown, &[
        message("content", "plain", "Refactor config\n"),
        message("content", "json", "{\"id\": \"17127583920\"}"),
    ]);

    assert_eq!(
        out,
        "{\"message\":\"Refactor config\"}\n{\"id\":\"17127583920\"}\n"
    );
    assert_eq!(err, "");
}

/// An error the plugin sends as JSON is its own record on stderr, like any
/// other JSON print, and `--quiet` does not silence it.
#[test]
fn under_json_a_json_error_is_its_own_record_on_stderr() {
    let (out, err) = printed(OutputFormat::Json, Chrome::Silenced, &[
        message("error", "json", "{\"code\": \"denied\"}"),
        message("error", "plain", "failed\n"),
    ]);

    assert_eq!(out, "");
    assert_eq!(err, "{\"code\":\"denied\"}\n{\"message\":\"failed\"}\n");
}

#[test]
fn under_json_markdown_is_not_rendered() {
    let (out, _) = printed(OutputFormat::Json, Chrome::Shown, &[message(
        "content",
        "markdown",
        "# Title\n",
    )]);

    assert_eq!(out, "{\"message\":\"# Title\"}\n");
}

#[test]
fn under_json_invalid_json_is_wrapped_as_a_message() {
    let (out, _) = printed(OutputFormat::Json, Chrome::Shown, &[message(
        "content",
        "json",
        "{not json",
    )]);

    assert_eq!(out, "{\"message\":\"{not json\"}\n");
}

#[test]
fn json_is_pretty_printed_in_terminal_output() {
    let (out, _) = printed(OutputFormat::Text, Chrome::Shown, &[message(
        "content",
        "json",
        "{\"id\":1}",
    )]);

    assert_eq!(out, "{\n  \"id\": 1\n}\n");
}

/// Markdown is rendered, not passed through: the list marker is the renderer's,
/// not the plugin's.
#[test]
fn markdown_is_rendered_in_terminal_output() {
    let rendered = render(
        &message("content", "markdown", "* one\n* two\n"),
        OutputFormat::Text,
        &style(),
    );

    assert_eq!(rendered, Emit::Text("- one\n- two\n".to_owned()));
}

#[test]
fn an_unknown_channel_is_content_and_an_unknown_format_is_plain() {
    assert_eq!(stream("status_bar"), Stream::Content);

    assert_eq!(
        render(
            &message("content", "html", "<b>x</b>"),
            OutputFormat::Text,
            &style()
        ),
        Emit::Text("<b>x</b>".to_owned())
    );
}
