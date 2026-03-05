# RFD 012: Incremental Tool Call Argument Streaming

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-02-24

## Summary

This RFD proposes extending the `EventBuilder` to parse individual key/value
pairs from tool call argument JSON as they stream in, emitting incremental
events so that downstream consumers (e.g. the terminal renderer) can act on
individual arguments before the full tool call is complete.

## Motivation

When the LLM calls a tool like `fs_create_file`, the terminal currently shows
nothing until *all* arguments have been fully received and parsed. For tools
with large arguments — file content, code, diffs — this means several seconds of
silence between the "Calling tool X" header and the styled argument display.

Today's flow:

```
content_block_start(tool_use{id, name})
  → Event::Part with ToolCallRequest{id, name, arguments: {}}     ← UI shows "Calling tool X"
content_block_delta(input_json_delta: '{"path":')
  → swallowed by ToolCallRequestAggregator                        ← silence
content_block_delta(input_json_delta: '"src/main.rs","content":')
  → swallowed                                                     ← silence
  ... 8+ seconds of JSON chunks ...
content_block_stop
  → Event::Part with ToolCallRequest{id, name, arguments: {path, content}}
  → Event::Flush                                                  ← UI shows styled args
```

If we could emit the `path` argument as soon as its value is complete, the UI
could show "creating `src/main.rs`" within the first second, then display the
file content as it streams in (future work).

This also unlocks a future where string argument values can be streamed
character-by-character to the renderer — imagine seeing the file content appear
in real-time rather than all at once. That streaming extension is explicitly out
of scope for this RFD but this design is shaped to support it.

## Design

### User-facing behavior

No changes to the CLI interface or configuration. The observable difference is
that tool call arguments appear in the terminal incrementally:

1. Tool name appears immediately (already implemented).
2. Each argument key/value pair appears as soon as its value is fully parsed.
3. The final styled tool call display is identical to today's.

Callers that don't care about incremental updates continue to work unchanged —
they ignore the intermediate events and only act on the final flushed
`ToolCallRequest`.

### Event model

The `EventBuilder` currently emits `ToolCallRequest` events only on flush,
with all arguments populated at once. This RFD introduces a new event kind that
the `EventBuilder` can emit *during* accumulation, before the flush.

