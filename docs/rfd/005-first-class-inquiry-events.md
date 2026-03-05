# RFD 005: First-Class Inquiry Events

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-02-17

## Summary

This RFD proposes recording `InquiryRequest` and `InquiryResponse` events in the
persisted `ConversationStream`, and introducing a centralized filtering
mechanism so that these internal events are never sent to LLM providers. This
gives us a complete audit trail of what happened during tool execution while
preserving the provider contract that requires tool call requests to be
immediately followed by tool call responses.

## Motivation

The stateful tool inquiry system (see [architecture doc][inquiry-arch]) resolves
tool questions by making structured output calls to the LLM. Today, these
inquiries happen on a cloned conversation stream that is discarded after the
answer is extracted. The real `ConversationStream`, the one persisted to disk,
contains no record that an inquiry occurred. It looks like this:

```rust
ToolCallRequest(call_123, fs_modify_file, {path, patterns})
ToolCallResponse(call_123, "File modified successfully")
```

A reader of the conversation history cannot tell that between those two events,
the tool paused, asked the LLM a question ("Create backup files?"), received an
answer (true), and then completed. That context is lost.

With first-class inquiry events, the stream would look like:

```rust
ToolCallRequest(call_123, fs_modify_file, {path, patterns})
InquiryRequest(inq_1, tool:fs_modify_file, "Create backup files?", Boolean)
InquiryResponse(inq_1, true)
ToolCallResponse(call_123, "File modified successfully")
```

This matters for:

- **Debugging**: When a tool produces unexpected results, knowing what questions
  it asked and what answers it received is essential.
- **Conversation replay**: Tooling that replays or analyzes conversations can
  reconstruct the full execution flow.
- **Future UI**: A conversation viewer could display inquiry events as a
  collapsible sub-flow within a tool call, giving users visibility into the
  system's decision-making.

If we do nothing, inquiry state remains ephemeral and invisible. The persisted
conversation is an incomplete record of what the system did.

## Design

### Overview

Three changes, in order of dependency:

1. **`EventKind::is_provider_visible()`** — A method on the event enum that
   declares whether an event type should be included in provider message
   streams. Structural and type-level.
2. **`Thread::into_parts()`** — A new decomposition method on `Thread` that
   renders system content and filters events in a single place. Providers
   consume `ThreadParts` instead of destructuring `Thread` manually.
3. **Recording inquiry events** — The `ToolCoordinator` pushes
   `InquiryRequest` and `InquiryResponse` into the real conversation stream
   at the right points during tool execution.

### Design Goals

| Goal                         | Description                                  |
|------------------------------|----------------------------------------------|
| **Complete audit trail**     | Every inquiry is recorded in the persisted   |
|                              | stream.                                      |
| **Provider contract intact** | Providers never see inquiry events. The       |
|                              | ToolCallRequest → ToolCallResponse pairing    |
|                              | is preserved in the provider-facing stream.   |
| **Single filtering point**   | Event visibility is determined once, in one   |
|                              | place. Providers don't need defensive         |
|                              | catch-all arms.                               |
| **No provider regression**   | Adding new internal event types in the future |
|                              | requires updating one method, not six         |
|                              | providers.                                    |

### Event Visibility

A new method on `EventKind`:

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

This is a positive list (allowlist), not a denylist. New event types are
invisible to providers by default — you have to opt them in. This is safer than
a denylist where forgetting to exclude a new type sends garbage to every
provider.

### Thread Decomposition

`Thread` has four public fields. Three providers use `Thread::into_messages()`
(Llamacpp, Ollama, OpenAI), which renders system content and delegates event
conversion to a closure. Three providers (Anthropic, Google, OpenRouter)
destructure `Thread` manually because their APIs need system content and
conversation messages as separate values — `into_messages` concatenates them
into a single list.

This means event filtering has no single enforcement point. Each provider
independently skips non-provider events in its own `convert_events` function,
or via `_ => None` catch-all match arms. Every new internal event type requires
touching every provider.

