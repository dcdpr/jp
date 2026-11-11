use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

fn options() -> DecideOptions {
    DecideOptions {
        url: "https://api.typesafe.ai/v1/systemone".to_owned(),
        model: "jev-latest".to_owned(),
        api_key_env: Some("JEV_API_KEY".to_owned()),
        threshold: 0.5,
        instructions: "What should the assistant do?".to_owned(),
    }
}

/// The options a request offers, sorted.
fn offered(body: &Value) -> Vec<&String> {
    let mut options: Vec<&String> = body["questions"]["action"]["criteria"]
        .as_object()
        .unwrap()
        .keys()
        .collect();
    options.sort();
    options
}

#[test]
fn a_message_that_may_give_a_command_is_offered_the_commands() {
    let body = request_body(
        &options(),
        "Last message:\nAda Client (to you): compact please",
        Action::offered(true),
    );

    assert_eq!(body["model"], "jev-latest");
    assert_eq!(
        body["state"],
        "Last message:\nAda Client (to you): compact please"
    );
    assert_eq!(body["questions"]["action"]["type"], "choice");
    assert_eq!(
        body["questions"]["action"]["instructions"],
        "What should the assistant do?"
    );
    assert_eq!(offered(&body), [
        "answer",
        "compact",
        "ignore",
        "new_conversation"
    ]);
}

#[test]
fn any_other_message_is_offered_answering_or_ignoring_it() {
    let body = request_body(
        &options(),
        "Last message:\nAda Client: thanks!",
        Action::offered(false),
    );

    assert_eq!(offered(&body), ["answer", "ignore"]);
}

/// Shaped like the `choice` answer in Jev's documentation.
#[test]
fn the_answer_is_the_chosen_action_and_its_probability() {
    let body = json!({
        "model": "jev-1.13.0",
        "answers": {
            "action": {
                "type": "choice",
                "choice": "new_conversation",
                "confidence": 0.62,
                "probabilities": { "ignore": 0.05, "answer": 0.13, "new_conversation": 0.82 }
            }
        },
        "usage": { "input_tokens": 320, "output_tokens": 30 }
    });

    assert_eq!(
        verdict(&body, Action::offered(true)),
        Ok(Verdict {
            action: Action::NewConversation,
            probability: 0.82,
        })
    );
}

#[test]
fn an_answer_the_bridge_cant_act_on_is_an_error() {
    let cases = [
        (
            json!({ "answers": {} }),
            "the decide API's answer has no `answers.action`",
        ),
        (
            json!({ "answers": { "action": { "probabilities": {} } } }),
            "the decide API's answer has no `answers.action.choice`",
        ),
        (
            json!({ "answers": { "action": { "choice": "summarize", "probabilities": { "summarize": 1.0 } } } }),
            "the decide API chose `summarize`, which isn't an option",
        ),
        (
            json!({ "answers": { "action": { "choice": "answer", "probabilities": { "ignore": 1.0 } } } }),
            "the decide API's answer has no probability for `answer`",
        ),
    ];

    for (body, error) in cases {
        assert_eq!(
            verdict(&body, Action::offered(true)),
            Err(error.to_owned()),
            "{body}"
        );
    }
}

/// A command the message wasn't offered isn't taken, even if a server picks it.
#[test]
fn a_command_that_wasnt_offered_is_an_error() {
    let body = json!({ "answers": { "action": { "choice": "compact", "probabilities": { "compact": 0.9 } } } });

    assert_eq!(
        verdict(&body, Action::offered(false)),
        Err("the decide API chose `compact`, which isn't an option".to_owned())
    );
}
