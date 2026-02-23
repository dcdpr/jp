# RFD 004: First-Class Inquiry Events

- **Status**: Draft
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
    /// conversion logic.
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

Today, `Thread` has four public fields. Three providers use
`Thread::into_messages()` (Llamacpp, Ollama, OpenAI), which renders system
content and delegates event conversion to a closure. Three providers (Anthropic,
Google, OpenRouter) destructure `Thread` manually because their APIs need system
content and conversation messages as separate values — `into_messages`
concatenates them into a single list.

This means event filtering has no single enforcement point. Each provider
independently skips non-provider events in its own `convert_events` function,
or via `_ => None` catch-all match arms. Every new internal event type requires
touching every provider.

We introduce `Thread::into_parts()` as the universal decomposition method:

```rust
pub struct ThreadParts {
    /// Rendered system prompt, sections, and attachment XML.
    pub system_parts: Vec<String>,

    /// Conversation events, filtered to provider-visible events only.
    pub events: ConversationStream,
}

impl Thread {
    /// Decompose the thread into rendered system parts and filtered events.
    ///
    /// System prompt, sections, and attachments are rendered to strings.
    /// Events are filtered to exclude internal types (InquiryRequest,
    /// InquiryResponse, TurnStart) via `EventKind::is_provider_visible()`.
    pub fn into_parts(self) -> Result<ThreadParts> { /* ... */ }
}
```

`into_messages` is refactored to delegate to `into_parts()`:

```rust
pub fn into_messages<T, U, M, S>(
    self,
    to_system_messages: M,
    convert_stream: S,
) -> Result<Vec<T>> {
    let parts = self.into_parts()?;
    let mut items = vec![];
    items.extend(to_system_messages(parts.system_parts));
    items.extend(convert_stream(parts.events));
    Ok(items)
}
```

The three providers already using `into_messages` get filtering for free — no
code changes. The three that currently destructure `Thread` can migrate to
`into_parts()` at their own pace.

### Provider Migration

Each provider that currently bypasses `into_messages` has different reasons for
doing so. Here is the migration path for each:

#### OpenRouter — Low-Medium Complexity

OpenRouter's system content is a standard system message (not a separate API
field like Anthropic/Google), so it's structurally compatible with
`into_parts()`. The main wrinkle is per-item cache control annotations on
system parts, and a post-processing step that strips cache directives for
non-Anthropic/Google sub-providers. Both are manageable — the cache annotations
can be applied when processing `system_parts`, and post-processing happens after
message construction regardless.

#### Google — Medium-High Complexity

Google uses a separate `system_instruction` field on the request. The system
parts from `into_parts()` would be wrapped in `types::ContentPart` items and
assigned to `system_instruction`, while `events` feeds `convert_events`. The
separation that `into_parts()` provides is exactly what Google needs. The
complexity is moderate because Google's event conversion maintains a
`tool_call_names` map (tracking tool call IDs → names for `FunctionResponse`),
and the `thought_signature` handling adds some per-event state. These are
self-contained within the event conversion closure.

#### Anthropic — High Complexity

Anthropic has two structural challenges:

1. **Separate system field.** Like Google, system content goes in a separate
   `builder.system()` call. `into_parts()` handles this — system parts and
   events are already separate.

2. **Shared cache control budget.** Anthropic limits cache control points to 4
   per request. The budget is distributed across system content (prompt,
   sections, attachments, tools) AND the last conversation message. A mutable
   counter is threaded through all construction steps. `into_parts()` renders
   system parts before the provider sees them, but the provider still needs to
   attach cache control to individual parts and to the last conversation
   message, sharing a single budget across both.

   This can be solved by having the Anthropic provider process `system_parts`
   first (consuming cache control points), then convert events with the
   remaining budget. The counter threading stays within Anthropic's
   `create_request` — it doesn't leak into the shared abstraction.

Additionally, Anthropic's event conversion needs per-event config access
(`event.config.assistant.model.id`) to determine thinking signature format.
This is available from `ConversationEventWithConfig` during stream iteration,
so it works with the filtered `ConversationStream` from `into_parts()`.

Anthropic migration is the most work but has the highest payoff — it's the
most complex provider and benefits most from not having to manually filter
events.

### Recording Inquiry Events

The `ToolCoordinator` records inquiry events at two points:

1. **On inquiry spawn** (`handle_tool_result`, when `NeedsInput` +
   `QuestionTarget::Assistant`): Push an `InquiryRequest` into the real
   conversation stream.

2. **On inquiry result** (`InquiryResult` handler in the event loop): Push
   an `InquiryResponse` into the real conversation stream.

This requires the `ToolCoordinator` to have write access to the conversation
stream. Today it receives `inquiry_events: &ConversationStream` (a read-only
reference, cloned just-in-time for the inquiry backend). Recording events
means passing `&mut ConversationStream` instead.

The events sit between the `ToolCallRequest` and `ToolCallResponse` for the
tool in question:

```
ToolCallRequest(call_123, fs_modify_file, {...})
  InquiryRequest(inq_1, tool:fs_modify_file, "Create backup?", Boolean)
  InquiryResponse(inq_1, true)
ToolCallResponse(call_123, "File modified successfully")
```

