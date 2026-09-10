//! The projection has to survive the dialect and still catch a real difference.
//!
//! Until both suites are recorded there is nothing on disk to compare, and the
//! fixture-driven comparisons pass by having no work to do.
//! These drive the same projection over bodies written here, so the comparison
//! is known to work before it is handed fixtures.

use jp_config::model::id::ProviderId;
use serde_json::{Value, json};

use crate::provider::provider_test_support;

/// One request in two dialects.
///
/// The API takes system content as a `system`-role message and marks cache
/// breakpoints; the subscription host takes it in `instructions`, rejects the
/// markers, and rejects several parameters outright.
fn openai_dialects() -> (Value, Value) {
    let api = json!({
        "model": "gpt-5.6-luna",
        "input": [
            {
                "type": "message",
                "role": "system",
                "content": [{ "type": "input_text", "text": "You are JP." }]
            },
            {
                "type": "message",
                "role": "user",
                "content": "hello",
                "prompt_cache_breakpoint": { "type": "ephemeral" }
            }
        ],
        "instructions": null,
        "max_output_tokens": 4096,
        "temperature": 0.7,
        "tools": [{ "type": "function", "name": "run_me" }],
        "tool_choice": "auto",
        "prompt_cache_key": "jp:conversation:1"
    });

    let subscription = json!({
        "model": "gpt-5.6-luna",
        "input": [
            { "type": "message", "role": "user", "content": "hello" }
        ],
        "instructions": "You are JP.",
        "tools": [{ "type": "function", "name": "run_me" }],
        "tool_choice": "auto",
        "prompt_cache_key": "jp:conversation:1"
    });

    (api, subscription)
}

#[test]
fn the_same_request_in_two_dialects_compares_equal() {
    let support = provider_test_support(ProviderId::Openai);
    let (api, subscription) = openai_dialects();

    assert_eq!(
        support.project_request(&api),
        support.project_request(&subscription)
    );
}

#[test]
fn a_tool_dropped_on_one_route_is_caught() {
    let support = provider_test_support(ProviderId::Openai);
    let (api, mut subscription) = openai_dialects();
    subscription["tools"] = json!([]);

    assert_ne!(
        support.project_request(&api),
        support.project_request(&subscription),
        "a route that sent no tools must not compare equal to one that did"
    );
}

#[test]
fn a_changed_system_prompt_is_caught() {
    let support = provider_test_support(ProviderId::Openai);
    let (api, mut subscription) = openai_dialects();
    subscription["instructions"] = json!("You are someone else.");

    assert_ne!(
        support.project_request(&api),
        support.project_request(&subscription)
    );
}

#[test]
fn a_dropped_conversation_turn_is_caught() {
    let support = provider_test_support(ProviderId::Openai);
    let (api, mut subscription) = openai_dialects();
    subscription["input"] = json!([]);

    assert_ne!(
        support.project_request(&api),
        support.project_request(&subscription)
    );
}

/// Two recordings never share an id the host minted, so comparing them raw
/// would fail on every scenario that replays one.
#[test]
fn host_assigned_ids_do_not_defeat_the_comparison() {
    let support = provider_test_support(ProviderId::Openai);

    let left = json!({
        "input": [
            { "type": "function_call", "call_id": "call_aaa", "name": "run_me" },
            { "type": "function_call_output", "call_id": "call_aaa", "output": "working!" }
        ]
    });
    let right = json!({
        "input": [
            { "type": "function_call", "call_id": "call_zzz", "name": "run_me" },
            { "type": "function_call_output", "call_id": "call_zzz", "output": "working!" }
        ]
    });

    assert_eq!(
        support.project_request(&left),
        support.project_request(&right)
    );
}

/// Numbering must not cost the pairing.
#[test]
fn a_tool_result_paired_to_the_wrong_call_is_caught() {
    let support = provider_test_support(ProviderId::Openai);

    let correct = json!({
        "input": [
            { "type": "function_call", "call_id": "call_aaa", "name": "run_me" },
            { "type": "function_call", "call_id": "call_bbb", "name": "run_me" },
            { "type": "function_call_output", "call_id": "call_aaa", "output": "working!" }
        ]
    });
    let mispaired = json!({
        "input": [
            { "type": "function_call", "call_id": "call_xxx", "name": "run_me" },
            { "type": "function_call", "call_id": "call_yyy", "name": "run_me" },
            { "type": "function_call_output", "call_id": "call_yyy", "output": "working!" }
        ]
    });

    assert_ne!(
        support.project_request(&correct),
        support.project_request(&mispaired),
        "a result answering the second call must not compare equal to one answering the first"
    );
}

