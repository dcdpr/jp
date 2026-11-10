# RFD 007: Llamacpp Reasoning Support

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-18
- **Summary**: Replaces the `openai` crate in the Llamacpp provider to extract
  reasoning content across all three `llama.cpp` formats.

## Summary

The Llamacpp provider silently drops reasoning content when the `llama.cpp`
server uses its default `--reasoning-format deepseek` setting.
This RFD replaces the `openai` crate with a direct HTTP implementation that
extracts reasoning content across all three `llama.cpp` reasoning formats.

## Motivation

The llama.cpp server exposes an OpenAI-compatible `/v1/chat/completions`
endpoint.
When models produce thinking tokens, the server returns them in one of three
formats depending on the `--reasoning-format` flag:

| Format               | `delta.content`                  | `delta.reasoning_content` |
| -------------------- | -------------------------------- | ------------------------- |
| `deepseek` (default) | Regular content only             | Reasoning content         |
| `none`               | Everything (with `<think>` tags) | Not set                   |
| `deepseek-legacy`    | Everything (with `<think>` tags) | Reasoning content         |

The Llamacpp provider uses the `openai` Rust crate, which reads `delta.content`
and `delta.tool_calls` but ignores the non-standard DeepSeek `reasoning_content`
extension.
The existing `ReasoningExtractor` parses `<think>` tags from `delta.content`,
which works for `none` and `deepseek-legacy` but not for the default `deepseek`
format, where the server has already stripped the tags.

The Ollama provider had the same class of bug when `message.content` and
`message.thinking` went unprocessed, which was straightforward to fix because
`ollama-rs` exposes `thinking` natively.
The `openai` crate offers no equivalent access to `reasoning_content`.

## Design

Reasoning content from llama.cpp models appears in conversations the same way it
does for Ollama or Anthropic, with no configuration changes or new flags.
All changes are internal to the Llamacpp provider.

### Drop the `openai` Crate

The `openai` crate's `ChatCompletionMessageDelta` struct omits
`reasoning_content`, and upstream is unlikely to add a field outside the OpenAI
specification.
Rather than forking or patching the crate, the provider replaces it across both
streaming and request building with `reqwest`, `reqwest_eventsource`, and
`serde_json`.

This replaces all four uses of the `openai` crate:

1. **`ChatCompletionDelta::builder()` + `create_stream()`**: Replaced with JSON
   body construction via `serde_json::json!` and SSE streaming via
   `reqwest_eventsource::EventSource`.
2. **`ChatCompletionMessage` / `ChatCompletionMessageRole`**: Conversation
   history serializes directly to `serde_json::Value` messages.
3. **`ChatCompletionChoiceDelta` / `ChatCompletionMessageDelta`**: Replaced with
   local serde types (`StreamChunk`, `StreamChoice`, `StreamDelta`) that include
   `reasoning_content`.
4. **`Credentials`**: Removed; the base URL comes directly from
   `LlamacppConfig`.

The `Llamacpp` struct simplifies to `reqwest_client` and `base_url`.
Model listing already uses `reqwest` directly and is unaffected.

### SSE Streaming

The llama.cpp server sends SSE events in this shape:

```json
data: {"choices":[{"delta":{"reasoning_content":"Let me think..."},"index":0}]}
data: {"choices":[{"delta":{"content":"The answer is..."},"index":0}]}
```

The provider parses each SSE event payload and reads both `reasoning_content`
and `content` from the delta object.
The llama.cpp server implementation (`common_chat_msg_diff_to_json_oaicompat` in
`common/chat.cpp`) defines the streaming delta format.

The SSE loop handles four cases:

- **`Event::Open`**: Ignored.
- **`Event::Message` with `data: [DONE]`**: Finalizes and flushes open streams.
- **`Event::Message` with JSON**: Deserializes the payload as `StreamChunk` and
  routes its fields to events.
- **Errors**: Delegates to the `From<reqwest_eventsource::Error> for
  StreamError` classifier in `error.rs`, which handles retry-after extraction,
  `x-should-retry`, and status code classification.

### Reasoning Routing

The provider detects the active llama.cpp format from the fields present in each
delta, without user configuration:

