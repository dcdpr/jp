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

/// Strip explicit prompt-cache breakpoints and `mode: Explicit`.
///
/// The subscription host rejects `prompt_cache_breakpoint` on content blocks
/// with `400 prompt_cache_breakpoint is not supported on this model`, even on
/// models that accept it via the platform API.
/// `prompt_cache_options.mode` is dropped as well, so `cache = "off"` cannot
/// disable caching on this route.
///
/// `prompt_cache_options.ttl` and `prompt_cache_key` are preserved: the
/// subscription host accepts `ttl` without `mode` and echoes `prompt_cache_key`
/// on the response as its cache key.
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

/// Strip request parameters that the subscription host rejects.
///
/// `max_output_tokens` is rejected with `400 Unsupported parameter`.
/// The remaining fields (`temperature`, `top_p`, `truncation`, `metadata`,
/// `user`, `previous_response_id`) are omitted by the Codex CLI as well;
/// stripping them up front avoids discovering unsupported parameters one `400`
/// at a time.
fn strip_unsupported_parameters(request: &mut Request) {
    request.max_output_tokens = None;
    request.temperature = None;
    request.top_p = None;
    request.truncation = None;
    request.metadata = None;
    request.user = None;
    request.previous_response_id = None;
}

/// Fold `role: "system"` input messages into the top-level `instructions`
/// field.
///
/// The subscription host rejects `role: "system"` items in `input` with `400
/// System messages are not allowed` and expects the system prompt in
/// `instructions`.
///
/// Any existing `instructions` text is appended after the folded messages to
/// preserve prompt order.
/// Cache breakpoints on the folded system blocks are discarded because the host
/// derives its cache key from `prompt_cache_key`.
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