/// A request after the first turn replays the model's earlier answer, which no
/// two runs word identically.
#[test]
fn the_models_own_wording_does_not_defeat_the_comparison() {
    let support = provider_test_support(ProviderId::Openai);

    let turn = |text| {
        json!({
            "input": [
                { "type": "message", "role": "user", "content": "hello" },
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [{ "type": "output_text", "text": text }]
                }
            ]
        })
    };

    assert_eq!(
        support.project_request(&turn("The tool returned: **42**")),
        support.project_request(&turn("The tool returned: \u{201c}42.\u{201d}"))
    );
}

/// Wording goes; the turn itself does not.
#[test]
fn a_dropped_assistant_turn_is_caught() {
    let support = provider_test_support(ProviderId::Openai);

    let kept = json!({
        "input": [
            { "type": "message", "role": "user", "content": "hello" },
            {
                "type": "message",
                "role": "assistant",
                "content": [{ "type": "output_text", "text": "anything" }]
            }
        ]
    });
    let dropped = json!({
        "input": [{ "type": "message", "role": "user", "content": "hello" }]
    });

    assert_ne!(
        support.project_request(&kept),
        support.project_request(&dropped)
    );
}

/// The user's own words are JP's to send, so they stay compared.
#[test]
fn a_changed_user_turn_is_caught() {
    let support = provider_test_support(ProviderId::Openai);

    let turn =
        |content| json!({ "input": [{ "type": "message", "role": "user", "content": content }] });

    assert_ne!(
        support.project_request(&turn("hello")),
        support.project_request(&turn("something else"))
    );
}

/// `input` names a tool call's arguments in one dialect and the whole
/// conversation in another, so no shared reading of it could be right.
#[test]
fn input_is_read_as_each_dialect_means_it() {
    let openai = provider_test_support(ProviderId::Openai);
    let anthropic = provider_test_support(ProviderId::Anthropic);

    let conversation = json!({
        "input": [{ "type": "message", "role": "user", "content": "hello" }]
    });
    assert_eq!(
        openai
            .project_request(&conversation)
            .get("input")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1),
        "OpenAI's conversation must not be mistaken for a tool call's arguments"
    );

    let call = |arguments| {
        json!({
            "messages": [{
                "role": "assistant",
                "content": [{
                    "type": "tool_use", "id": "toolu_x", "name": "run_me", "input": arguments
                }]
            }]
        })
    };

    assert_eq!(
        anthropic.project_request(&call(json!({ "bar": "foo" }))),
        anthropic.project_request(&call(json!({ "bar": "foo", "foo": "foo" }))),
        "the arguments a model chose vary between runs"
    );
}

/// Anthropic reaches one host both ways, so its dialects differ only by the
/// identity line its subscription transport mandates.
#[test]
fn anthropics_identity_line_is_projected_away() {
    let support = provider_test_support(ProviderId::Anthropic);

    let api = json!({
        "system": [{ "type": "text", "text": "You are JP." }],
        "messages": [{ "role": "user", "content": "hello" }]
    });
    let subscription = json!({
        "system": [
            {
                "type": "text",
                "text": "You are Claude Code, Anthropic's official CLI for Claude."
            },
            { "type": "text", "text": "You are JP." }
        ],
        "messages": [{ "role": "user", "content": "hello" }]
    });

    assert_eq!(
        support.project_request(&api),
        support.project_request(&subscription)
    );
}

/// The identity line is stripped; the prompt it precedes is not.
#[test]
fn anthropics_projection_keeps_the_real_system_prompt() {
    let support = provider_test_support(ProviderId::Anthropic);

    let api = json!({
        "system": [{ "type": "text", "text": "You are JP." }],
        "messages": [{ "role": "user", "content": "hello" }]
    });
    let subscription = json!({
        "system": [
            {
                "type": "text",
                "text": "You are Claude Code, Anthropic's official CLI for Claude."
            },
            { "type": "text", "text": "You are someone else." }
        ],
        "messages": [{ "role": "user", "content": "hello" }]
    });

    assert_ne!(
        support.project_request(&api),
        support.project_request(&subscription)
    );
}