```rust
/// Progress of a tool call's argument parsing, emitted by EventBuilder
/// as JSON streams in.
pub enum ToolCallArgumentProgress {
    /// A complete key/value pair has been parsed from the arguments object.
    ///
    /// Emitted as soon as the parser detects the end of a value for a
    /// top-level key. For example, after receiving `{"path":"/tmp/foo.rs"`
    /// the builder emits ArgumentParsed for key="path", value="/tmp/foo.rs".
    ArgumentParsed {
        /// The tool call ID this argument belongs to.
        tool_call_id: String,
        /// The tool name.
        tool_name: String,
        /// The argument key.
        key: String,
        /// The fully parsed argument value.
        value: serde_json::Value,
    },

    /// The arguments object is not a JSON object (e.g. a string, array, or
    /// scalar). Incremental parsing is not possible. The caller should wait
    /// for the final ToolCallRequest on flush.
    NotAnObject {
        /// The tool call ID.
        tool_call_id: String,
    },
}
```

This is a *new event type emitted by the `EventBuilder`*, not a new
`EventKind` in `ConversationEvent`. These progress events are ephemeral —
they drive the UI but are not persisted to the conversation stream. The
persisted event remains the final `ToolCallRequest` with the complete
`arguments` map, emitted on flush as today.

### Where the parsing lives

The incremental JSON parsing happens inside `EventBuilder`, not in the provider
layer. The provider layer (`ToolCallRequestAggregator`) continues to buffer raw
JSON strings as it does today — that responsibility doesn't change.

The key insight is that `EventBuilder` already receives multi-part tool calls:

1. **First Part** (from `content_block_start`): `ToolCallRequest { id, name, arguments: {} }`
2. **Second Part** (from `content_block_stop`): `ToolCallRequest { id, name, arguments: {path: ..., content: ...} }`

Between these two Parts, the provider's `ToolCallRequestAggregator` buffers
the raw JSON chunks. We change the aggregator to forward those raw chunks to
the `EventBuilder` as well, allowing the `EventBuilder` to parse key/value
pairs incrementally.

This means we need a new channel from the provider layer to the `EventBuilder`
for partial JSON chunks. The simplest approach: emit them as a new variant
on `Event`:

```rust
// In jp_llm::event::Event
Event::ToolCallArgumentChunk {
    index: usize,
    chunk: String,
}
```

The `EventBuilder` receives these chunks, appends them to an internal buffer
per index, and attempts to parse complete key/value pairs from the front of
the buffer using a lightweight incremental JSON state machine.

### Incremental JSON parsing

The parser is a state machine that tracks:

- Whether we've seen the opening `{`
- The current nesting depth (to handle nested objects/arrays as values)
- Whether we're inside a string (to handle `{` and `}` inside string values)
- The position of the last successfully parsed key/value boundary

```
Buffer: {"path":"/tmp/foo.rs","content":"fn main() {}\n","dry_run":false}
         ^                    ^
         │                    └── After parsing: emit ArgumentParsed(path, "/tmp/foo.rs")
         └── Opening brace detected: object mode enabled

Buffer:                        "content":"fn main() {}\n","dry_run":false}
                                                          ^
                                                          └── emit ArgumentParsed(content, "fn main() {}\n")
```

The parser does NOT need a full JSON parser. It needs to:

1. Detect the opening `{` (if absent, emit `NotAnObject` and stop).
2. Track string boundaries (to ignore structural characters inside strings).
3. Track nesting depth (to handle `{...}` and `[...]` inside values).
4. Detect the `,` or `}` that ends a top-level value.
5. When a complete key/value pair is detected, extract it by parsing just
   that slice with `serde_json`.

This is deliberately simple — we parse only the top-level object structure.
Nested objects and arrays are emitted as complete `serde_json::Value`s once
their closing delimiter is found.

### EventBuilder changes

The `IndexBuffer::ToolCall` variant gains an incremental parser:

```rust
enum IndexBuffer {
    // ...existing variants...
    ToolCall {
        request: ToolCallRequest,
        /// Incremental argument parser, active when the arguments are
        /// an object. None if the tool call arrived as a single Part
        /// (no streaming) or if the arguments aren't an object.
        arg_parser: Option<IncrementalArgParser>,
    },
}
```

`EventBuilder::handle_tool_argument_chunk` is a new method that:

1. Appends the chunk to the parser's buffer.
2. Calls `parser.try_parse_next()` which returns `Vec<(String, Value)>` —
   zero or more complete key/value pairs.
3. For each parsed pair, adds it to `request.arguments` and returns a
   `ToolCallArgumentProgress::ArgumentParsed` event.

The existing `handle_flush` behavior is unchanged — it returns the final
`ToolCallRequest` with all arguments.

### Return type for handle_part / handle_tool_argument_chunk

Currently `handle_part` returns nothing — it just accumulates. To surface
incremental progress, the new `handle_tool_argument_chunk` method returns
`Vec<ToolCallArgumentProgress>`. The `TurnCoordinator` forwards these to the
`ToolRenderer`.

### TurnCoordinator changes

The `handle_streaming_event` method gains a new match arm for
`Event::ToolCallArgumentChunk`:

```rust
Event::ToolCallArgumentChunk { index, chunk } => {
    let progress = self.event_builder.handle_tool_argument_chunk(index, &chunk);
    for event in progress {
        // Forward to ToolRenderer for incremental display
    }
    Action::Continue
}
```

### Provider changes

Providers that stream tool call arguments (Anthropic, OpenRouter, Llamacpp)
emit `Event::ToolCallArgumentChunk` alongside their existing aggregation.
The `InputJsonDelta` handler in the Anthropic provider changes from:

```rust
types::ContentBlockDelta::InputJsonDelta { partial_json } => {
    agg.add_chunk(index, None, None, Some(&partial_json));
    return None;
}
```

to:

```rust
types::ContentBlockDelta::InputJsonDelta { partial_json } => {
    agg.add_chunk(index, None, None, Some(&partial_json));
    return Some(Event::ToolCallArgumentChunk {
        index,
        chunk: partial_json,
    });
}
```

The `ToolCallRequestAggregator` continues to do its job — it buffers and
parses the full arguments on finalize. The chunk events are an additional
signal for the `EventBuilder`.

### Backwards compatibility

- The final `ToolCallRequest` event on flush is identical to today's.
- `ToolCallArgumentProgress` events are ephemeral and not persisted.
- Callers that don't handle `Event::ToolCallArgumentChunk` can ignore it
  (it's a new variant they simply skip in their match).
- The `ConversationStream` is not affected.

## Drawbacks

- **Added complexity in EventBuilder.** The incremental JSON parser is new code
  that needs to be correct for all valid JSON argument shapes. Edge cases:
  escaped characters in strings, Unicode escapes, deeply nested values.

- **Duplicated buffering.** Both `ToolCallRequestAggregator` (in the provider)
  and `IncrementalArgParser` (in the `EventBuilder`) buffer the same JSON
  chunks. The aggregator buffers for final parsing; the parser buffers for
  incremental extraction. This is intentional — the aggregator is the source
  of truth and the parser is best-effort.

- **Argument order assumption.** The incremental parsing is most useful when
  short arguments (like `path`) come before long ones (like `content`). Most
  providers emit arguments in schema-definition order, but this is not
  guaranteed by any spec. If `content` comes first, we gain nothing — we still
  wait for the full content before emitting the path.

## Alternatives

### Parse in the provider layer instead of EventBuilder

Move the incremental parser into `ToolCallRequestAggregator` and have it emit
parsed arguments as they complete. Rejected because:

- The aggregator is a generic component used by multiple providers. Adding
  event emission to it couples it to the event system.
- Different providers may want different chunk granularity. Keeping parsing in
  `EventBuilder` makes it provider-agnostic.

### Use a streaming JSON parser crate

Use something like `serde_json::StreamDeserializer` or a SAX-style JSON parser
instead of a hand-written state machine. Worth investigating during
implementation. The hand-rolled approach is proposed because the parsing
requirement is narrow (top-level object keys only) and a full streaming parser
may be more complex to integrate. If a well-tested crate fits with minimal
API surface, it should be preferred.

### Emit intermediate ToolCallRequest events with partial arguments

Instead of a separate `ToolCallArgumentProgress` type, emit `ToolCallRequest`
events with partial `arguments` maps. Rejected because:

- Consumers that match on `ToolCallRequest` would need to distinguish partial
  from complete.
- Partial events would need careful handling in persistence to avoid writing
  incomplete tool calls.
- A distinct type makes the contract explicit: these are progress signals,
  not complete events.

## Non-Goals

- **Streaming string argument values.** This RFD does not address streaming the
  *contents* of a string value character-by-character (e.g. streaming the
  `content` field of `fs_create_file` to the renderer). That is a natural
  follow-up: once we can detect "the parser is now inside the value for key
  `content`", we can emit partial string chunks. The `IncrementalArgParser`
  is designed with this extension in mind but this RFD does not specify it.

- **Terminal renderer changes.** How the renderer uses
  `ToolCallArgumentProgress` events to update the display is out of scope.
  The renderer already has the infrastructure for incremental tool call display
  (`ToolRenderer::register`, `complete`). Wiring up the new events is
  straightforward follow-up work.

- **Reordering arguments.** We do not attempt to reorder arguments to ensure
  short ones arrive first. The LLM controls the order.

## Risks and Open Questions

- **JSON edge cases.** The incremental parser must handle: escaped quotes
  (`\"`), Unicode escapes (`\uXXXX`), nested objects and arrays, `null` /
  `true` / `false` / numbers as values, empty objects and arrays. A thorough
  test suite with property-based testing is warranted.

- **Provider argument ordering.** We assume providers tend to emit arguments in
  schema-definition order. If a provider reorders arguments (e.g. longest
  first), the incremental parsing adds overhead with no user-visible benefit.
  This should be validated empirically with Anthropic, OpenRouter, and
  Llamacpp.

- **Chunk boundaries.** JSON chunks from providers can split at arbitrary byte
  positions — including mid-string, mid-escape-sequence, or mid-number. The
  parser must handle partial data gracefully by buffering until a parse
  boundary is reached.

- **`Event` enum growth.** Adding `ToolCallArgumentChunk` to `Event` is a new
  variant. If we keep adding event types for incremental progress of different
  kinds, the enum grows. An alternative is a generic "progress" variant, but
  that trades type safety for fewer variants. For now, one new variant is
  acceptable.

## Implementation Plan

### Phase 1: Incremental JSON parser

Implement `IncrementalArgParser` as a standalone module in `jp_conversation`
with thorough unit tests. This is pure logic with no I/O dependencies.

- Input: `push(&mut self, chunk: &str)`
- Output: `Vec<(String, Value)>` of newly completed key/value pairs
- Edge case tests: escaped strings, nested structures, split boundaries
- Can be merged independently.

### Phase 2: Event plumbing

1. Add `Event::ToolCallArgumentChunk` to `jp_llm::event::Event`.
2. Add `ToolCallArgumentProgress` to `jp_conversation`.
3. Wire `EventBuilder` to use `IncrementalArgParser` when it receives
   tool call argument chunks.
4. Update `TurnCoordinator::handle_streaming_event` to handle the new
   event variant.
5. Update Anthropic, OpenRouter, and Llamacpp providers to emit
   `ToolCallArgumentChunk` events.
6. Can be merged independently (UI changes are a separate phase).

### Phase 3: Terminal renderer integration

Wire `ToolCallArgumentProgress` events into `ToolRenderer` to display
arguments incrementally. This is follow-up work and can be scoped in a
separate PR or RFD.

## References

- [Query Stream Pipeline Architecture](../architecture/query-stream-pipeline.md)
- `crates/jp_llm/src/stream/aggregator/tool_call_request.rs` — current
  `ToolCallRequestAggregator`
- `crates/jp_conversation/src/event_builder.rs` — current `EventBuilder`
- `crates/jp_cli/src/cmd/query/turn/coordinator.rs` — `TurnCoordinator`
- `crates/jp_cli/src/cmd/query/tool/renderer.rs` — `ToolRenderer`
