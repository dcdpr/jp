# RFD 012: Typed LLM Streaming Events

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-15
- **Summary**: Decouples streaming transport from persistence by replacing
  `ConversationEvent` in `Event::Part` with a dedicated `EventPart` enum.

## Summary

This RFD replaces `ConversationEvent` inside `jp_llm::Event::Part` with an
`EventPart` enum that models what LLM providers stream: message chunks,
reasoning chunks, structured data chunks, and tool call lifecycle events.
Replacing `ConversationEvent` decouples the streaming transport layer from the
persistence layer, eliminates invalid states, and prepares for incremental tool
call argument streaming.

## Motivation

`Event::Part` currently carries a `ConversationEvent`:

```rust
pub enum Event {
    Part { index: usize, event: ConversationEvent },
    Flush { index: usize, metadata: IndexMap<String, Value> },
    Finished(FinishReason),
}
```

`ConversationEvent` is a persistence type that carries a timestamp, a metadata
map, and one of seven `EventKind` variants: `TurnStart`, `ChatRequest`,
`ChatResponse`, `ToolCallRequest`, `ToolCallResponse`, `InquiryRequest`, or
`InquiryResponse`.

Only a subset of these variants is valid during streaming:

| EventKind variant          | Used in `Event::Part`?                           |
| -------------------------- | ------------------------------------------------ |
| `ChatResponse::Message`    | Yes: message content chunks                      |
| `ChatResponse::Reasoning`  | Yes: reasoning content chunks                    |
| `ChatResponse::Structured` | Yes, hacked as `Value::String(chunk)`            |
| `ToolCallRequest`          | Yes, only as a start signal with empty arguments |
| `TurnStart`                | No                                               |
| `ChatRequest`              | No                                               |
| `ToolCallResponse`         | No                                               |
| `InquiryRequest`           | No                                               |
| `InquiryResponse`          | No                                               |

Coupling `Event::Part` to `ConversationEvent` causes several problems:

1. **Invalid states are representable.** `EventBuilder::handle_part` must
   silently ignore `ChatRequest`, `ToolCallResponse`, `InquiryRequest`,
   `InquiryResponse`, and `TurnStart`.
   These variants should be unrepresentable in `Event::Part`.

2. **Structured data is a hack.** Structured response chunks piggyback on
   `ChatResponse::Structured { data: Value::String(chunk) }`, abusing a
   persistence type as a transport wrapper.
   This has caused bugs where consumers process structured data chunks as
   regular message content.

3. **Tool call starts are overloaded.** When a tool call begins, the provider
   emits a `ToolCallRequest` with empty arguments, using a persistence type as a
   start signal.

4. **Wasted allocations.** Every streaming chunk allocates a `ConversationEvent`
   with a timestamp and metadata map that nothing reads during streaming.

5. **Growing `ConversationEvent`.** As `ConversationEvent` gains new event kinds
   and richer metadata, every streaming chunk pays the cost while using only a
   fraction of the type.

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

`EventBuilder` translates between the streaming domain (`EventPart`) and the
persistence domain (`ConversationEvent`):

| EventPart received             | Buffer action                    | ConversationEvent on Flush                       |
| ------------------------------ | -------------------------------- | ------------------------------------------------ |
| `Message(s)`                   | Append to string buffer          | `ChatResponse::Message { message }`              |
| `Reasoning(s)`                 | Append to string buffer          | `ChatResponse::Reasoning { reasoning }`          |
| `Structured(s)`                | Append to JSON string buffer     | Parse JSON → `ChatResponse::Structured { data }` |
| `ToolCall(Start { id, name })` | Initialize tool call buffer      | None                                             |
| `ToolCall(ArgumentChunk(s))`   | Feed `ToolCallRequestAggregator` | `ToolCallRequest { id, name, arguments }`        |

`ToolCallRequestAggregator` is unchanged: it buffers raw JSON argument chunks
and parses them on finalize.
This RFD changes only the streaming transport, not argument parsing.

### Metadata handling

Streaming metadata, such as Anthropic's thinking signatures via
`SignatureDelta`, moves from `ConversationEvent`'s metadata map to a field on
`Event::Part`:

```rust
Event::Part {
    index: 0,
    part: EventPart::Reasoning("thinking...".into()),
    metadata: Map::new(), // usually empty; populated for signature deltas
}
```

`EventBuilder` accumulates `Part`-level metadata and merges it into the final
`ConversationEvent` metadata on `Flush`.

### Provider changes

Each provider's stream mapper constructs `EventPart` values instead of
`ConversationEvent` values:

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

`TurnCoordinator` matches directly on `EventPart` to decide what to render,
instead of inspecting `ConversationEvent` variants inside `Event::Part`:

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

