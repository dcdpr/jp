use super::*;
use crate::message::ComposeMode;

const INIT: &str = r#"{"type":"init","version":12,"workspace":{"root":"/w","storage":"/w/.jp","id":"abc"},"config":{}}"#;

const MANIFEST: &str = crate::manifest!(
    protocol: 12,
    description: "Probe the protocol",
    command: ["probe"],
);

const PACKAGE: Package = Package {
    version: "1.2.3",
    authors: "Ada <ada@example.com>:Bob <bob@example.com>",
    repository: "",
};

fn written(stdout: Vec<u8>) -> String {
    String::from_utf8(stdout).unwrap()
}

fn never_described() -> Result<DescribeResponse, String> {
    panic!("`describe` asked for on an `init`")
}

#[test]
fn start_answers_describe_and_ends_the_session() {
    let mut stdout = vec![];

    let init = start(
        &mut "{\"type\":\"describe\"}\n".as_bytes(),
        &mut stdout,
        "jp-probe",
        12,
        || describe(MANIFEST, "probe", "Usage: probe".to_owned(), PACKAGE),
    )
    .unwrap();

    assert_eq!(init, None);
    let sent: PluginToHost = serde_json::from_str(written(stdout).trim()).unwrap();
    let PluginToHost::Describe(describe) = sent else {
        panic!("expected a describe answer, got {sent:?}");
    };
    assert_eq!(describe.name, "probe");
    assert_eq!(describe.version, "1.2.3");
    assert_eq!(describe.help, "Usage: probe");
    assert_eq!(
        describe.author.as_deref(),
        Some("Ada <ada@example.com>, Bob <bob@example.com>")
    );
    assert_eq!(describe.repository, None);
}

#[test]
fn start_readies_a_host_that_speaks_the_protocol() {
    let mut stdout = vec![];

    let init = start(
        &mut format!("{INIT}\n").as_bytes(),
        &mut stdout,
        "jp-probe",
        12,
        never_described,
    )
    .unwrap();

    assert_eq!(init.map(|init| init.version), Some(12));
    assert_eq!(written(stdout), "{\"type\":\"ready\",\"protocol\":12}\n");
}

#[test]
fn start_ends_the_session_with_an_older_host() {
    let mut stdout = vec![];

    let init = start(
        &mut format!("{INIT}\n").as_bytes(),
        &mut stdout,
        "jp-probe",
        13,
        never_described,
    )
    .unwrap();

    assert_eq!(init, None);
    assert_eq!(
        written(stdout),
        "{\"type\":\"exit\",\"code\":1,\"reason\":\"this plugin needs `jp` protocol 13, and this \
         `jp` speaks 12. Reinstall the two together.\"}\n"
    );
}

#[test]
fn start_refuses_a_first_message_that_is_neither_init_nor_describe() {
    let mut stdout = vec![];

    let error = start(
        &mut "{\"type\":\"shutdown\"}\n".as_bytes(),
        &mut stdout,
        "jp-probe",
        12,
        never_described,
    )
    .unwrap_err();

    assert_eq!(error, "expected init or describe, got: Shutdown");
    assert_eq!(written(stdout), "");
}

#[test]
fn read_names_the_binary_when_the_host_stops_responding() {
    let error = read(&mut "".as_bytes(), "jp-probe").unwrap_err();

    assert_eq!(
        error,
        "The host stopped responding. Are `jp` and `jp-probe` the same build?"
    );
}

fn line_request() -> ComposeRequest {
    ComposeRequest {
        id: None,
        message: "Name?".to_owned(),
        mode: ComposeMode::Line { default: None },
        help: None,
    }
}

#[test]
fn compose_returns_what_the_user_wrote() {
    let mut stdout = vec![];

    let text = compose(
        &mut "{\"type\":\"composed\",\"text\":\"Ada\"}\n".as_bytes(),
        &mut stdout,
        "jp-probe",
        line_request(),
    );

    assert_eq!(text, Ok("Ada".to_owned()));
    assert_eq!(
        written(stdout),
        "{\"type\":\"compose\",\"message\":\"Name?\",\"mode\":{\"kind\":\"line\"}}\n"
    );
}

#[test]
fn compose_reports_a_cancelled_prompt_an_error_and_a_shutdown() {
    let answers = [
        (
            "{\"type\":\"composed\"}\n",
            "Nothing composed; run it again with what it asked for given as flags.",
        ),
        (
            "{\"type\":\"error\",\"message\":\"no terminal\"}\n",
            "no terminal",
        ),
        ("{\"type\":\"shutdown\"}\n", "Interrupted."),
    ];

    for (answer, expected) in answers {
        let text = compose(
            &mut answer.as_bytes(),
            &mut vec![],
            "jp-probe",
            line_request(),
        );

        assert_eq!(text, Err(expected.to_owned()), "answered with {answer}");
    }
}

#[test]
fn answer_tool_sends_the_outcome_then_exits() {
    let mut stdout = vec![];

    answer_tool(&mut stdout, &serde_json::json!({ "type": "success" })).unwrap();

    assert_eq!(
        written(stdout),
        "{\"type\":\"tool_outcome\",\"outcome\":{\"type\":\"success\"}}\n{\"type\":\"exit\",\"\
         code\":0}\n"
    );
}

#[test]
fn a_result_code_is_expected_and_a_failure_is_not() {
    let mut stdout = vec![];

    result(&mut stdout, 1).unwrap();
    result(&mut stdout, 0).unwrap();
    exit(&mut stdout, 1, Some("broke")).unwrap();

    assert_eq!(
        written(stdout),
        "{\"type\":\"exit\",\"code\":1,\"expected\":true}\n{\"type\":\"exit\",\"code\":0}\n{\"\
         type\":\"exit\",\"code\":1,\"reason\":\"broke\"}\n"
    );
}

#[test]
fn print_sends_the_text_on_the_channel_in_the_format() {
    let mut stdout = vec![];

    print(&mut stdout, "{}", "content", "json").unwrap();

    assert_eq!(
        written(stdout),
        "{\"type\":\"print\",\"text\":\"{}\",\"channel\":\"content\",\"format\":\"json\"}\n"
    );
}