For multi-question tools, multiple inquiry pairs appear:

```
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
`ToolCallRequest` and `ToolCallResponse` — never at the tail — so this is not
an issue. The `into_parts()` filtering makes it impossible regardless, since
the provider receives a stream where inquiry events don't exist.

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

- **Mutable stream reference**: The `ToolCoordinator` currently takes a
  read-only reference to the conversation stream. Recording events requires
  `&mut`. This changes the signature of `execute_with_prompting` and
  `handle_tool_result`. The change is contained within the tool execution
  module.

- **Migration effort**: Migrating all six providers to `into_parts()` is
  non-trivial, especially Anthropic. The phased approach mitigates this —
  filtering works correctly at every step, and provider migration can happen
  incrementally.

## Alternatives

### Per-event metadata flag (`provider_hidden: bool`)

Instead of type-level visibility, add a `provider_hidden` field to
`ConversationEvent` that any event instance can set.

Rejected because: for inquiry events, the flag would always be `true` — it's a
property of the event *type*, not individual instances. A runtime flag adds
serialization overhead to every event and makes the filtering intent implicit
rather than structural. If a future need arises for per-instance visibility
control (e.g. hiding a specific `ChatRequest`), this field can be added later
in a backward-compatible way.

### Filter in each provider (status quo)

Keep the current pattern where each provider's `convert_events` function skips
non-provider events via pattern matching.

Rejected because: it's fragile. Every new internal event type requires touching
every provider. The three providers that don't use `into_messages` each have
their own filtering logic (Anthropic and OpenRouter explicitly match
`InquiryRequest` / `InquiryResponse`; Google, Llamacpp, and Ollama use
`_ => None`). This is the pattern we're trying to eliminate.

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
  a single structured request. Noted as a future enhancement in the
  [inquiry architecture doc][inquiry-arch].

## Risks and Open Questions

- **Parallel tool calls and stream ordering**: When multiple tools run in
  parallel and both trigger inquiries, their `InquiryRequest`/`InquiryResponse`
  events may interleave in the stream. Each pair is correlated by `id`, so
  this is correct, but it may look confusing in the raw JSON. We could buffer
  inquiry events and insert them in tool-order, but this adds complexity for
  marginal readability benefit.

- **`into_parts()` adoption pace**: The feature works without migrating all
  providers — filtering in `into_parts()` covers the three `into_messages`
  users, and the remaining three already filter manually. The risk is that the
  manual filters become stale if new internal event types are added before those
  providers migrate. Mitigated by making `is_provider_visible()` the canonical
  check and having providers reference it even if they don't yet use
  `into_parts()`.

- **Anthropic migration complexity**: The cache control budget threading is the
  hardest part. If it proves too entangled, Anthropic can remain on manual
  decomposition indefinitely — as long as it calls `is_provider_visible()` or
  `events.retain(|e| e.kind.is_provider_visible())` instead of hardcoding event
  type filters.

## Implementation Plan

### Phase 1: Event visibility and `into_parts()`

Add `EventKind::is_provider_visible()` to `jp_conversation`. Add
`Thread::into_parts()` and refactor `into_messages()` to delegate to it.

No provider changes. No behavioral changes. Can be merged independently.

### Phase 2: Record inquiry events in the stream

In `ToolCoordinator`, push `InquiryRequest` on inquiry spawn and
`InquiryResponse` on inquiry result. Change `inquiry_events` from
`&ConversationStream` to `&mut ConversationStream`. Update tests.

Depends on Phase 1 (filtering must be in place before events are recorded).

### Phase 3: Migrate OpenRouter to `into_parts()`

Replace manual `Thread` destructuring with `into_parts()`. Remove the
explicit `InquiryRequest | InquiryResponse | TurnStart => vec![]` arm from
`convert_events`. Good first migration candidate — lowest complexity.

Independent of Phase 2.

### Phase 4: Migrate Google to `into_parts()`

Replace manual `Thread` destructuring. System parts go to
`system_instruction`, filtered events go to `convert_events`. Remove the
`_ => None` catch-all that currently handles non-provider events.

Independent of Phase 3.

### Phase 5: Migrate Anthropic to `into_parts()`

Replace manual `Thread` destructuring. Handle cache control budget
distribution across `system_parts` and events within `create_request`.
Remove explicit inquiry/turn-start filtering from `convert_event`.

Independent of Phase 4. Can be deferred if the cache control threading
proves too complex — the feature works without this migration.

## References

- [Stateful Tool Inquiries Architecture][inquiry-arch] — the inquiry system
  this RFD builds on.
- [Query Stream Pipeline](../architecture/query-stream-pipeline.md) — turn
  loop and streaming architecture.
- `jp_conversation::event::inquiry` — existing `InquiryRequest` and
  `InquiryResponse` event types.
- `jp_conversation::thread::Thread` — the thread type being extended with
  `into_parts()`.

[inquiry-arch]: ../architecture/stateful-tool-inquiries.md