We introduce `Thread::into_parts()` as the universal decomposition method.
System parts are returned as tagged `SystemPart` values rather than plain
`String`s, so that providers needing per-part cache control (Anthropic,
OpenRouter) can match on the variant to decide cache placement:

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

`into_messages` delegates to `into_parts()` and flattens the tags to plain
`String`s for providers that don't need them:

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

The three providers using `into_messages` (Llamacpp, Ollama, OpenAI) get
filtering for free — no code changes. The three that destructure `Thread`
manually (Anthropic, Google, OpenRouter) migrate to `into_parts()`.

### Recording Inquiry Events

The `ToolCoordinator` records inquiry events at two points:

1. **On inquiry spawn** (`handle_tool_result`, when a question is routed through
   the inquiry backend): Push an `InquiryRequest` into the conversation stream.

2. **On inquiry result** (`InquiryResult` handler in the event loop): Push an
   `InquiryResponse` into the conversation stream.

Inquiry events are recorded for any question routed through the
`InquiryBackend`, not only those with `QuestionTarget::Assistant`. In
non-interactive environments (no TTY), user-targeted questions also fall through
to the inquiry backend and are recorded.

Questions answered via interactive user prompts (TTY + `QuestionTarget::User`)
are *not* recorded as inquiry events — they go through the `ToolPrompter` path,
which doesn't touch the conversation stream.

`execute_with_prompting` receives `events: &mut ConversationStream` so the
coordinator has write access.

The events sit between the `ToolCallRequest` and `ToolCallResponse` for the tool
in question:

```txt
ToolCallRequest(call_123, fs_modify_file, {...})
  InquiryRequest(inq_1, tool:fs_modify_file, "Create backup?", Boolean)
  InquiryResponse(inq_1, true)
ToolCallResponse(call_123, "File modified successfully")
```

For multi-question tools, multiple inquiry pairs appear:

```txt
ToolCallRequest(call_123, fs_modify_file, {...})
  InquiryRequest(inq_1, tool:fs_modify_file, "Create backup?", Boolean)
  InquiryResponse(inq_1, true)
  InquiryRequest(inq_2, tool:fs_modify_file, "Overwrite existing?", Boolean)
  InquiryResponse(inq_2, false)
ToolCallResponse(call_123, "File modified successfully")
```

For parallel tool calls, inquiry events for different tools may interleave, but
each `InquiryRequest`/`InquiryResponse` pair is matched by `id`, so ordering
between tools doesn't affect correctness.

### Schema Extraction Edge Case

Several providers check `events.last().as_chat_request().schema` to detect
structured output requests. If inquiry events were at the tail of the stream,
this check could break. In practice, inquiry events always sit between a
`ToolCallRequest` and `ToolCallResponse` — never at the tail — so this is not an
issue. The `into_parts()` filtering makes it impossible regardless, since the
provider receives a stream where inquiry events don't exist.

### Serialization

`InquiryRequest` and `InquiryResponse` are already fully integrated into the
serialization system. `EventKind` includes both variants,
`InternalEventFlattened` (the optimized deserializer) handles them, and
`ConversationStream` has `add_inquiry_request()` / `add_inquiry_response()`
helpers. No serialization work is needed.

## Drawbacks

- **Stream size**: Inquiry events add to the persisted conversation. Each
  inquiry pair is small (the question text + a JSON answer value), but tools
  with many questions over many turns will accumulate. This is a minor cost
  compared to the tool arguments and responses already stored.

- **Mutable stream reference**: `execute_with_prompting` and
  `handle_tool_result` take `&mut ConversationStream`. The change is contained
  within the tool execution module.

## Alternatives

### Per-event metadata flag (`provider_hidden: bool`)

Instead of type-level visibility, add a `provider_hidden` field to
`ConversationEvent` that any event instance can set.

Rejected because: for inquiry events, the flag would always be `true` — it's a
property of the event *type*, not individual instances. A runtime flag adds
serialization overhead to every event and makes the filtering intent implicit
rather than structural. If a future need arises for per-instance visibility
control (e.g. hiding a specific `ChatRequest`), this field can be added later in
a backward-compatible way.

