# RFD 012: Typed LLM Streaming Events

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-15

## Summary

This RFD replaces `ConversationEvent` inside `jp_llm::Event::Part` with a
purpose-built `EventPart` enum that models what LLM providers actually stream:
message chunks, reasoning chunks, structured data chunks, and tool call
lifecycle events. This decouples the streaming transport layer from the
persistence layer, eliminates invalid states, and prepares the infrastructure
for incremental tool call argument streaming.

## Motivation

`Event::Part` currently carries a `ConversationEvent`:

```rust
pub enum Event {
    Part { index: usize, event: ConversationEvent },
    Flush { index: usize, metadata: IndexMap<String, Value> },
    Finished(FinishReason),
}
```

`ConversationEvent` is a persistence type. It carries a timestamp, a metadata
map, and an `EventKind` that can be any of seven variants: `TurnStart`,
`ChatRequest`, `ChatResponse`, `ToolCallRequest`, `ToolCallResponse`,
`InquiryRequest`, `InquiryResponse`.

In the streaming context, only a subset is valid:

| EventKind variant    | Used in `Event::Part`? |
|----------------------|------------------------|
| `ChatResponse::Message`    | Yes — message content chunks |
| `ChatResponse::Reasoning`  | Yes — reasoning content chunks |
| `ChatResponse::Structured` | Yes — but hacked as `Value::String(chunk)` |
| `ToolCallRequest`          | Yes — but only as a start signal with empty arguments |
| `TurnStart`                | No |
| `ChatRequest`              | No |
| `ToolCallResponse`         | No |
| `InquiryRequest`           | No |
| `InquiryResponse`          | No |

This creates several problems:

1. **Invalid states are representable.** The `EventBuilder` must silently ignore
   `ChatRequest`, `ToolCallResponse`, `InquiryRequest`, `InquiryResponse`, and
   `TurnStart` in its `handle_part` method. These should be unrepresentable at
   the type level.

2. **Structured data is a hack.** Structured response chunks piggy-back on
   `ChatResponse::Structured { data: Value::String(chunk) }` — a persistence
   type abused as transport. This has caused bugs where consumers process
   structured data chunks as regular message content.

3. **Tool call starts are overloaded.** When a tool call begins, the provider
   emits a `ToolCallRequest` with empty arguments. This is a persistence type
   pretending to be a "streaming has started" signal.

4. **Wasted allocations.** Every streaming chunk allocates a `ConversationEvent`
   with a timestamp and metadata map that nobody reads during streaming.

5. **Growing `ConversationEvent`.** As the persistence type grows (new event
   kinds, richer metadata), every streaming chunk pays the cost even though it
   only uses a small fraction of the type.

## Design

### New Event structure

```rust
pub enum Event {
    /// Streaming data for a given index, accumulated until Flush.
    Part {
        index: usize,
        part: EventPart,
        /// Metadata accumulated during streaming (e.g. thinking signatures).
        /// Usually empty.
        metadata: Map<String, Value>,
    },

    /// Flush accumulated Parts for this index into a ConversationEvent.
    Flush {
        index: usize,
        metadata: IndexMap<String, Value>,
    },

    /// The response stream is finished.
    Finished(FinishReason),
}
```

### EventPart

```rust
/// A chunk of streaming data from an LLM provider.
///
/// Each variant maps to a distinct content type that providers
/// differentiate between. The EventBuilder accumulates these into
/// ConversationEvents on Flush.
pub enum EventPart {
    /// A chunk of assistant message content.
    Message(String),

    /// A chunk of reasoning/thinking content.
    Reasoning(String),

    /// A chunk of structured response JSON.
    Structured(String),

    /// Tool call streaming data.
    ToolCall(ToolCallPart),
}
```

### ToolCallPart

```rust
/// Streaming events for a single tool call.
pub enum ToolCallPart {
    /// Tool call identity. First non-empty value wins per field when
    /// multiple Start events arrive for the same index.
    Start {
        id: String,
        name: String,
    },

    /// A raw JSON chunk of tool call arguments.
    ArgumentChunk(String),
}
```

### EventBuilder translation

The `EventBuilder` becomes an explicit translator between the streaming domain
(`EventPart`) and the persistence domain (`ConversationEvent`):

