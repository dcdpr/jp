# RFD 043: Incremental Tool Call Argument Streaming

- **Status**: Discussion
- **Depends on**: [RFD 012 (Event Part Redesign)][RFD 012]
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-02-24

## Summary

This RFD proposes extending the `EventBuilder` to incrementally parse tool call
argument JSON as it streams in, emitting a recursive stream of typed fragments
so that downstream consumers (e.g. the terminal renderer) can act on argument
data as it arrives — including partial string values, individual array items,
and nested object entries — before the full tool call is complete.

## Motivation

When the LLM calls a tool like `fs_create_file`, the terminal currently shows
nothing until *all* arguments have been fully received and parsed. For tools
with large arguments — file content, code, diffs — this means several seconds of
silence between the "Calling tool X" header and the styled argument display.

Today's flow:

```txt
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
could show "creating `src/main.rs`" within the first second. And if the
`content` string were streamed as chunks, the file content could appear in the
terminal in real-time rather than all at once.

For tools like `fs_modify_file`, whose `patterns` argument is an array of
objects with potentially large `old` and `new` string fields, the same principle
applies recursively: each array item's fields can stream as they arrive, giving
the user immediate feedback on what's being changed.

## Design

### User-facing behavior

No changes to the CLI interface or configuration. The observable difference is
that tool call arguments appear in the terminal incrementally:

1. Tool name appears immediately (already implemented).
2. Short arguments (paths, flags) appear as soon as their value is parsed.
3. Long string arguments (file content, diffs) stream to the terminal in
   real-time as chunks arrive.
4. Compound arguments (arrays, nested objects) stream recursively — individual
   items and entries appear as they arrive, with their own values streaming in
   turn.

Callers that don't care about incremental updates continue to work unchanged —
they ignore the intermediate events and only act on the final flushed
`ToolCallRequest`.

### Event model

The `EventBuilder` currently emits `ToolCallRequest` events only on flush, with
all arguments populated at once. This RFD introduces a streaming fragment
protocol that the `EventBuilder` emits *during* accumulation, before the flush.

The protocol mirrors the existing `Event::Part` / `Event::Flush` pattern:
fragments stream in, a `Done` signal marks completion. Every value — whether a
scalar, a long string, or a deeply nested object — follows the same pipeline.
There is no separate "complete" vs "streaming" code path.

#### Fragment types

```rust
/// A non-string, non-compound JSON value.
///
/// Separated from `serde_json::Value` so that the type system enforces
/// that strings, arrays, and objects are always streamed through their
/// respective `StreamFragment` variants — never smuggled inside a
/// catch-all `Value`.
pub enum Scalar {
    Null,
    Bool(bool),
    Number(serde_json::Number),
}

/// A single fragment of an incrementally parsed JSON value.
///
/// The parser emits a sequence of fragments as JSON chunks arrive.
/// Every value path ends with `Done`. Recursive nesting encodes the
/// path from root to leaf — each fragment is self-describing.
pub enum StreamFragment {
    /// A scalar value (null, bool, or number).
    Scalar(Scalar),

    /// A chunk of string data.
    String(String),

    /// An item in a streaming array.
    ArrayItem {
        index: usize,
        value: Box<StreamFragment>,
    },

    /// A key-value pair in a streaming object.
    ObjectEntry {
        key: String,
        value: Box<StreamFragment>,
    },

