use pretty_assertions::assert_eq;

use super::*;

fn own() -> Vec<String> {
    vec![
        "+31600000000".to_owned(),
        "44444444-4444-4444-8444-444444444444".to_owned(),
    ]
}

fn account() -> Account {
    Account {
        ids: own(),
        attachments: "/data/signal-cli/attachments".into(),
    }
}

fn read(line: &str) -> Line {
    parse(line, &account())
}

/// Direct message notification in `signal-cli jsonRpc` default receive mode
/// (`signal-cli-jsonrpc(5)`).
const DIRECT: &str = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"source":"+33123456789","sourceNumber":"+33123456789","sourceUuid":"11111111-1111-4111-8111-111111111111","sourceName":"name","sourceDevice":1,"timestamp":1631458508784,"dataMessage":{"timestamp":1631458508784,"message":"What is being worked on?","expiresInSeconds":0,"viewOnce":false,"mentions":[],"attachments":[],"contacts":[]}},"account":"+31600000000"}}"#;

#[test]
fn a_direct_message_is_from_its_sender_in_a_chat_of_their_own() {
    assert_eq!(
        read(DIRECT),
        Line::Message(Inbound {
            chat: Chat::Direct("11111111-1111-4111-8111-111111111111".to_owned()),
            sender: vec![
                "11111111-1111-4111-8111-111111111111".to_owned(),
                "+33123456789".to_owned()
            ],
            name: Some("name".to_owned()),
            addressed: false,
            leading_mention: false,
            text: "What is being worked on?".to_owned(),
            attachments: vec![],
        })
    );
}

#[test]
fn a_group_message_in_subscription_mode_is_in_the_group() {
    let line = r#"{"jsonrpc":"2.0","method":"receive","params":{"subscription":0,"result":{"envelope":{"sourceUuid":"22222222-2222-4222-8222-222222222222","sourceNumber":null,"dataMessage":{"message":"See attached","attachments":[{"contentType":"image/png"}],"groupInfo":{"groupId":"Z3JvdXA=","type":"DELIVER"}}},"account":"+31600000000"}}}"#;

    assert_eq!(
        read(line),
        Line::Message(Inbound {
            chat: Chat::Group("Z3JvdXA=".to_owned()),
            sender: vec!["22222222-2222-4222-8222-222222222222".to_owned()],
            name: None,
            addressed: false,
            leading_mention: false,
            text: "See attached".to_owned(),
            attachments: vec![Attachment {
                content_type: "image/png".to_owned(),
                name: None,
                path: None,
            }],
        })
    );
}

/// Real `signal-cli` 0.14.9 envelope (with anonymized IDs) from a sender with
/// phone-number privacy enabled (`sourceNumber: null`).
#[test]
fn a_sender_with_a_hidden_number_is_named_by_account_id() {
    let line = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"source":"33333333-3333-4333-8333-333333333333","sourceNumber":null,"sourceUuid":"33333333-3333-4333-8333-333333333333","sourceName":"Ada Client","sourceDevice":1,"timestamp":1794057460077,"serverReceivedTimestamp":1794057460167,"serverDeliveredTimestamp":1794057503074,"dataMessage":{"timestamp":1794057460077,"message":"ping","expiresInSeconds":0,"isExpirationUpdate":false,"viewOnce":false,"isEndSession":false,"isProfileKeyUpdate":false,"hasProfileKey":true}},"account":"+31600000000"}}"#;

    assert_eq!(
        read(line),
        Line::Message(Inbound {
            chat: Chat::Direct("33333333-3333-4333-8333-333333333333".to_owned()),
            sender: vec!["33333333-3333-4333-8333-333333333333".to_owned()],
            name: Some("Ada Client".to_owned()),
            addressed: false,
            leading_mention: false,
            text: "ping".to_owned(),
            attachments: vec![],
        })
    );
}

