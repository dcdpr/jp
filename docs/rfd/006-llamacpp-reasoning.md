# RFD 006: Llamacpp Reasoning Support

- **Status**: Implemented
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-18

## Summary

The Llamacpp provider silently drops reasoning content when the llama.cpp server
uses its default `--reasoning-format deepseek` setting. This RFD proposes fixing
reasoning extraction to work across all three llama.cpp reasoning formats.

## Motivation

The llama.cpp server exposes an OpenAI-compatible `/v1/chat/completions`
endpoint. When models produce thinking tokens, the server can return them in
three ways depending on the `--reasoning-format` flag:

| Format               | `delta.content`                  | `delta.reasoning_content` |
|----------------------|----------------------------------|---------------------------|
| `deepseek` (default) | Regular content only             | Reasoning content         |
| `none`               | Everything (with `<think>` tags) | Not set                   |
| `deepseek-legacy`    | Everything (with `<think>` tags) | Reasoning content         |

Our Llamacpp provider uses the `openai` Rust crate, which only reads
`delta.content` and `delta.tool_calls`. The `reasoning_content` field is a
non-standard DeepSeek extension that the crate ignores.

The existing `ReasoningExtractor` parses `<think>` tags from `delta.content`,
which works for `none` and `deepseek-legacy` but does nothing for the default
`deepseek` format — there are no tags to extract because the server already
stripped them.

This is the same class of bug we fixed in the Ollama provider, where
`message.content` and `message.thinking` were never processed. The Ollama fix
was straightforward because `ollama-rs` exposes the `thinking` field natively.
For Llamacpp, the `openai` crate does not expose `reasoning_content`.

## Design

From a user's perspective, reasoning content from llama.cpp models appears in
conversations the same way it does for Ollama or Anthropic — no configuration
changes, no new flags. The fix is entirely internal to the Llamacpp provider.

### Drop the `openai` Crate Entirely

The `openai` crate's `ChatCompletionMessageDelta` struct does not include
`reasoning_content`, and the field is unlikely to be added upstream since it's
not part of the OpenAI spec. Rather than forking or patching the crate, we
replace all of its usage — both streaming and request building — with our own
implementation using `reqwest`, `reqwest_eventsource`, and `serde_json`.

The `openai` crate is used for four things, all replaced:

1. **`ChatCompletionDelta::builder()` + `create_stream()`** — replaced with
   manual JSON body construction via `serde_json::json!` and SSE streaming via
   `reqwest_eventsource::EventSource`.
2. **`ChatCompletionMessage` / `ChatCompletionMessageRole`** — conversation
   history is serialized directly to `serde_json::Value` messages.
3. **`ChatCompletionChoiceDelta` / `ChatCompletionMessageDelta`** — replaced
   with local serde types (`StreamChunk`, `StreamChoice`, `StreamDelta`) that
   include the `reasoning_content` field.
4. **`Credentials`** — removed; base URL comes from `LlamacppConfig` directly.

The `Llamacpp` struct simplifies to just `reqwest_client` and `base_url`. Model
listing already uses `reqwest` directly and is unaffected.

### SSE Streaming

The llama.cpp server sends SSE events in this shape:

```json
data: {"choices":[{"delta":{"reasoning_content":"Let me think..."},"index":0}]}
data: {"choices":[{"delta":{"content":"The answer is..."},"index":0}]}
```

We parse each SSE event's JSON ourselves, reading both `reasoning_content` and
`content` from the delta object. The llama.cpp server source
(`common_chat_msg_diff_to_json_oaicompat` in `common/chat.cpp`) is the
authoritative reference for the streaming delta format.

The SSE loop handles:

- `Event::Open` — ignored
- `Event::Message` with `data: [DONE]` — finalize and flush
- `Event::Message` with JSON — parse as `StreamChunk`, route fields to events
- Errors — delegated to the existing `From<reqwest_eventsource::Error> for
  StreamError` classifier in `error.rs`, which handles retry-after extraction,
  `x-should-retry`, and status code classification

### Reasoning Routing

The implementation auto-detects which llama.cpp format is active based on which
fields are present in each delta, with no user configuration required:

| Format            | `reasoning_content` | `content`            | Routing                                  |
|-------------------|---------------------|----------------------|------------------------------------------|
| `deepseek`        | Present             | Regular text         | `reasoning_content` → reasoning events   |
|                   |                     |                      | directly                                 |
| `none`            | Absent              | Everything with tags | `content` → `ReasoningExtractor` parses  |
|                   |                     |                      | `<think>`                                |
| `deepseek-legacy` | Present             | Everything with tags | `reasoning_content` → reasoning;         |
|                   |                     |                      | `content` → text                         |

For `deepseek-legacy`, when both fields are present, `reasoning_content` goes
directly to reasoning events and `content` is treated as regular text (bypassing
the extractor, since the server already separates them).

### Event Indexing

Following the Ollama provider's convention:

- **Index 0**: reasoning content
- **Index 1**: message content (or structured output)
- **Index 2+**: tool calls

The `ToolCallRequestAggregator` is reused unchanged to accumulate partial tool
call JSON chunks across multiple SSE events.

### Flush Strategy

Same as Ollama — reasoning (index 0) flushes when the first content or tool call
chunk arrives. This guarantees reasoning events precede content and tool call
events in the conversation history. On `finish_reason` or `[DONE]`, all
remaining indices flush.

### Request Building

Conversation history serialization produces `serde_json::Value` messages
directly. Reasoning responses from previous turns are wrapped in `<think>` tags
so the model can pick up its own chain-of-thought. Tool definitions use
`parameters_with_strict_mode` from the OpenAI provider module (shared utility,
not the `openai` crate).

## Alternatives

- **Patch the `openai` crate** to add `reasoning_content` to
  `ChatCompletionMessageDelta`. Rejected because the field is a non-standard
  DeepSeek extension unlikely to be accepted upstream, and maintaining a fork
  for a single field is more burden than owning the SSE loop directly.
- **Keep the `openai` crate for request building, replace only streaming.**
  Rejected because the crate's builder, message types, and credential handling
  are tightly coupled — replacing streaming alone leaves dead code and a
  confusing split of responsibilities.

## Non-Goals

- Adding user-facing configuration for `--reasoning-format`. The routing
  auto-detects the format from the delta fields.
- Changing other providers. The Ollama and OpenRouter providers already handle
  reasoning correctly.

## Risks

- **Owning the SSE loop**: Edge cases like malformed events and the `[DONE]`
  sentinel are our responsibility. Mitigated by keeping the loop thin and
  delegating error classification to the existing `reqwest_eventsource`
  classifier.
- **`ReasoningExtractor` buffering latency**: The extractor holds a small tail
  buffer (< 8 bytes) to detect split `<think>` tags. Only affects the `none`
  format path; latency is negligible.

## Implementation

1. Define local serde types for the OpenAI Chat Completions streaming delta
   format: `StreamChunk`, `StreamChoice`, `StreamDelta`, `ToolCallDelta`,
   `FunctionDelta`. The key addition is `reasoning_content` on `StreamDelta`.
2. Replace request building with `build_request()` producing
   `serde_json::Value`. Conversation history, tool definitions, and structured
   output schemas serialize to JSON directly.
3. Replace `openai` crate streaming with `reqwest_eventsource::EventSource`. The
   `handle_sse_event` function processes each SSE event and routes
   reasoning/content/tool-call fields to provider-agnostic `Event` values.
4. Wire up reasoning routing: `reasoning_content` is used directly when present,
   `ReasoningExtractor` as fallback when only `content` is available.
5. Remove all `openai` crate imports from `llamacpp.rs`.
6. Add unit tests for delta parsing across the three formats, request building
   (tool call merging, reasoning wrapping), and tool choice conversion.
7. Re-record VCR cassettes against a live llama.cpp server.

## References

- llama.cpp server reasoning format docs: `--reasoning-format` flag in
  `tools/server/README.md`
- llama.cpp source: `common_chat_msg_diff_to_json_oaicompat` in
  `common/chat.cpp`
- Ollama provider (this codebase): reads `message.thinking` and
  `message.content` directly; uses the same index convention (0/1/2+) and flush
  strategy that this implementation mirrors.
- [yoagent `openai_compat.rs`](https://github.com/yologdev/yoagent/blob/main/src/provider/openai_compat.rs):
  Reference implementation of OpenAI-compatible SSE streaming with
  `reqwest_eventsource`. Provides the serde type patterns and SSE loop structure
  adapted for this implementation.