    /// No more fragments for this value.
    Done,
}
```

The `EventBuilder` wraps each fragment with tool call identity:

```rust
/// Ephemeral progress event emitted by EventBuilder during tool call
/// argument streaming. Not persisted to the conversation stream.
pub struct ToolCallArgumentProgress {
    pub tool_call_id: String,
    pub tool_name: String,
    pub fragment: StreamFragment,
}
```

#### Protocol examples

For `fs_create_file` with `{"path": "/tmp/foo.rs", "content": "fn main() {...}"}`:

```txt
ObjectEntry { key: "path", value: String("/tmp/foo.rs") }
ObjectEntry { key: "path", value: Done }
ObjectEntry { key: "content", value: String("fn main(") }
ObjectEntry { key: "content", value: String(") {...}") }
ObjectEntry { key: "content", value: Done }
Done
```

The renderer sees `path` complete in the first events (after getting `Done` in
the second event) and can display "creating `/tmp/foo.rs`" immediately. The
`content` string streams to the terminal in real-time.

For `fs_modify_file` with `{"path": "lib.rs", "patterns": [{"old": "long...", "new": "also long..."}]}`:

```txt
ObjectEntry { key: "path", value: String("lib.rs") }
ObjectEntry { key: "path", value: Done }
ObjectEntry { key: "patterns", value: ArrayItem { index: 0, value:
    ObjectEntry { key: "old", value: String("lo") } } }
ObjectEntry { key: "patterns", value: ArrayItem { index: 0, value:
    ObjectEntry { key: "old", value: String("ng...") } } }
ObjectEntry { key: "patterns", value: ArrayItem { index: 0, value:
    ObjectEntry { key: "old", value: Done } } }
ObjectEntry { key: "patterns", value: ArrayItem { index: 0, value:
    ObjectEntry { key: "new", value: String("also ") } } }
ObjectEntry { key: "patterns", value: ArrayItem { index: 0, value:
    ObjectEntry { key: "new", value: String("long...") } } }
ObjectEntry { key: "patterns", value: ArrayItem { index: 0, value:
    ObjectEntry { key: "new", value: Done } } }
ObjectEntry { key: "patterns", value: ArrayItem { index: 0, value: Done } }
ObjectEntry { key: "patterns", value: Done }
Done
```

Each nesting level encodes the path from root to leaf. The renderer can stream
each pattern's `old` and `new` fields as they arrive.

For a scalar argument like `{"dry_run": true}`:

```txt
ObjectEntry { key: "dry_run", value: Scalar(Bool(true)) }
ObjectEntry { key: "dry_run", value: Done }
Done
```

Scalars are a single fragment followed by `Done`, same pipeline as everything
else.

If the root value is not an object (e.g. a bare string), the parser streams it
directly — `String("chunk")` fragments followed by `Done`. No special error case
needed; the protocol handles any JSON shape.

#### Aggregator layer

Consumers that don't care about streaming can use a `FragmentAggregator` that
collects fragments and emits complete `serde_json::Value`s:

```rust
struct FragmentAggregator { /* ... */ }

