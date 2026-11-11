use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

fn options() -> DecideOptions {
    DecideOptions {
        url: "https://api.typesafe.ai/v1/systemone".to_owned(),
        model: "jev-latest".to_owned(),
        api_key_env: "JEV_API_KEY".to_owned(),
        threshold: 0.5,
        instructions: "Is this for the assistant?".to_owned(),
    }
}

#[test]
fn the_request_asks_one_yes_or_no_question_about_the_chat() {
    assert_eq!(
        request_body(&options(), "Last message:\nAda Client: thanks!"),
        json!({
            "model": "jev-latest",
            "state": "Last message:\nAda Client: thanks!",
            "questions": {
                "respond": { "type": "noul", "instructions": "Is this for the assistant?" }
            }
        })
    );
}

/// The answer from the API's documentation, with the question renamed.
#[test]
fn the_answer_is_the_probability_of_yes() {
    let body = json!({
        "model": "jev-latest",
        "answers": { "respond": { "type": "noul", "noul": 0.98 } },
        "usage": { "input_tokens": 120, "output_tokens": 12 }
    });

    assert_eq!(probability(&body), Ok(0.98));
}

#[test]
fn an_answer_without_a_probability_is_an_error() {
    let missing = json!({ "answers": { "billing": { "type": "noul", "noul": 0.98 } } });
    let out_of_range = json!({ "answers": { "respond": { "type": "noul", "noul": 1.5 } } });

    for body in [missing, out_of_range] {
        assert_eq!(
            probability(&body),
            Err("the decide API's answer has no probability at `answers.respond.noul`".to_owned())
        );
    }
}
