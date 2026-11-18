use pretty_assertions::assert_eq;

use super::*;
use crate::message::ExitMessage;

/// The manifest line `manifest!` builds for a plugin claiming `jp test`.
const MANIFEST: &str = crate::manifest!(
    protocol: 3,
    description: "Test things",
    command: ["test"],
);

#[test]
fn a_message_is_read_from_one_line() {
    let mut stdin = "{\"type\":\"shutdown\"}\n{\"type\":\"describe\"}\n".as_bytes();

    assert_eq!(read(&mut stdin, "jp-test"), Ok(HostToPlugin::Shutdown));
    assert_eq!(read(&mut stdin, "jp-test"), Ok(HostToPlugin::Describe));
}

#[test]
fn a_closed_pipe_names_the_plugin() {
    let mut stdin = "".as_bytes();

    assert_eq!(
        read(&mut stdin, "jp-test"),
        Err("The host stopped responding. Are `jp` and `jp-test` the same build?".to_owned())
    );
}

#[test]
fn a_line_that_is_no_message_is_refused() {
    let mut stdin = "{\"type\":\"nonsense\"}\n".as_bytes();

    let error = read(&mut stdin, "jp-test").unwrap_err();

    assert!(error.starts_with("invalid host message: "), "{error}");
}

#[test]
fn a_message_is_sent_as_one_line() {
    let mut stdout = vec![];
    let exit = PluginToHost::Exit(ExitMessage {
        code: 0,
        reason: None,
        expected: false,
    });

    send(&mut stdout, &exit).unwrap();
    send(&mut stdout, &exit).unwrap();

    let text = String::from_utf8(stdout).unwrap();
    assert_eq!(text, "{\"type\":\"exit\",\"code\":0}\n".repeat(2));
}

#[test]
fn describe_answers_with_the_manifest_and_jp_as_author() {
    let mut stdout = vec![];

    describe(
        &mut stdout,
        MANIFEST,
        "test",
        "1.2.3",
        "Usage: jp test".to_owned(),
    )
    .unwrap();

    let sent: PluginToHost = serde_json::from_slice(&stdout).unwrap();
    let PluginToHost::Describe(response) = sent else {
        panic!("expected a describe, got {sent:?}");
    };
    assert_eq!(response.manifest.command, ["test"]);
    assert_eq!(response.manifest.protocol, 3);
    assert_eq!(
        (
            response.name.as_str(),
            response.version.as_str(),
            response.help.as_str()
        ),
        ("test", "1.2.3", "Usage: jp test")
    );
    assert_eq!(
        response.author.as_deref(),
        Some("Jean Mertz <git@jeanmertz.com>")
    );
    assert_eq!(
        response.repository.as_deref(),
        Some("https://github.com/dcdpr/jp")
    );
}

#[test]
fn describe_refuses_a_line_that_is_no_manifest() {
    let error = describe(&mut vec![], "not a manifest", "test", "1", String::new()).unwrap_err();

    assert!(error.starts_with("invalid embedded manifest: "), "{error}");
}

/// What `stdout` holds, one line per message.
fn lines(stdout: Vec<u8>) -> Vec<String> {
    String::from_utf8(stdout)
        .unwrap()
        .lines()
        .map(ToOwned::to_owned)
        .collect()
}

#[test]
fn printing_names_the_channel_and_the_format() {
    let mut stdout = vec![];

    print_to(&mut stdout, "note\n", "chrome").unwrap();
    print_json(&mut stdout, "{\"a\":1}").unwrap();

    assert_eq!(lines(stdout), [
        r#"{"type":"print","text":"note\n","channel":"chrome","format":"plain"}"#,
        r#"{"type":"print","text":"{\"a\":1}","channel":"content","format":"json"}"#,
    ]);
}

/// A failure carries its reason; a result the command reports by its code is
/// not a failure, so `jp` prints nothing for it.
#[test]
fn an_exit_is_a_failure_and_a_result_is_not() {
    let mut stdout = vec![];

    send_exit(&mut stdout, 0, None).unwrap();
    send_exit(&mut stdout, 1, Some("no store")).unwrap();
    send_result(&mut stdout, 2).unwrap();
    send_result(&mut stdout, 0).unwrap();

    assert_eq!(lines(stdout), [
        r#"{"type":"exit","code":0}"#,
        r#"{"type":"exit","code":1,"reason":"no store"}"#,
        r#"{"type":"exit","code":2,"expected":true}"#,
        r#"{"type":"exit","code":0}"#,
    ]);
}

/// Someone ran the executable by hand: there is no host to wait for.
#[test]
fn from_a_terminal_the_help_is_printed_and_the_session_is_not_run() {
    let mut stderr = vec![];

    let code = exit_code(
        "test",
        true,
        || "Usage: jp test".to_owned(),
        || panic!("the session ran"),
        &mut stderr,
    );

    assert_eq!(code, 0);
    assert_eq!(
        String::from_utf8(stderr).unwrap(),
        "Usage: jp test\n\nNote: this binary is a JP plugin. Run it via `jp test`.\n"
    );
}

#[test]
fn the_session_decides_the_exit_status() {
    let mut quiet = vec![];
    let mut failed = vec![];

    let ok = exit_code("test", false, String::new, || Ok(()), &mut quiet);
    let err = exit_code(
        "test",
        false,
        String::new,
        || Err("no host".to_owned()),
        &mut failed,
    );

    assert_eq!((ok, String::from_utf8(quiet).unwrap().as_str()), (0, ""));
    assert_eq!(
        (err, String::from_utf8(failed).unwrap().as_str()),
        (1, "Fatal: no host\n")
    );
}