Changing `Event::Part` is a breaking change within `jp_llm`.
All producers and consumers of `Event::Part` (providers, `EventBuilder`, and
`TurnCoordinator`) must be updated.

Everything outside `Event::Part` is unchanged:

- `Event::Flush`, `Event::Finished`, and the query stream pipeline's `Part` >
  `Flush` > `Finished` lifecycle.
- `ConversationEvent` and `EventKind` as the persistence types used by
  `ConversationStream`.
- `ToolCallRequestAggregator` for argument parsing (see [RFD 048] for its
  replacement).

## Drawbacks

- **Breaking change to `Event`.** Every provider and consumer of `Event::Part`
  must be updated.
  The transformation is mechanical, but it touches many files.

- **Parallel type hierarchies.** `EventPart::Message` and
  `ChatResponse::Message` both represent assistant message content, one for
  streaming and one for persistence.
  Because the two layers have different concerns, `EventBuilder` must translate
  between parallel types.

## Alternatives

### Keep ConversationEvent in Event::Part and add ValueChunk

Add `Event::ValueChunk { index, chunk }` alongside `Event::Part` for raw JSON
chunks (tool call arguments and structured responses).
This is the minimal change needed for incremental argument streaming.

Rejected because:

- `Event::Part` still carries invalid variants (`ChatRequest`, `TurnStart`, and
  others) that `EventBuilder` silently ignores.
- Structured data still piggybacks on `ChatResponse::Structured` with
  `Value::String(chunk)`.
- Tool call starts still overload `ToolCallRequest` with empty arguments.
- The streaming and persistence layers remain coupled.

### Use a generic EventPart::Json variant

Use `EventPart::Json(String)` for both structured response chunks and tool call
argument chunks, routing by index buffer type in `EventBuilder`.

Rejected because:

- `EventBuilder` already routes by content type (message, reasoning, structured,
  tool call), and typed `EventPart` variants match that routing.
- `Json(String)` cannot carry the tool call identity that `ToolCall(Start { id,
  name })` provides.
- Structured data and tool call arguments have different downstream handling,
  which separate variants make explicit.

## Non-Goals

- **Incremental tool call argument streaming.** `EventBuilder` still uses
  `ToolCallRequestAggregator` to buffer and parse arguments.
  `ArgumentChunk` forwards raw JSON to the aggregator over the new transport.

- **Changing `ConversationEvent`.** The persistence type is unchanged; this RFD
  affects only the streaming transport layer.

## Risks and Open Questions

- **Provider-specific streaming patterns.** Some providers may stream a tool
  call's `id` and `name` across multiple deltas rather than in a single start
  event.
  `ToolCallPart::Start` handles this with first-non-empty-wins semantics,
  matching `merge_tool_call`, but this behavior needs validation across all
  supported providers (Anthropic, OpenRouter, Llamacpp, Ollama, and Google).

- **Metadata edge cases.** Moving metadata from `ConversationEvent` to
  `Event::Part` requires testing provider-specific metadata patterns.

## Implementation Plan

### Phase 1: Define EventPart types

Add `EventPart` and `ToolCallPart` to `jp_llm`.
These are pure data types with no behavioral changes.

### Phase 2: Update EventBuilder

Change `EventBuilder::handle_part` to accept `EventPart` instead of
`ConversationEvent`, and update `IndexBuffer` creation and accumulation.
The flushed `ConversationEvent` output is unchanged.

### Phase 3: Update providers

Update the Anthropic, OpenRouter, Llamacpp, Ollama, and Google providers to emit
`EventPart` variants, removing `ConversationEvent` construction from the
provider layer.

Move `ToolCallRequestAggregator` from the provider layer into `EventBuilder` so
providers emit `ToolCall(ArgumentChunk(chunk))` instead of buffering chunks
themselves.

### Phase 4: Update TurnCoordinator

Update `TurnCoordinator::handle_streaming_event` to match on `EventPart` instead
of `ConversationEvent`, and update the chat, structured, and tool renderer
integration points.

### Phase 5: Clean up

Remove `Event`'s `as_conversation_event()`, `into_conversation_event()`, and
related helpers that assume `Part` carries a `ConversationEvent`.
Add equivalent helpers for `EventPart` where needed.

## References

- `crates/jp_llm/src/event.rs`: current `Event` type
- `crates/jp_conversation/src/event.rs`: `ConversationEvent`, `EventKind`
- `crates/jp_conversation/src/event_builder.rs`: `EventBuilder`
- `crates/jp_cli/src/cmd/query/turn/coordinator.rs`: `TurnCoordinator`
- `crates/jp_llm/src/stream/aggregator/tool_call_request.rs`:
  `ToolCallRequestAggregator`

[RFD 048]: 048-four-channel-output-model.md