| Format            | `reasoning_content` | `content`            | Routing                                 |
| ----------------- | ------------------- | -------------------- | --------------------------------------- |
| `deepseek`        | Present             | Regular text         | `reasoning_content` → reasoning events  |
|                   |                     |                      | directly                                |
| `none`            | Absent              | Everything with tags | `content` → `ReasoningExtractor` parses |
|                   |                     |                      | `<think>`                               |
| `deepseek-legacy` | Present             | Everything with tags | `reasoning_content` → reasoning;        |
|                   |                     |                      | `content` → text                        |

When both fields are present (`deepseek-legacy`), `reasoning_content` routes
directly to reasoning events and `content` is treated as regular text, bypassing
`ReasoningExtractor` because the server already separates them.

### Event Indexing

Streamed events follow the Ollama provider's index convention:

- **Index 0**: Reasoning content
- **Index 1**: Message content (or structured output)
- **Index 2+**: Tool calls

`ToolCallRequestAggregator` is reused unchanged to accumulate partial tool call
JSON chunks across SSE events.

### Flush Strategy

Matching the Ollama provider, reasoning at index 0 flushes when the first
content or tool call chunk arrives, ensuring reasoning events precede content
and tool call events in the conversation history.
On `finish_reason` or `[DONE]`, all remaining indices flush.

### Request Building

Conversation history serializes directly to `serde_json::Value` messages.
Reasoning responses from previous turns are wrapped in `<think>` tags so the
model retains its chain of thought across turns.
Tool definitions use the shared `parameters_with_strict_mode` helper from the
OpenAI provider module rather than the `openai` crate.

## Alternatives

- **Patch the `openai` crate**: Add `reasoning_content` to
  `ChatCompletionMessageDelta`.
  Rejected because `reasoning_content` is a non-standard DeepSeek extension
  unlikely to be accepted upstream, and maintaining a fork for a single field
  costs more than owning the SSE loop directly.
- **Keep the `openai` crate for request building and replace only streaming**:
  Rejected because the crate's builder, message types, and credential handling
  are tightly coupled; replacing streaming alone leaves dead code and splits
  request handling across two implementations.

## Non-Goals

- **User-facing configuration for `--reasoning-format`**: The provider detects
  the active format from the delta fields.
- **Changes to other providers**: The Ollama and OpenRouter providers already
  handle reasoning content.

## Risks

- **Owning the SSE loop**: The provider must handle edge cases such as malformed
  events and the `[DONE]` sentinel directly.
  Keeping the loop thin and delegating error classification to the existing
  `reqwest_eventsource` classifier limits this surface area.
- **`ReasoningExtractor` buffering latency**: The extractor holds a tail buffer
  (< 8 bytes) to detect `<think>` tags split across chunks.
  This only affects the `none` format path, and the added latency is negligible.

## Implementation

1. Define local serde types for the OpenAI Chat Completions streaming delta
   format (`StreamChunk`, `StreamChoice`, `StreamDelta`, `ToolCallDelta`, and
   `FunctionDelta`), including `reasoning_content` on `StreamDelta`.
2. Replace request building with `build_request()`, serializing conversation
   history, tool definitions, and structured output schemas directly to
   `serde_json::Value`.
3. Replace `openai` crate streaming with `reqwest_eventsource::EventSource`.
   Have `handle_sse_event` process each SSE event and route reasoning, content,
   and tool call fields to provider-agnostic `Event` values.
4. Route `reasoning_content` directly when present, falling back to
   `ReasoningExtractor` when only `content` is available.
5. Remove all `openai` crate imports from `llamacpp.rs`.
6. Add unit tests for delta parsing across all three formats, request building
   (tool call merging and reasoning wrapping), and tool choice conversion.
7. Re-record VCR cassettes against a live llama.cpp server.

## References

- llama.cpp server documentation: `--reasoning-format` flag in
  `tools/server/README.md`.
- llama.cpp server source: `common_chat_msg_diff_to_json_oaicompat` in
  `common/chat.cpp`.
- Ollama provider: Reads `message.thinking` and `message.content` directly,
  using the same index convention (0/1/2+) and flush strategy as this design.
- [yoagent `openai_compat.rs`]: Reference implementation of OpenAI-compatible
  SSE streaming with `reqwest_eventsource`, from which the serde types and SSE
  loop structure are adapted.

[yoagent `openai_compat.rs`]: https://github.com/yologdev/yoagent/blob/main/src/provider/openai_compat.rs
