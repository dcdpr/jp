# RFD 043: Incremental Tool Call Argument Streaming

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-02-24
- **Summary**: Extends `EventBuilder` to incrementally parse streaming tool call
  JSON and emit typed argument fragments before completion.

## Summary

This RFD extends `EventBuilder` to parse tool call argument JSON incrementally
as it streams from the provider.
`EventBuilder` emits a recursive stream of typed fragments (including partial
strings, array items, and nested object entries) so downstream consumers like
the terminal renderer can display arguments before the full tool call completes.

## Motivation

When the LLM calls a tool like `fs_create_file`, the terminal currently shows
nothing until all arguments have been received and parsed.
For tools with large arguments (file content, code, diffs), this causes several
seconds of silence between the "Calling tool X" header and the styled argument
display.

In the current flow, JSON chunks accumulate in
`IndexBuffer::ToolCall.arguments_json` and are parsed only when the content
block finishes:

```txt
content_block_start(tool_use{id, name})
  → Event::Part with ToolCallRequest{id, name, arguments: {}}     ← UI shows "Calling tool X"
content_block_delta(input_json_delta: '{"path":')
  → appended to IndexBuffer::ToolCall.arguments_json             ← silence
content_block_delta(input_json_delta: '"src/main.rs","content":')
  → swallowed                                                     ← silence
  ... 8+ seconds of JSON chunks ...
content_block_stop
  → Event::Part with ToolCallRequest{id, name, arguments: {path, content}}
  → Event::Flush                                                  ← UI shows styled args
```

Emitting the `path` argument as soon as its value is complete lets the UI show
"creating `src/main.rs`" within the first second.
Streaming the `content` string in chunks lets the file content appear in the
terminal in real time rather than all at once.

The same principle applies recursively to nested arguments.
For example, `fs_modify_file` takes a `patterns` array of objects with large
`old` and `new` string fields; streaming each item's fields as they arrive shows
the user what is changing right away.

## Design

### User-facing behavior

This RFD does not change the CLI interface or configuration.
Tool call arguments appear in the terminal incrementally:

1. The tool name appears immediately (already implemented).
2. Short arguments (paths, flags) appear as soon as their value is parsed.
3. Long string arguments (file content, diffs) stream to the terminal in real
   time as chunks arrive.
4. Compound arguments (arrays, nested objects) stream recursively: individual
   items and entries appear as they arrive, and their values stream in turn.

Callers that do not use incremental updates ignore the intermediate events and
act only on the final flushed `ToolCallRequest`.

### Event model

`EventBuilder` currently emits `ToolCallRequest` events only on flush, with all
arguments populated at once.
This RFD introduces a streaming fragment protocol that `EventBuilder` emits
during accumulation, before the flush.

The protocol mirrors the `Event::Part` and `Event::Flush` pattern: fragments
stream in and a `Done` signal marks completion.
Every value (scalar, long string, or nested object) uses the same pipeline,
without a separate code path for complete versus streaming values.

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

`EventBuilder` wraps each fragment with the tool call's ID and name:

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

For `fs_create_file` with `{"path": "/tmp/foo.rs", "content": "fn main()
{...}"}`:

```txt
ObjectEntry { key: "path", value: String("/tmp/foo.rs") }
ObjectEntry { key: "path", value: Done }
ObjectEntry { key: "content", value: String("fn main(") }
ObjectEntry { key: "content", value: String(") {...}") }
ObjectEntry { key: "content", value: Done }
Done
```

Once the second event delivers `Done` for `path`, the renderer knows `path` is
complete and can display "creating `/tmp/foo.rs`" immediately, while `content`
streams to the terminal in real time.

For `fs_modify_file` with `{"path": "lib.rs", "patterns": [{"old": "long...",
"new": "also long..."}]}`:

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

Each nesting level encodes the path from root to leaf, so the renderer can
stream each pattern's `old` and `new` fields as they arrive.

For a scalar argument like `{"dry_run": true}`:

```txt
ObjectEntry { key: "dry_run", value: Scalar(Bool(true)) }
ObjectEntry { key: "dry_run", value: Done }
Done
```

A scalar emits a single `Scalar` fragment followed by `Done`.

If the root value is not an object (for example, a bare string), the parser
streams it directly as `String("chunk")` fragments followed by `Done`.
The protocol handles any JSON shape without a special error case.

#### Aggregator layer

`FragmentAggregator` collects streaming fragments and emits complete
`serde_json::Value`s for consumers that need assembled values:

```rust
struct FragmentAggregator { /* ... */ }

impl FragmentAggregator {
    /// Feed a fragment. Returns `Some(Value)` when a value is complete
    /// (i.e. `Done` was received).
    fn push(&mut self, fragment: StreamFragment) -> Option<serde_json::Value>;
}
```

`EventBuilder` uses `FragmentAggregator` internally to populate
`ToolCallRequest.arguments` as each argument's `Done` fragment arrives,
replacing the `arguments_json: String` buffer on `IndexBuffer::ToolCall`.

### Incremental JSON parser

`IncrementalArgParser` is a recursive-descent state machine that emits
`StreamFragment` events as JSON chunks arrive.
It maintains a stack of active nesting contexts:

- **Object context**: expects a key or a value and tracks the current key.
- **Array context**: expects an item and tracks the current item index.
- **String context**: accumulates string data and tracks escape-sequence state.
- **Scalar context**: accumulates a number, boolean, or `null` literal.

On each `push(chunk)` call, the parser appends to its internal buffer and
advances the state machine, emitting fragments as it crosses structural
boundaries.

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

The parser handles each JSON construct as follows:

1. **Strings**: Emits `String` chunks as data arrives and decodes escape
   sequences (`\"`, `\\`, `\uXXXX`).
   If a chunk boundary splits an escape sequence, the parser buffers the partial
   escape until the next chunk completes it.
2. **Objects**: Parses each key as a complete string, then recursively parses
   the value and wraps each emitted fragment in `ObjectEntry`.
3. **Arrays**: Recursively parses each item and wraps each emitted fragment in
   `ArrayItem` with the item's index.
4. **Scalars**: Buffers number, boolean, and `null` literals and emits `Scalar`
   once the next structural character or whitespace marks the end of the
   literal.
5. **`Done` signals**: Emits `Done` at the current nesting level when its
   closing delimiter (`}`, `]`, closing quote, or end of scalar) is reached.

### `EventBuilder` and `TurnCoordinator` integration

Providers already emit `ToolCall(ArgumentChunk(chunk))` events via the
`EventPart` types from [RFD 012], so no provider changes are required.
Incremental parsing lives inside `EventBuilder`, where argument accumulation
already happens.

`IndexBuffer::ToolCall` replaces its `arguments_json: String` buffer (which
buffered every `ArgumentChunk` and parsed once on flush) with an optional
`IncrementalArgParser` and a `FragmentAggregator`:

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

`EventBuilder::handle_part` returns `Vec<ToolCallArgumentProgress>` (empty for
non-tool-call parts).
When `EventBuilder::handle_part` receives `ToolCall(ArgumentChunk(chunk))`:

1. It feeds `chunk` to `IncrementalArgParser`, which returns zero or more
   `StreamFragment`s.
2. It feeds each fragment to `FragmentAggregator`, which inserts completed
   arguments into `request.arguments` as their `Done` fragment arrives.
3. It wraps each fragment in `ToolCallArgumentProgress` (with `tool_call_id` and
   `tool_name`) and returns the progress events to the caller.

`EventBuilder::handle_flush` is unchanged: it returns the final
`ToolCallRequest`, whose `arguments` map is already populated by
`FragmentAggregator`.

`TurnCoordinator` already matches on `EventPart::ToolCall` variants (per [RFD
012]) and forwards the progress events returned for `ArgumentChunk` parts to
`ToolRenderer`:

```rust
EventPart::ToolCall(ToolCallPart::ArgumentChunk(_)) => {
    let progress = self.event_builder.handle_part(index, part, metadata);
    for event in progress {
        // Forward to ToolRenderer for incremental display
    }
}
```

### Backwards compatibility

- The final `ToolCallRequest` event emitted on flush is unchanged.
- `ToolCallArgumentProgress` events are ephemeral and are not persisted to
  `ConversationStream`.
- No new `Event` variants are added; this RFD builds on the `EventPart` types
  introduced by [RFD 012].

## Drawbacks

- **Recursive parser complexity.** Supporting arbitrary nesting depth in a
  recursive-descent state machine is substantially more complex than parsing
  only top-level keys.
  The parser must handle escaped characters, Unicode escapes (`\uXXXX`), and
  escape sequences split across chunk boundaries at every nesting level.

- **Box allocations per fragment.** Each nesting level in a `StreamFragment`
  allocates a `Box`.
  A string chunk inside `patterns[0].old` requires three `Box` allocations.
  This per-chunk cost is proportional to nesting depth, though negligible for
  terminal streaming where chunks are tens to hundreds of bytes.

- **Argument order assumption.** Incremental parsing is most useful when short
  identifying arguments (like `path`) precede long ones (like `content`).
  Most providers emit arguments in schema-definition order, but no specification
  guarantees this.
  If `content` arrives first, it still streams in real time, but `path` does not
  appear until `content` finishes.

## Alternatives

### Keep the `arguments_json` buffer alongside the incremental parser

Keep the buffer-and-parse-on-flush path as the authoritative source for the
final `ToolCallRequest`, and treat the incremental parser as a best-effort side
channel for UI progress.
If the incremental parser had a bug, the flush-time parse would still produce
valid arguments.

Rejected because:

- Every JSON chunk would be buffered twice (once in the string buffer and once
  in the parser) for no functional benefit.
- `IncrementalArgParser` and `FragmentAggregator` already assemble the complete
  arguments map by flush time.
  A fallback parse path would silently mask bugs in the incremental parser
  rather than surfacing them.
- Maintaining a single accumulation path keeps `IndexBuffer::ToolCall` from
  holding two representations of the arguments that can disagree.

### Use a streaming JSON parser crate

Use an existing crate such as `serde_json::StreamDeserializer` or a SAX-style
JSON parser instead of a hand-written state machine.
A SAX-style parser could emit low-level events (start-object, key, start-string,
string-data, end-string) that `IncrementalArgParser` translates into
`StreamFragment`s.

This option is worth evaluating during implementation.
If a well-tested crate supports arbitrary chunk boundaries with a minimal API
surface, prefer it over a hand-rolled recursive parser.

### Emit intermediate `ToolCallRequest` events with partial arguments

Emit `ToolCallRequest` events with partial `arguments` maps instead of
introducing `ToolCallArgumentProgress`.

Rejected because:

- Consumers matching on `ToolCallRequest` would have to distinguish partial
  requests from completed ones.
- Persistence code would have to filter out partial events to avoid writing
  incomplete tool calls.
- A distinct type makes the contract explicit: `ToolCallArgumentProgress` values
  are ephemeral progress signals, not complete requests.

### Flat fragment model

Stream only top-level argument key-value pairs and emit nested structures
(arrays, objects) as complete `serde_json::Value`s.
This simplifies the parser, but cannot stream large strings inside nested
structures such as `fs_modify_file`'s `patterns` array.
The recursive model adds one `Box` allocation per nesting level per fragment,
which is negligible for a CLI tool.

### Carry the complete value in `Done`

Attach the assembled value to the terminal variant as `Done(serde_json::Value)`
instead of using a unit `Done` variant.

Rejected because:

- The value is redundant: consumers either aggregate chunks as they arrive or
  use the final `ToolCallRequest` from the flush path.
- For large string values, building `Done(Value)` allocates a full copy of the
  string just to attach it to a completion signal nobody reads.