### Filter in each provider (status quo)

Keep the current pattern where each provider's `convert_events` function skips
non-provider events via pattern matching.

Rejected because: it's fragile. Every new internal event type requires touching
every provider. The three providers that don't use `into_messages` each have
their own filtering logic (Anthropic and OpenRouter explicitly match
`InquiryRequest` / `InquiryResponse`; Google, Llamacpp, and Ollama use `_ =>
None`). This is the pattern we're trying to eliminate.

### Filter via `ConversationStream::provider_events()` method

Add a method that returns a filtered iterator, and have each provider call it
instead of `into_iter()`.

This is better than the status quo but still requires each provider to remember
to call the right method. A new provider that calls `into_iter()` instead of
`provider_events()` silently gets unfiltered events. `into_parts()` makes the
filtering mandatory — you can't get the events without going through it.

## Non-Goals

- **Rendering inquiry events in `conversation show`**: Display formatting for
  inquiry events in the CLI is deferred. The events are stored and visible in
  the raw JSON. A future change can add pretty-printing.
- **Exposing inquiry events to the LLM**: Inquiry events are always
  provider-hidden. The inquiry itself is already represented by a `ChatRequest`
  (with structured schema) and `ChatResponse::Structured` in the inquiry
  backend's cloned stream — those are the provider-visible artifacts. The
  `InquiryRequest`/`InquiryResponse` events are our internal bookkeeping.
- **Batching multiple inquiries**: Combining questions from multiple tools into
  a single structured request. Noted as a future enhancement in the [inquiry
  architecture doc][inquiry-arch].

## Risks and Open Questions

- **Parallel tool calls and stream ordering**: When multiple tools run in
  parallel and both trigger inquiries, their `InquiryRequest`/`InquiryResponse`
  events may interleave in the stream. Each pair is correlated by `id`, so this
  is correct, but it may look confusing in the raw JSON. We could buffer inquiry
  events and insert them in tool-order, but this adds complexity for marginal
  readability benefit.

## Implementation Plan

### Phase 1: Event visibility and `into_parts()`

Add `EventKind::is_provider_visible()` to `jp_conversation`. Add
`Thread::into_parts()` with the `SystemPart` enum and refactor `into_messages()`
to delegate to it.

No provider changes. No behavioral changes. Can be merged independently.

### Phase 2: Record inquiry events in the stream

In `ToolCoordinator`, push `InquiryRequest` on inquiry spawn and
`InquiryResponse` on inquiry result. Change `execute_with_prompting` to take
`events: &mut ConversationStream`.

Depends on Phase 1 (filtering must be in place before events are recorded).

### Phase 3: Migrate OpenRouter to `into_parts()`

Replace manual `Thread` destructuring with `into_parts()`. The explicit
`InquiryRequest | InquiryResponse | TurnStart => vec![]` arm in `convert_events`
becomes a `_ => vec![]` catch-all (still needed for exhaustive matching, but
internal events never reach it).

Independent of Phase 2.

### Phase 4: Migrate Google to `into_parts()`

Replace manual `Thread` destructuring. System parts go to `system_instruction`,
filtered events go to `convert_events`.

Independent of Phase 3.

### Phase 5: Migrate Anthropic to `into_parts()`

Replace manual `Thread` destructuring. Anthropic extracts attachments from the
thread before calling `into_parts()` to send them as native document blocks
rather than XML. Cache control budget distribution across `system_parts` and
events uses the `SystemPart` tags — no complex counter threading is needed.

Independent of Phase 4.

## References

- [Stateful Tool Inquiries Architecture][inquiry-arch] — the inquiry system
  this RFD builds on.
- [Query Stream Pipeline](../architecture/query-stream-pipeline.md) — turn
  loop and streaming architecture.
- `jp_conversation::event::inquiry` — existing `InquiryRequest` and
  `InquiryResponse` event types.
- `jp_conversation::thread::Thread` — the thread type being extended with
  `into_parts()`.

[inquiry-arch]: 028-structured-inquiry-system-for-tool-questions.md