impl FragmentAggregator {
    /// Feed a fragment. Returns `Some(Value)` when a value is complete
    /// (i.e. `Done` was received).
    fn push(&mut self, fragment: StreamFragment) -> Option<serde_json::Value>;
}
```

The `EventBuilder` uses this internally to build the final
`ToolCallRequest.arguments` map, adding each argument as its `Done` arrives.
This replaces the `ToolCallRequestAggregator` that currently lives in the
provider layer.

### Where the parsing lives

The incremental JSON parsing happens inside `EventBuilder`, not in the provider
layer. This is a change from today's architecture, where the provider layer's
`ToolCallRequestAggregator` buffers raw JSON strings and parses them on
finalize.

With this RFD, argument parsing moves entirely to `EventBuilder` via the
`IncrementalArgParser` and `FragmentAggregator`. The `ToolCallRequestAggregator`
is removed. Providers already emit `ToolCall(ArgumentChunk(chunk))` events via
the `EventPart` redesign from [RFD 012]. The `EventBuilder` feeds these chunks
to the `IncrementalArgParser`, which emits `StreamFragment`s. The
`FragmentAggregator` collects these and populates `ToolCallRequest.arguments`
progressively. By flush time, the arguments map is complete.

### Incremental JSON parsing

The parser is a recursive descent state machine that emits `StreamFragment`
events as JSON chunks arrive. It maintains a stack tracking the current nesting
context:

- **Object context**: expecting a key or a value; tracks the current key.
- **Array context**: expecting an item; tracks the current item index.
- **String context**: accumulating string data; tracks escape sequence state.
- **Scalar context**: accumulating a number, boolean, or null literal.

On each `push(chunk)` call, the parser appends to its internal buffer and
advances the state machine, emitting fragments as structural boundaries are
detected.

For example, given `{"path":"/tmp/foo.rs","content":"fn main()
{}\n","dry_run":false}`, the parser emits:

```txt
ObjectEntry { key: "path", value: String("/tmp/foo.rs") }
ObjectEntry { key: "path", value: Done }
ObjectEntry { key: "content", value: String("fn main()") }   ┐
ObjectEntry { key: "content", value: String(" {}\n") }       ├ streamed as chunks arrive
ObjectEntry { key: "content", value: Done }                  ┘
ObjectEntry { key: "dry_run", value: Scalar(Bool(false)) }
ObjectEntry { key: "dry_run", value: Done }
Done
```

The parser handles:

1. **Strings**: Emitted as `String` chunks as data arrives. Escape sequences
   (`\"`, `\\`, `\uXXXX`) are decoded correctly. If a chunk boundary splits an
   escape sequence, the partial escape is buffered until the next chunk resolves
   it.
2. **Objects**: Each key-value pair is wrapped in `ObjectEntry`. The key is
   parsed first (always a complete string), then the value is parsed
   recursively.
3. **Arrays**: Each item is wrapped in `ArrayItem` with its index. Items are
   parsed recursively.
4. **Scalars**: Numbers, booleans, and null are emitted as `Scalar` once the
   literal is complete (detected by the next structural character or
   whitespace).
5. **Done signals**: Emitted at each level when the closing delimiter (`}`, `]`,
   or end of string/scalar) is reached.

### EventBuilder changes

The `IndexBuffer::ToolCall` variant gains an incremental parser and an optional
fragment aggregator:

```rust
enum IndexBuffer {
    // ...existing variants...
    ToolCall {
        request: ToolCallRequest,
        /// Incremental argument parser. None if the tool call arrived
        /// as a single Part (no streaming).
        arg_parser: Option<IncrementalArgParser>,
        /// Aggregator that collects fragments into complete values,
        /// populating `request.arguments` progressively.
        aggregator: FragmentAggregator,
    },
}
```

When the `EventBuilder` receives a `ToolCall(ArgumentChunk(chunk))`, it:

1. Feeds the chunk to the `IncrementalArgParser`.
2. The parser returns `Vec<StreamFragment>` — zero or more fragments.
3. Each fragment is fed to the `FragmentAggregator`, which adds completed
   arguments to `request.arguments` as their `Done` arrives.
4. Each fragment is wrapped in `ToolCallArgumentProgress` (with tool call id and
   name) and returned to the caller.

The existing `handle_flush` behavior is unchanged — it returns the final
`ToolCallRequest` with all arguments. The aggregator ensures arguments parsed
incrementally are already present in the request by flush time.

### Return type for handle_part

`EventBuilder::handle_part` currently returns nothing — it just accumulates.
With this RFD, it returns `Vec<ToolCallArgumentProgress>` (empty for
non-tool-call parts). The `TurnCoordinator` forwards these to the
`ToolRenderer`.

### TurnCoordinator changes

The `TurnCoordinator` already matches on `EventPart::ToolCall` variants (per [RFD
012]). The change is that `handle_part` now returns `ToolCallArgumentProgress`
events for `ArgumentChunk` parts:

```rust
EventPart::ToolCall(ToolCallPart::ArgumentChunk(_)) => {
    let progress = self.event_builder.handle_part(index, part, metadata);
    for event in progress {
        // Forward to ToolRenderer for incremental display
    }
}
```

### Provider changes

Providers already emit `ToolCall(ArgumentChunk(chunk))` per [RFD 012].
The only change is that the `ToolCallRequestAggregator` is removed from
the `EventBuilder` and replaced by the `IncrementalArgParser` +
`FragmentAggregator` pipeline.

### Backwards compatibility

- The final `ToolCallRequest` event on flush is identical to today's.
- `ToolCallArgumentProgress` events are ephemeral and not persisted.
- The `ConversationStream` is not affected.
- No new `Event` variants are added — this RFD builds on the `EventPart` types
  introduced by [RFD 012].

## Drawbacks

- **Recursive parser complexity.** The incremental JSON parser is a recursive
  descent state machine that must handle arbitrary nesting depths correctly.
  Edge cases include escaped characters in strings at any depth, Unicode
  escapes (`\uXXXX`), escape sequences split across chunk boundaries, and
  deeply nested structures. This is substantially more complex than a
  top-level-only parser.

- **Box allocations per fragment.** Each nesting level in a `StreamFragment`
  requires a `Box` allocation. For `patterns[0].old` string chunks, that's
  three Boxes per chunk. In a CLI streaming terminal output where chunks are
  tens to hundreds of bytes, this is negligible — but it is a per-chunk cost
  proportional to nesting depth.

- **Argument order assumption.** The incremental parsing is most useful when
  short arguments (like `path`) come before long ones (like `content`). Most
  providers emit arguments in schema-definition order, but this is not
  guaranteed by any spec. If `content` comes first, the user still sees it
  streamed in real-time, but the `path` won't appear until after the full
  content has been received.

## Alternatives

### Keep ToolCallRequestAggregator alongside the incremental parser

Keep the existing `ToolCallRequestAggregator` in the provider layer as the
authoritative source for the final `ToolCallRequest`, and treat the
incremental parser as a best-effort side channel for UI progress. This
provides defense in depth — if the incremental parser has a bug, the
aggregator still produces correct arguments on flush.

Rejected because:

- It means buffering every JSON chunk twice (once in the aggregator, once
  in the parser) for no functional benefit.
- The incremental parser + `FragmentAggregator` already produce the
  complete arguments by flush time. A second independent parse path adds
  complexity without improving correctness — if the parser is buggy, the
  aggregator would silently mask the bug rather than surfacing it.
- Removing the aggregator simplifies the provider layer: providers just
  forward chunks and let `EventBuilder` handle parsing.

### Use a streaming JSON parser crate

Use something like `serde_json::StreamDeserializer` or a SAX-style JSON parser
instead of a hand-written state machine. Worth investigating during
implementation. A SAX-style parser would provide the event stream we need
(start-object, key, start-string, string-data, end-string, etc.) and the
`IncrementalArgParser` would translate SAX events into `StreamFragment`s.
If a well-tested crate fits with minimal API surface, it should be preferred
over a hand-rolled parser — especially given the recursive parsing requirement.

### Emit intermediate ToolCallRequest events with partial arguments

Instead of a separate `ToolCallArgumentProgress` type, emit `ToolCallRequest`
events with partial `arguments` maps. Rejected because:

- Consumers that match on `ToolCallRequest` would need to distinguish partial
  from complete.
- Partial events would need careful handling in persistence to avoid writing
  incomplete tool calls.
- A distinct type makes the contract explicit: these are progress signals,
  not complete events.

### Flat (non-recursive) fragment model

Stream only top-level argument key-value pairs, emitting nested structures
(arrays, objects) as complete `serde_json::Value`s. Simpler parser, but loses
the ability to stream inside `fs_modify_file`'s `patterns` array or any other
nested structure with large string values. The recursive model adds Box
allocations per nesting level per fragment, but this cost is negligible for
a CLI tool and the streaming benefit for nested arguments is real.

### Carry complete value in terminal Done fragment

Instead of a data-free `Done`, have `Done(serde_json::Value)` carrying the
final assembled value. Rejected because:

- The value is redundant — the consumer either aggregated from chunks already,
  or uses the final `ToolCallRequest` from the flush path.
- For large string values, this means allocating the full value a second time
  just to attach it to a signal nobody reads.
- The `FragmentAggregator` provides this convenience for consumers that need
  complete values without duplicating data in the streaming protocol.

## Non-Goals

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
  `true` / `false` / numbers as values, empty objects and arrays. The
  recursive parser must handle these at every nesting depth. A thorough
  test suite with property-based testing is warranted.

- **Escape sequences at chunk boundaries.** A chunk can split mid-escape:
  `\` at the end of one chunk, `"` at the start of the next. Or `\u00`
  in one chunk, `41` in the next. The parser needs a small state machine
  (4-5 states) to handle partial escapes at every string nesting level.
  This is the trickiest part of the parser implementation.

- **Provider argument ordering.** We assume providers tend to emit arguments in
  schema-definition order. If a provider reorders arguments (e.g. longest
  first), the incremental parsing adds overhead with no user-visible benefit.
  This should be validated empirically with Anthropic, OpenRouter, and
  Llamacpp.

- **Chunk boundaries.** JSON chunks from providers can split at arbitrary byte
  positions — including mid-string, mid-escape-sequence, or mid-number. The
  parser must handle partial data gracefully by buffering until a parse
  boundary is reached.

## Implementation Plan

### Phase 1: StreamFragment types and FragmentAggregator

Define the `Scalar`, `StreamFragment`, and `ToolCallArgumentProgress` types
in `jp_conversation`. Implement `FragmentAggregator` with thorough unit
tests. These are pure data types with no I/O dependencies.

- Test aggregation: scalars, strings, arrays, nested objects.
- Test that `Done` at each level produces the correct `serde_json::Value`.
- Can be merged independently.

### Phase 2: Incremental JSON parser

Implement `IncrementalArgParser` as a standalone module in `jp_conversation`
with thorough unit tests. This is pure logic with no I/O dependencies.

- Input: `push(&mut self, chunk: &str)`
- Output: `Vec<StreamFragment>`
- Recursive parsing of objects, arrays, strings, and scalars.
- Edge case tests: escaped strings at every depth, split escape sequences,
  nested structures, empty containers.
- Property-based tests: generate random JSON, chunk it at random boundaries,
  verify the fragment stream reassembles to the original value via
  `FragmentAggregator`.
- Can be merged independently.

### Phase 3: Event plumbing

1. Replace the `ToolCallRequestAggregator` in `EventBuilder` with
   `IncrementalArgParser` + `FragmentAggregator` for
   `ToolCall(ArgumentChunk(...))` handling.
2. Update `EventBuilder::handle_part` to return
   `Vec<ToolCallArgumentProgress>` for tool call argument chunks.
3. Update `TurnCoordinator` to forward progress events to the
   `ToolRenderer`.
4. Can be merged independently (UI changes are a separate phase).

### Phase 4: Terminal renderer integration

Wire `ToolCallArgumentProgress` events into `ToolRenderer` to display
arguments incrementally. This is follow-up work and can be scoped in a
separate PR or RFD.

## References

- [RFD 012 (Event Part Redesign)][RFD 012] — prerequisite; introduces
  `EventPart` and `ToolCallPart`
- [Query Stream Pipeline Architecture](../architecture/query-stream-pipeline.md)
- `crates/jp_llm/src/stream/aggregator/tool_call_request.rs` —
  `ToolCallRequestAggregator` (removed by this RFD)
- `crates/jp_conversation/src/event_builder.rs` — current `EventBuilder`
- `crates/jp_cli/src/cmd/query/turn/coordinator.rs` — `TurnCoordinator`
- `crates/jp_cli/src/cmd/query/tool/renderer.rs` — `ToolRenderer`

[RFD 012]: 012-typed-llm-streaming-events.md