#[test]
fn an_attachment_is_where_signal_cli_saved_it() {
    let line = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"sourceUuid":"33333333-3333-4333-8333-333333333333","dataMessage":{"message":null,"attachments":[{"contentType":"image/png","filename":"crash.png","id":"Xd9f-2a_k.png","size":2048}]}},"account":"+31600000000"}}"#;

    let Line::Message(message) = read(line) else {
        panic!("not a message: {:?}", read(line));
    };

    assert_eq!(message.attachments, vec![Attachment {
        content_type: "image/png".to_owned(),
        name: Some("crash.png".to_owned()),
        path: Some("/data/signal-cli/attachments/Xd9f-2a_k.png".into()),
    }]);
}

#[test]
fn an_attachment_id_that_is_not_a_file_name_gets_no_path() {
    for id in ["../../.ssh/id_ed25519", ".hidden", "a/b.png"] {
        let line = format!(
            r#"{{"jsonrpc":"2.0","method":"receive","params":{{"envelope":{{"sourceUuid":"u","dataMessage":{{"attachments":[{{"contentType":"image/png","id":"{id}"}}]}}}}}}}}"#
        );

        let Line::Message(message) = read(&line) else {
            panic!("not a message: {:?}", read(&line));
        };
        assert_eq!(message.attachments[0].path, None, "{id}");
    }
}

#[test]
fn a_group_invitation_is_ignored() {
    let line = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"source":"33333333-3333-4333-8333-333333333333","sourceNumber":null,"sourceUuid":"33333333-3333-4333-8333-333333333333","sourceName":"Ada Client","sourceDevice":1,"timestamp":1794057277801,"dataMessage":{"timestamp":1794057277801,"message":null,"expiresInSeconds":0,"hasProfileKey":true,"groupInfo":{"groupId":"Z3JvdXA=","groupName":"Work Log","revision":0,"type":"DELIVER"}}},"account":"+31600000000"}}"#;

    assert_eq!(read(line), Line::Ignored);
}

fn group_message(text: &str, mentions: &str, quote: &str) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","method":"receive","params":{{"envelope":{{"sourceUuid":"33333333-3333-4333-8333-333333333333","sourceNumber":null,"sourceName":"Ada Client","dataMessage":{{"message":{text},"mentions":{mentions}{quote},"groupInfo":{{"groupId":"Z3JvdXA=","type":"DELIVER"}}}}}},"account":"+31600000000"}}}}"#,
        text = serde_json::to_string(text).unwrap(),
    )
}

fn addressed(line: &str) -> (String, bool) {
    match read(line) {
        Line::Message(message) => (message.text, message.addressed),
        other => panic!("expected a message, got {other:?}"),
    }
}

fn leading_mention(line: &str) -> bool {
    match read(line) {
        Line::Message(message) => message.leading_mention,
        other => panic!("expected a message, got {other:?}"),
    }
}

#[test]
fn only_a_message_starting_with_a_mention_of_the_bridge_leads_with_it() {
    let leading = group_message(
        " \u{fffc} compact please",
        r#"[{"number":null,"uuid":"44444444-4444-4444-8444-444444444444","start":1,"length":1}]"#,
        "",
    );
    let later = group_message(
        "so \u{fffc}, compact please",
        r#"[{"number":"+31600000000","uuid":null,"start":3,"length":1}]"#,
        "",
    );
    let someone_else = group_message(
        "\u{fffc} compact please",
        r#"[{"number":"+31622222222","uuid":null,"start":0,"length":1}]"#,
        "",
    );

    assert!(leading_mention(&leading));
    assert!(!leading_mention(&later));
    assert!(!leading_mention(&someone_else));
}

