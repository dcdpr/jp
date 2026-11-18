//! Rewriting a platform API request for the `ChatGPT` subscription host.
//!
//! [`prepare_subscription_request`] is the entry point: the provider builds one
//! request and rewrites a copy of it for every attempt that sends through a
//! subscription credential.

use openai_responses::types::{self, Request};

/// Reshape a request for the subscription host.
///
/// That host accepts a narrower request than the platform API: it takes the
/// system prompt in a different field, and rejects several parameters outright
/// with `400 Unsupported parameter: <name>`.
pub(super) fn prepare_subscription_request(request: &mut Request) {
    fold_system_into_instructions(request);
    strip_unsupported_parameters(request);
    strip_explicit_cache_controls(request);
}

/// Remove the explicit prompt-cache controls.
///
/// The model behind the subscription route answers a content block carrying
/// `prompt_cache_breakpoint` with `400 prompt_cache_breakpoint is not supported
/// on this model`, even for a model that accepts it through the platform API.
/// Explicit mode in `prompt_cache_options` is dropped with it, as the other
/// half of the same feature, so `cache = off` cannot disable caching here.
///
/// `prompt_cache_options.ttl` and `prompt_cache_key` stay.
/// Codex sends `ttl` alone through this host, and the host echoes the key back
/// on the response as the key it cached under.
fn strip_explicit_cache_controls(request: &mut Request) {
    request.prompt_cache_options = request
        .prompt_cache_options
        .take()
        .and_then(|options| options.ttl)
        .map(|ttl| types::PromptCacheOptions {
            mode: None,
            ttl: Some(ttl),
        });

    let types::Input::List(items) = &mut request.input else {
        return;
    };

    for item in items {
        let types::InputListItem::Message(message) = item else {
            continue;
        };
        let types::ContentInput::List(blocks) = &mut message.content else {
            continue;
        };

        for block in blocks {
            let (types::ContentItem::Text {
                prompt_cache_breakpoint,
                ..
            }
            | types::ContentItem::Image {
                prompt_cache_breakpoint,
                ..
            }
            | types::ContentItem::File {
                prompt_cache_breakpoint,
                ..
            }) = block;

            *prompt_cache_breakpoint = None;
        }
    }
}

/// Drop the parameters the subscription host refuses.
///
/// `max_output_tokens` is refused by name (measured).
/// The rest are dropped because the first-party client does not send them
/// either, and the host reports only one unsupported parameter per request:
/// discovering them one `400` at a time costs a round-trip each.
///
/// Sampling controls go with them.
/// On the API path they are already suppressed for reasoning models, so a
/// subscription request loses nothing it would have applied; a caller who sets
/// them explicitly gets them ignored rather than getting a rejected request.
fn strip_unsupported_parameters(request: &mut Request) {
    request.max_output_tokens = None;
    request.temperature = None;
    request.top_p = None;
    request.truncation = None;
    request.metadata = None;
    request.user = None;
    request.previous_response_id = None;
}

/// Move system-role input messages into the request's `instructions`.
///
/// The subscription host answers a `role: "system"` entry in `input` with `400
/// System messages are not allowed`; it takes the system prompt in
/// `instructions` instead, which is where the Responses API documents it and
/// where the Codex CLI puts it.
///
/// Any `instructions` already set is kept after the folded parts, so the prompt
/// reads in the order it was assembled.
///
/// The cache breakpoint that rode on the last system block is dropped with it.
/// The host derives its own cache key from the request's `prompt_cache_key`, so
/// a breakpoint inside a field it does not read buys nothing.
pub(super) fn fold_system_into_instructions(request: &mut Request) {
    let types::Input::List(items) = &mut request.input else {
        return;
    };

    let mut folded: Vec<String> = vec![];

    items.retain(|item| {
        let types::InputListItem::Message(message) = item else {
            return true;
        };
        if !matches!(message.role, types::Role::System) {
            return true;
        }

        match &message.content {
            types::ContentInput::Text(text) => folded.push(text.clone()),
            types::ContentInput::List(blocks) => {
                folded.extend(blocks.iter().filter_map(|block| match block {
                    types::ContentItem::Text { text, .. } => Some(text.clone()),
                    types::ContentItem::Image { .. } | types::ContentItem::File { .. } => None,
                }));
            }
        }

        false
    });

    if folded.is_empty() {
        return;
    }

    folded.extend(request.instructions.take());
    request.instructions = Some(folded.join("\n\n"));
}
