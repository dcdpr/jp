//! Folding system messages into `instructions` for the subscription host.

use openai_responses::types::{
    ContentInput, ContentItem, Input, InputListItem, InputMessage, Model, PromptCacheBreakpoint,
    PromptCacheBreakpointMode, PromptCacheOptions, Request, Role,
};

use super::{fold_system_into_instructions, prepare_subscription_request};

/// The two-message shape JP builds for a plain query: a system prompt split
/// into blocks, then the user's turn.
fn system_then_user(parts: &[&str]) -> Input {
    Input::List(vec![
        InputListItem::Message(InputMessage {
            role: Role::System,
            content: ContentInput::List(
                parts
                    .iter()
                    .map(|text| ContentItem::Text {
                        text: (*text).to_owned(),
                        prompt_cache_breakpoint: None,
                    })
                    .collect(),
            ),
            phase: None,
        }),
        InputListItem::Message(InputMessage {
            role: Role::User,
            content: ContentInput::Text("reply with the single word: ok".to_owned()),
            phase: None,
        }),
    ])
}

fn request(input: Input) -> Request {
    Request {
        model: Model::Other("gpt-5.6-sol".to_owned()),
        input,
        instructions: None,
        ..Default::default()
    }
}

fn items(request: &Request) -> &[InputListItem] {
    let Input::List(items) = &request.input else {
        panic!("expected a list input");
    };

    items
}

#[test]
fn test_system_blocks_move_into_instructions() {
    // The subscription host answers a system-role input entry with
    // `400 System messages are not allowed`, so none may survive the fold.
    let mut request = request(system_then_user(&[
        "You are Jean-Pierre.",
        "<voice>Be direct.</voice>",
    ]));

    fold_system_into_instructions(&mut request);

    assert_eq!(
        request.instructions.as_deref(),
        Some("You are Jean-Pierre.\n\n<voice>Be direct.</voice>")
    );

    let items = items(&request);
    assert_eq!(items.len(), 1);
    assert!(
        matches!(&items[0], InputListItem::Message(m) if matches!(m.role, Role::User)),
        "the user message must survive the fold"
    );
}

#[test]
fn test_existing_instructions_are_kept_after_the_folded_parts() {
    let mut request = request(system_then_user(&["folded"]));
    request.instructions = Some("already set".to_owned());

    fold_system_into_instructions(&mut request);

    assert_eq!(
        request.instructions.as_deref(),
        Some("folded\n\nalready set")
    );
}

#[test]
fn test_a_text_content_system_message_folds_too() {
    let mut request = request(Input::List(vec![InputListItem::Message(InputMessage {
        role: Role::System,
        content: ContentInput::Text("plain text prompt".to_owned()),
        phase: None,
    })]));

    fold_system_into_instructions(&mut request);

    assert_eq!(request.instructions.as_deref(), Some("plain text prompt"));
    assert!(items(&request).is_empty());
}

#[test]
fn test_prepare_drops_the_parameters_the_host_refuses() {
    // The host answers `max_output_tokens` with `400 Unsupported parameter`
    // (measured), and reports only one such parameter per request — so every
    // field the first-party client omits goes in the same pass.
    let mut request = request(system_then_user(&["prompt"]));
    request.max_output_tokens = Some(4096);
    request.temperature = Some(0.7);
    request.top_p = Some(0.9);
    request.metadata = Some(std::collections::HashMap::from([(
        "k".to_owned(),
        "v".to_owned(),
    )]));
    request.user = Some("someone".to_owned());
    request.previous_response_id = Some("resp_1".to_owned());

    prepare_subscription_request(&mut request);

    assert_eq!(request.max_output_tokens, None);
    assert_eq!(request.temperature, None);
    assert_eq!(request.top_p, None);
    assert_eq!(request.truncation, None);
    assert_eq!(request.metadata, None);
    assert_eq!(request.user, None);
    assert_eq!(request.previous_response_id, None);

    // The fold still runs: both reshapings are needed for one accepted
    // request, and testing them apart would let either regress silently.
    assert_eq!(request.instructions.as_deref(), Some("prompt"));
}

#[test]
fn test_prepare_keeps_the_fields_the_host_accepts() {
    let mut request = request(system_then_user(&["prompt"]));
    request.store = Some(false);
    request.prompt_cache_key = Some("cache-1".to_owned());

    prepare_subscription_request(&mut request);

    // A live request carried both and was accepted, so neither may be
    // stripped along with the refused ones.
    assert_eq!(request.store, Some(false));
    assert_eq!(request.prompt_cache_key.as_deref(), Some("cache-1"));
}

#[test]
fn test_prepare_clears_every_explicit_cache_breakpoint() {
    // The model behind the subscription route rejects the whole request over
    // one breakpoint, so a survivor anywhere in the input is a `400`.
    let mut request = request(Input::List(vec![
        InputListItem::Message(InputMessage {
            role: Role::User,
            content: ContentInput::List(vec![
                ContentItem::Text {
                    text: "attachment".to_owned(),
                    prompt_cache_breakpoint: Some(PromptCacheBreakpoint {
                        mode: PromptCacheBreakpointMode::Explicit,
                    }),
                },
                ContentItem::Text {
                    text: "another".to_owned(),
                    prompt_cache_breakpoint: None,
                },
            ]),
            phase: None,
        }),
        InputListItem::Message(InputMessage {
            role: Role::User,
            content: ContentInput::List(vec![ContentItem::Text {
                text: "turn".to_owned(),
                prompt_cache_breakpoint: Some(PromptCacheBreakpoint {
                    mode: PromptCacheBreakpointMode::Explicit,
                }),
            }]),
            phase: None,
        }),
    ]));
    request.prompt_cache_options = Some(PromptCacheOptions::default());
    request.prompt_cache_key = Some("cache-1".to_owned());

    prepare_subscription_request(&mut request);

    let Input::List(items) = &request.input else {
        panic!("expected a list input");
    };
    for item in items {
        let InputListItem::Message(message) = item else {
            continue;
        };
        let ContentInput::List(blocks) = &message.content else {
            continue;
        };
        for block in blocks {
            let ContentItem::Text {
                prompt_cache_breakpoint,
                text,
            } = block
            else {
                continue;
            };
            assert!(
                prompt_cache_breakpoint.is_none(),
                "breakpoint survived on {text:?}"
            );
        }
    }

    assert!(request.prompt_cache_options.is_none());
    // The key is the half the host does read.
    assert_eq!(request.prompt_cache_key.as_deref(), Some("cache-1"));
}

#[test]
fn test_a_request_without_system_messages_is_untouched() {
    let mut request = request(Input::List(vec![InputListItem::Message(InputMessage {
        role: Role::User,
        content: ContentInput::Text("hello".to_owned()),
        phase: None,
    })]));

    fold_system_into_instructions(&mut request);

    assert_eq!(request.instructions, None);
    assert_eq!(items(&request).len(), 1);
}
