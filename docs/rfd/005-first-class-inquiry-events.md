# RFD 005: First-Class Inquiry Events

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-02-17
- **Extended by**: [RFD 082]
- **Summary**: Records inquiry events in the conversation stream and filters
  them out before sending history to LLM providers.

## Summary

This RFD proposes recording `InquiryRequest` and `InquiryResponse` events in the
persisted `ConversationStream` and filtering them out in one place before
sending the stream to LLM providers.
This preserves a complete audit trail of tool execution without breaking the
provider contract that tool call responses immediately follow tool call
requests.

## Motivation

The stateful tool inquiry system (see [architecture doc][inquiry-arch]) resolves
tool questions by making structured output calls to the LLM.
These inquiries run on a cloned conversation stream that is discarded after the
answer is extracted, leaving no record in the persisted `ConversationStream`:

```rust
ToolCallRequest(call_123, fs_modify_file, {path, patterns})
ToolCallResponse(call_123, "File modified successfully")
```

Nothing in the history shows that between those two events the tool paused,
asked the LLM a question ("Create backup files?"), and received an answer (true)
before completing.

With first-class inquiry events, the stream records the exchange:

```rust
ToolCallRequest(call_123, fs_modify_file, {path, patterns})
InquiryRequest(inq_1, tool:fs_modify_file, "Create backup files?", Boolean)
InquiryResponse(inq_1, true)
ToolCallResponse(call_123, "File modified successfully")
```

Persisting these events supports three use cases:

- **Debugging**: Inspecting unexpected tool behavior requires knowing what
  questions the tool asked and what answers it received.
- **Conversation replay**: Tooling that replays or analyzes conversations can
  reconstruct the full execution flow.
- **Future UI**: A conversation viewer can display inquiries as a collapsible
  sub-flow under the parent tool call.

## Design

### Overview

The design has three parts, in dependency order:

1. **`EventKind::is_provider_visible()`**: A type-level predicate on `EventKind`
   that declares whether an event variant belongs in provider message streams.
2. **`Thread::into_parts()`**: A decomposition method on `Thread` that renders
   system content and filters events in one place.
   Providers consume `ThreadParts` instead of destructuring `Thread` directly.
3. **Recording inquiry events**: `ToolCoordinator` pushes `InquiryRequest` and
   `InquiryResponse` events into the conversation stream during tool execution.

### Design Goals

| Goal                         | Description                                  |
| ---------------------------- | -------------------------------------------- |
| **Complete audit trail**     | Every inquiry is recorded in the persisted   |
|                              | stream.                                      |
| **Provider contract intact** | Providers never see inquiry events. The      |
|                              | ToolCallRequest → ToolCallResponse pairing   |
|                              | is preserved in the provider-facing stream.  |
| **Single filtering point**   | Event visibility is defined in one place, so |
|                              | providers do not need defensive catch-all    |
|                              | arms.                                        |
| **No provider regression**   | Adding an internal event type requires       |
|                              | updating one method, not all six providers.  |

### Event Visibility

`EventKind::is_provider_visible()` defines which event variants are sent to
providers:

```rust
impl EventKind {
    /// Whether this event should be included when building provider messages.
    ///
    /// Internal events (turn markers, inquiry exchanges) are filtered out
    /// before the conversation stream reaches any provider's message
    /// conversion logic. Uses an allowlist so new event types are invisible
    /// to providers by default.
    pub const fn is_provider_visible(&self) -> bool {
        matches!(
            self,
            Self::ChatRequest(_)
                | Self::ChatResponse(_)
                | Self::ToolCallRequest(_)
                | Self::ToolCallResponse(_)
        )
    }
}
```

`is_provider_visible()` uses an allowlist rather than a denylist.
New event variants are hidden from providers unless explicitly opted in, so
adding an internal event cannot accidentally leak it to providers.

### Thread Decomposition

Three providers (Llamacpp, Ollama, OpenAI) call `Thread::into_messages()`, which
renders system content and passes the event stream to a conversion closure.
The other three (Anthropic, Google, OpenRouter) destructure `Thread`'s four
public fields directly because their APIs take system content and conversation
messages separately, whereas `into_messages` concatenates them into a single
list.

Because `Thread` has no single decomposition path, each provider filters
non-provider events on its own in `convert_events` or with `_ => None` catch-all
arms.
Adding an internal event type requires updating every provider.

`Thread::into_parts()` replaces manual destructuring as the universal
decomposition method.
It returns system content as tagged `SystemPart` values rather than plain
`String`s so providers with per-part cache control (Anthropic, OpenRouter) can
match on the variant to place cache breakpoints:

```rust
/// A rendered piece of system content, tagged by origin.
pub enum SystemPart {
    /// A prompt or section string (system prompt, instructions, context).
    Prompt(String),

    /// Rendered attachment XML.
    Attachment(String),
}

pub struct ThreadParts {
    /// Rendered system content, tagged by origin.
    pub system_parts: Vec<SystemPart>,

    /// Conversation events filtered to provider-visible events only.
    pub events: ConversationStream,
}

impl Thread {
    /// Decompose the thread into rendered system parts and filtered events.
    pub fn into_parts(self) -> Result<ThreadParts> { /* ... */ }
}
```

`into_messages` delegates to `into_parts()` and unwraps `SystemPart` values into
plain `String`s for providers that do not need the origin tags:

```rust
pub fn into_messages<T, U, M, S>(
    self,
    to_system_messages: M,
    convert_stream: S,
) -> Result<Vec<T>> {
    let parts = self.into_parts()?;
    let strings: Vec<String> = parts
        .system_parts
        .into_iter()
        .map(SystemPart::into_inner)
        .collect();

    let mut items = vec![];
    items.extend(to_system_messages(strings));
    items.extend(convert_stream(parts.events));
    Ok(items)
}
```

Llamacpp, Ollama, and OpenAI inherit the filtering through `into_messages`
without code changes.
Anthropic, Google, and OpenRouter migrate from manual destructuring to
`into_parts()`.

### Recording Inquiry Events

`ToolCoordinator` records inquiry events at two points:

1. **On question arrival** (`handle_tool_result`, before any routing decision):
   Push an `InquiryRequest` into the conversation stream.
2. **On resolution**: Push an `InquiryResponse` into the conversation stream:
   `Answered` when an answer resolves the question, `Redacted` for
   `AnswerType::Secret` answers, or `Cancelled` when the question closes without
   an answer.

Inquiry events are recorded for every `Outcome::NeedsInput` round-trip,
regardless of routing path (amended by [RFD 082]; this section originally
excluded prompter-answered questions):

- Interactive prompter (TTY + `QuestionTarget::User`).
- Cached "remember for turn" answer from earlier in the same turn.
- Static `QuestionConfig.answer` value.
- `InquiryBackend` (explicit `QuestionTarget::Assistant`, or user-targeted
  questions falling through in non-interactive environments).

When a question closes without an answer, the coordinator completes the pair
with `InquiryResponse::Cancelled { reason }` rather than leaving the request
dangling.
The `reason` is `user` on user cancellation (Ctrl-C at the prompt or a
user-triggered cancellation token), `backend_error` on an inquiry-backend
failure, and `no_prompt_backend` or `assistant_routing_denied` when an
`AnswerType::Secret` question cannot be routed to a human.

`execute_with_prompting` takes `events: &mut ConversationStream` so
`ToolCoordinator` can append these events between the tool's `ToolCallRequest`
and `ToolCallResponse`:

```txt
ToolCallRequest(call_123, fs_modify_file, {...})
  InquiryRequest(inq_1, tool:fs_modify_file, "Create backup?", Boolean)
  InquiryResponse(inq_1, true)
ToolCallResponse(call_123, "File modified successfully")
```

A tool that asks multiple questions records one pair per question:

```txt
ToolCallRequest(call_123, fs_modify_file, {...})
  InquiryRequest(inq_1, tool:fs_modify_file, "Create backup?", Boolean)
  InquiryResponse(inq_1, true)
  InquiryRequest(inq_2, tool:fs_modify_file, "Overwrite existing?", Boolean)
  InquiryResponse(inq_2, false)
ToolCallResponse(call_123, "File modified successfully")
```

When tools run in parallel, inquiry events from different tools may interleave;
matching each `InquiryRequest` and `InquiryResponse` by `id` keeps pairs
unambiguous.

### Schema Extraction Edge Case

Several providers inspect `events.last().as_chat_request().schema` to detect
structured output requests.
An inquiry event at the tail of the stream would break this check, but two
invariants prevent that: inquiry events are always followed by a
`ToolCallResponse` before the next provider call, and `into_parts()` strips them
before the provider sees the stream.

### Serialization

No serialization changes are needed.
`EventKind` already includes `InquiryRequest` and `InquiryResponse`,
`InternalEventFlattened` (the optimized deserializer) handles both variants, and
`ConversationStream` provides `add_inquiry_request()` and
`add_inquiry_response()` helpers.

## Drawbacks