#[test]
fn a_mention_of_the_bridge_addresses_the_message() {
    let by_id = group_message(
        "\u{fffc} what's next?",
        r#"[{"name":"44444444-4444-4444-8444-444444444444","number":null,"uuid":"44444444-4444-4444-8444-444444444444","start":0,"length":1}]"#,
        "",
    );
    assert_eq!(addressed(&by_id), (" what's next?".to_owned(), true));

    let by_number = group_message(
        "so \u{fffc}, what's next?",
        r#"[{"number":"+31600000000","uuid":null,"start":3,"length":1}]"#,
        "",
    );
    assert_eq!(
        addressed(&by_number),
        ("so , what's next?".to_owned(), true)
    );
}

/// Emoji outside the BMP occupy two UTF-16 code units, so mention `start`
/// offsets after an emoji must be indexed in UTF-16 units rather than `char`s.
#[test]
fn a_mention_of_someone_else_is_named_and_not_addressed() {
    let line = group_message(
        "\u{1f389} \u{fffc} and \u{fffc}: done?",
        r#"[{"number":"+31622222222","uuid":"55555555-5555-4555-8555-555555555555","start":3,"length":1},{"number":null,"uuid":"66666666-6666-4666-8666-666666666666","start":9,"length":1}]"#,
        "",
    );

    assert_eq!(
        addressed(&line),
        (
            "\u{1f389} @+31622222222 and @someone: done?".to_owned(),
            false
        )
    );
}

#[test]
fn a_reply_to_the_bridge_addresses_the_message() {
    let to_bridge = group_message(
        "And the one after?",
        "[]",
        r#","quote":{"id":1,"authorNumber":null,"authorUuid":"44444444-4444-4444-8444-444444444444","text":"T-1"}"#,
    );
    assert_eq!(
        addressed(&to_bridge),
        ("And the one after?".to_owned(), true)
    );

    let to_someone = group_message(
        "Agreed",
        "[]",
        r#","quote":{"id":1,"authorNumber":null,"authorUuid":"55555555-5555-4555-8555-555555555555","text":"Lunch?"}"#,
    );
    assert_eq!(addressed(&to_someone), ("Agreed".to_owned(), false));
}

#[test]
fn everything_but_a_written_message_is_ignored() {
    for line in [
        r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"sourceUuid":"u","receiptMessage":{"isRead":true}}}}"#,
        r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"sourceUuid":"u","typingMessage":{"action":"STARTED"}}}}"#,
        r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"sourceUuid":"u","syncMessage":{"sentMessage":{"message":"j"}}}}}"#,
        r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"sourceUuid":"u","dataMessage":{"message":null,"reaction":{"emoji":"👍"}}}}}"#,
        r#"{"jsonrpc":"2.0","result":{"timestamp":999},"id":"4"}"#,
        "not json",
    ] {
        assert_eq!(read(line), Line::Ignored, "{line}");
    }
}

#[test]
fn a_failed_request_is_reported() {
    assert_eq!(
        read(r#"{"jsonrpc":"2.0","error":{"code":-1,"message":"Unregistered user"},"id":"7"}"#),
        Line::Failed("Unregistered user".to_owned())
    );
}

/// Compared as parsed JSON: key order depends on whether another crate in the
/// build enables `serde_json`'s `preserve_order`.
#[test]
fn a_send_names_its_recipient_and_carries_the_styles() {
    let parsed = |line: String| serde_json::from_str::<Value>(&line).unwrap();

    assert_eq!(
        parsed(request(
            3,
            "send",
            &send_params(&Chat::Direct("+33123456789".to_owned()), "Filed **T-1**")
        )),
        parsed(
            r#"{"id":"3","jsonrpc":"2.0","method":"send","params":{"message":"Filed T-1","recipient":["+33123456789"],"textStyle":["6:3:BOLD"]}}"#
                .to_owned()
        )
    );
    assert_eq!(
        parsed(request(
            4,
            "send",
            &send_params(&Chat::Group("Z3JvdXA=".to_owned()), "Hi")
        )),
        parsed(
            r#"{"id":"4","jsonrpc":"2.0","method":"send","params":{"groupId":"Z3JvdXA=","message":"Hi"}}"#
                .to_owned()
        )
    );
}