- `FragmentAggregator` already provides assembled `serde_json::Value`s for
  consumers that need them without duplicating data in the streaming protocol.

## Non-Goals

- **Terminal renderer changes.** How `ToolRenderer` renders
  `ToolCallArgumentProgress` events is out of scope.
  `ToolRenderer` already supports incremental tool call display
  (`ToolRenderer::register`, `complete`), and wiring up the new events is
  follow-up work.

- **Reordering arguments.** `EventBuilder` emits arguments in the order the LLM
  streams them and does not reorder short arguments ahead of long ones.

## Risks and Open Questions

- **JSON grammar coverage at arbitrary depth.** The recursive parser must handle
  escaped quotes (`\"`), Unicode escapes (`\uXXXX`), `null`, `true`, `false`,
  numbers, and empty or nested objects and arrays at every nesting depth.
  A unit and property-based test suite should verify parity with `serde_json`.

- **Arbitrary chunk boundaries.** JSON chunks from providers can split at any
  byte or token boundary, including mid-string, mid-number, or mid-escape
  sequence (for example, `\` at the end of one chunk and `"` at the start of the
  next, or `\u00` followed by `41`).
  The parser must buffer incomplete tokens until a parse boundary is reached and
  maintain a 4-to-5-state escape sub-machine at every string nesting level.

- **Provider argument ordering.** Incremental display assumes providers tend to
  emit arguments in schema-definition order.
  If a provider emits large arguments first, incremental parsing adds overhead
  without improving time-to-first-argument.
  This assumption should be validated empirically with Anthropic, OpenRouter,
  and Llamacpp.

## Implementation Plan

Each phase can be merged independently.

### Phase 1: `StreamFragment` types and `FragmentAggregator`

Define `Scalar`, `StreamFragment`, and `ToolCallArgumentProgress` in `jp_llm`
alongside `EventPart` and `ToolCallPart`, and implement `FragmentAggregator`.
These are pure data types with no I/O dependencies.

- Unit-test aggregation of scalars, strings, arrays, and nested objects.
- Verify that `Done` at each level produces the expected `serde_json::Value`.

### Phase 2: Incremental JSON parser

Implement `IncrementalArgParser` (`push(&mut self, chunk: &str) ->
Vec<StreamFragment>`) as a standalone module in `jp_llm` with no I/O
dependencies.

- Parse objects, arrays, strings, and scalars recursively.
- Unit-test escaped strings at every depth, split escape sequences, nested
  structures, and empty containers.
- Add property-based tests that split random JSON values at random chunk
  boundaries and verify that `FragmentAggregator` reconstructs the original
  value.

### Phase 3: Event plumbing

1. Replace `IndexBuffer::ToolCall`'s `arguments_json` buffer with
   `IncrementalArgParser` and `FragmentAggregator` for
   `ToolCall(ArgumentChunk(...))` handling.
2. Update `EventBuilder::handle_part` to return `Vec<ToolCallArgumentProgress>`
   for tool call argument chunks.
3. Update `TurnCoordinator` to forward progress events to `ToolRenderer`.

### Phase 4: Terminal renderer integration

Wire `ToolCallArgumentProgress` events into `ToolRenderer` to display arguments
incrementally.
This follow-up work can be scoped in a separate PR or RFD.

## References

- [RFD 012 (Event Part Redesign)][RFD 012]: prerequisite that introduces
  `EventPart` and `ToolCallPart`.
- [Query Stream Pipeline Architecture]
- `crates/jp_llm/src/event_builder.rs`: `EventBuilder` and
  `IndexBuffer::ToolCall`, whose `arguments_json` buffer-and-parse-on-flush path
  this RFD replaces.
- `crates/jp_cli/src/cmd/query/turn/coordinator.rs`: `TurnCoordinator`.
- `crates/jp_cli/src/render/tool.rs`: `ToolRenderer`.

[Query Stream Pipeline Architecture]: ../wiki/00dxyb6-query-stream-pipeline-architecture.md
[RFD 012]: 012-typed-llm-streaming-events.md
