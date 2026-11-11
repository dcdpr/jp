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

#[test]
fn the_request_asks_one_choice_between_the_actions() {
    let body = request_body(&options(), "Last message:\nAda Client: thanks!");

    assert_eq!(body["model"], "jev-latest");
    assert_eq!(body["state"], "Last message:\nAda Client: thanks!");
    assert_eq!(body["questions"]["action"]["type"], "choice");
    assert_eq!(
        body["questions"]["action"]["instructions"],
        "What should the assistant do?"
    );
    let mut options: Vec<&String> = body["questions"]["action"]["criteria"]
        .as_object()
        .unwrap()
        .keys()
        .collect();
    options.sort();
    assert_eq!(options, ["answer", "ignore", "new_conversation"]);
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
        verdict(&body),
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
            json!({ "answers": { "action": { "choice": "compact", "probabilities": { "compact": 1.0 } } } }),
            "the decide API chose `compact`, which isn't an option",
        ),
        (
            json!({ "answers": { "action": { "choice": "answer", "probabilities": { "ignore": 1.0 } } } }),
            "the decide API's answer has no probability for `answer`",
        ),
    ];

    for (body, error) in cases {
        assert_eq!(verdict(&body), Err(error.to_owned()), "{body}");
    }
}