| EventPart received | Buffer action | ConversationEvent on Flush |
|---------------------|---------------|----------------------------|
| `Message(s)` | Append to string buffer | `ChatResponse::Message { message }` |
| `Reasoning(s)` | Append to string buffer | `ChatResponse::Reasoning { reasoning }` |
| `Structured(s)` | Append to JSON string buffer | Parse JSON → `ChatResponse::Structured { data }` |
| `ToolCall(Start { id, name })` | Initialize tool call buffer | — |
| `ToolCall(ArgumentChunk(s))` | Feed `ToolCallRequestAggregator` | `ToolCallRequest { id, name, arguments }` |

The `ToolCallRequestAggregator` remains unchanged — it buffers raw JSON argument
chunks and parses them on finalize, as today. This RFD only changes the
transport; it does not change how arguments are parsed.

### Metadata handling

Streaming metadata (e.g. Anthropic's thinking signatures via `SignatureDelta`)
currently piggy-backs on `ConversationEvent`'s metadata map. With `EventPart`,
metadata moves to a field on `Event::Part`:

```rust
Event::Part {
    index: 0,
    part: EventPart::Reasoning("thinking...".into()),
    metadata: Map::new(), // usually empty; populated for signature deltas
}
```

The `EventBuilder` accumulates Part-level metadata the same way it does today —
merging it into the final `ConversationEvent`'s metadata on Flush.

### Provider changes

Each provider's stream mapper changes from constructing `ConversationEvent`s to
constructing `EventPart`s. This is a mechanical transformation:

**Before (Anthropic example):**

```rust
// Message content
Event::Part {
    index,
    event: ConversationEvent::now(ChatResponse::Message {
        message: text.clone(),
    }),
}

// Tool call start
Event::Part {
    index,
    event: ConversationEvent::now(ToolCallRequest {
        id: id.clone(),
        name: name.clone(),
        arguments: Map::new(),
    }),
}

// Structured data chunk
Event::Part {
    index,
    event: ConversationEvent::now(ChatResponse::Structured {
        data: Value::String(chunk),
    }),
}
```

**After:**

```rust
// Message content
Event::Part {
    index,
    part: EventPart::Message(text.clone()),
    metadata: Map::new(),
}

// Tool call start
Event::Part {
    index,
    part: EventPart::ToolCall(ToolCallPart::Start {
        id: id.clone(),
        name: name.clone(),
    }),
    metadata: Map::new(),
}

// Structured data chunk
Event::Part {
    index,
    part: EventPart::Structured(chunk),
    metadata: Map::new(),
}
```

### TurnCoordinator changes

The `TurnCoordinator` currently inspects `ConversationEvent` variants inside
`Event::Part` to decide what to render. With `EventPart`, it matches directly on
the streaming types:

```rust
Event::Part { index, part, metadata } => {
    match &part {
        EventPart::Message(text) => {
            self.chat_renderer.render_message(text);
        }
        EventPart::Reasoning(text) => {
            self.chat_renderer.render_reasoning(text);
        }
        EventPart::Structured(chunk) => {
            self.structured_renderer.render_chunk(chunk);
        }
        EventPart::ToolCall(ToolCallPart::Start { id, name }) => {
            self.chat_renderer.flush();
            // register tool call for preparing display
        }
        EventPart::ToolCall(ToolCallPart::ArgumentChunk(_)) => {
            // forwarded to EventBuilder only; no rendering yet
        }
    }
    self.event_builder.handle_part(index, part, metadata);
}
```

### Backwards compatibility

- `ConversationEvent` is unchanged — it remains the persistence type used by
  `ConversationStream`.
- The `ConversationStream` is not affected.
- `Event::Flush` and `Event::Finished` are unchanged.
- The `Event::Part` variant changes its inner type. All consumers of
  `Event::Part` must be updated (providers, EventBuilder, TurnCoordinator). This
  is a breaking change within `jp_llm`.

### What does NOT change

- `ConversationEvent` and `EventKind` (persistence types)
- `ConversationStream` (persistence layer)
- `ToolCallRequestAggregator` (argument parsing — see [RFD 048] for its
  replacement)
- The query stream pipeline's Part > Flush > Finished lifecycle

## Drawbacks

- **Breaking change to `Event`.** Every provider and every consumer of
  `Event::Part` must be updated. This is a mechanical transformation but touches
  many files.

- **Two type hierarchies for the same concepts.** `EventPart::Message` and
  `ChatResponse::Message` represent the same concept (assistant message content)
  at different layers. This is intentional — streaming and persistence have
  different concerns — but it means the EventBuilder translates between parallel
  hierarchies.

## Alternatives

### Keep ConversationEvent in Event::Part, add ValueChunk variant

Add `Event::ValueChunk { index, chunk }` alongside the existing `Event::Part`
for raw JSON chunks (tool call arguments, structured responses). This is the
minimal change needed for incremental argument streaming.

Rejected because:

- `Event::Part` continues to carry invalid variants (`ChatRequest`, `TurnStart`,
  etc.) that the EventBuilder silently ignores.
- Structured data still piggy-backs on `ChatResponse::Structured` with
  `Value::String(chunk)` — the hack that has caused bugs.
- Tool call starts still overload `ToolCallRequest` with empty arguments.
- The streaming and persistence layers remain coupled.

### EventPart with a generic Json variant instead of Structured + ToolCall

Use `EventPart::Json(String)` for both structured response chunks and tool call
argument chunks, routing by index buffer type in EventBuilder.

Rejected because:

- The EventBuilder already routes by type (message vs reasoning vs structured vs
  tool call). Having typed variants at the EventPart level is consistent with
  this routing.
- `Json(String)` loses the tool call identity information that `ToolCall(Start {
  id, name })` provides.
- Two different streaming use cases (structured data, tool call arguments) have
  different downstream handling — typed variants make this explicit.

## Non-Goals

- **Incremental tool call argument streaming.** This RFD keeps the
  `ToolCallRequestAggregator` in the EventBuilder for argument parsing. The
  `ArgumentChunk` variant forwards raw JSON to the aggregator, same as today's
  behavior but with cleaner transport.

- **Changing ConversationEvent.** The persistence type is unchanged. This RFD
  only affects the streaming transport layer.

## Risks and Open Questions

- **Provider-specific streaming patterns.** Some providers may stream tool call
  `id` and `name` across multiple deltas rather than in a single start event.
  The `ToolCallPart::Start` design handles this via first-non-empty-wins
  semantics (same as today's `merge_tool_call`), but this needs validation with
  all supported providers (Anthropic, OpenRouter, Llamacpp, Ollama, Google).

- **Metadata edge cases.** Moving metadata from `ConversationEvent` to
  `Event::Part` should be straightforward, but any provider-specific metadata
  patterns need testing.

## Implementation Plan

### Phase 1: Define EventPart types

Add `EventPart`, `ToolCallPart` to `jp_llm`. These are pure types with no
behavioral changes.

### Phase 2: Update EventBuilder

Change `EventBuilder::handle_part` to accept `EventPart` instead of
`ConversationEvent`. Update the `IndexBuffer` creation and accumulation logic.
The output (flushed `ConversationEvent`s) is identical.

### Phase 3: Update providers

Update Anthropic, OpenRouter, Llamacpp, Ollama, and Google providers to emit
`EventPart` variants instead of constructing `ConversationEvent`s. Remove
`ConversationEvent` construction from the provider layer entirely.

Move the `ToolCallRequestAggregator` from the provider layer into the
EventBuilder — providers now emit `ToolCall(ArgumentChunk(chunk))` instead of
buffering chunks themselves.

### Phase 4: Update TurnCoordinator

Update `TurnCoordinator::handle_streaming_event` to match on `EventPart` instead
of `ConversationEvent`. Update the chat renderer, structured renderer, and tool
renderer integration points.

### Phase 5: Clean up

Remove `Event`'s `as_conversation_event()`, `into_conversation_event()`, and
related helper methods that assume `Part` carries a `ConversationEvent`. Add
equivalent helpers for `EventPart` if needed.

## References

- `crates/jp_llm/src/event.rs` — current `Event` type
- `crates/jp_conversation/src/event.rs` — `ConversationEvent`, `EventKind`
- `crates/jp_conversation/src/event_builder.rs` — `EventBuilder`
- `crates/jp_cli/src/cmd/query/turn/coordinator.rs` — `TurnCoordinator`
- `crates/jp_llm/src/stream/aggregator/tool_call_request.rs` —
  `ToolCallRequestAggregator`

[RFD 048]: 048-four-channel-output-model.md