- **Stream size**: Each inquiry adds a question string and a JSON answer value
  to the persisted stream.
  Even across many turns, this overhead is minor compared to tool call arguments
  and responses.
- **Mutable stream reference**: `execute_with_prompting` and
  `handle_tool_result` must take `&mut ConversationStream`, though the signature
  change is confined to the tool execution module.

## Alternatives

### Per-event metadata flag (`provider_hidden: bool`)

Add a `provider_hidden` field to `ConversationEvent` so any event instance can
hide itself from providers.

Rejected because visibility for inquiry events is a property of the event type,
not of individual instances.
A runtime flag adds serialization overhead to every event and makes filtering
implicit rather than structural.
If per-instance visibility is needed later (for example, hiding a specific
`ChatRequest`), the field can be added in a backward-compatible way.

### Filter in each provider (status quo)

Keep filtering in each provider's `convert_events` function (Anthropic and
OpenRouter match `InquiryRequest` and `InquiryResponse` explicitly; Google,
Llamacpp, and Ollama rely on `_ => None` catch-all arms).

Rejected because every new internal event type requires updating every provider.

### Filter via `ConversationStream::provider_events()`

Add a method that returns a filtered iterator and have each provider call it
instead of `into_iter()`.

Rejected because filtering remains opt-in: a provider that calls `into_iter()`
by mistake silently receives unfiltered events.
`into_parts()` makes filtering mandatory before a provider can access the event
stream.

## Non-Goals

- **Rendering inquiry events in `conversation show`**: The events are stored and
  visible in the raw JSON, but CLI formatting is deferred.
- **Exposing inquiry events to the LLM**: `InquiryRequest` and `InquiryResponse`
  are internal bookkeeping and are always hidden from providers.
  The inquiry backend's cloned stream already represents the exchange to the
  provider as a structured `ChatRequest` and `ChatResponse::Structured`.
- **Batching multiple inquiries**: Combining questions from multiple tools into
  a single structured request remains future work (see the [inquiry architecture
  doc][inquiry-arch]).

## Risks and Open Questions

- **Parallel tool calls and stream ordering**: When parallel tools trigger
  inquiries concurrently, their `InquiryRequest` and `InquiryResponse` events
  may interleave in the stream.
  Each pair is correlated by `id`, so execution is unaffected, and buffering
  events to group them by tool is not worth the added complexity just to make
  raw JSON easier to read.

## Implementation Plan

### Phase 1: Event visibility and `into_parts()`

Add `EventKind::is_provider_visible()` to `jp_conversation`.
Add `Thread::into_parts()` with the `SystemPart` enum and refactor
`into_messages()` to delegate to it.

This phase makes no provider or behavioral changes and can be merged
independently.

### Phase 2: Record inquiry events in the stream

In `ToolCoordinator`, push `InquiryRequest` on inquiry spawn and
`InquiryResponse` on inquiry result.
Change `execute_with_prompting` to take `events: &mut ConversationStream`.

Depends on Phase 1 (filtering must be in place before events are recorded).

### Phase 3: Migrate OpenRouter to `into_parts()`

Replace manual `Thread` destructuring with `into_parts()`.
The explicit `InquiryRequest | InquiryResponse | TurnStart => vec![]` arm in
`convert_events` becomes a `_ => vec![]` catch-all (still needed for exhaustive
matching, but internal events never reach it).

Independent of Phase 2.

### Phase 4: Migrate Google to `into_parts()`

Replace manual `Thread` destructuring.
Pass `system_parts` to `system_instruction` and filtered `events` to
`convert_events`.

Independent of Phase 3.

### Phase 5: Migrate Anthropic to `into_parts()`

Replace manual `Thread` destructuring.
Anthropic extracts attachments from the thread before calling `into_parts()` to
send them as native document blocks rather than XML.
Cache control budget distribution across `system_parts` and events matches on
the `SystemPart` tags instead of threading a counter.

Independent of Phase 4.

## References

- [Stateful Tool Inquiries Architecture][inquiry-arch]: The inquiry system this
  RFD builds on.
- [Query Stream Pipeline]: Turn loop and streaming architecture.
- `jp_conversation::event::inquiry`: Existing `InquiryRequest` and
  `InquiryResponse` event types.
- `jp_conversation::thread::Thread`: The thread type extended with
  `into_parts()`.

[Query Stream Pipeline]: ../wiki/00dxyb6-query-stream-pipeline-architecture.md
[RFD 082]: 082-unified-inquiry-event-recording.md
[inquiry-arch]: 028-structured-inquiry-system-for-tool-questions.md
