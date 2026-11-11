# RFD D18: Plugin Event Subscriptions and Query Delegation

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-06
- **Extends**: [RFD 072]
- **Summary**: Extends the command plugin protocol with live conversation event
  subscriptions, interactive prompt handling, and query delegation.

## Summary

This RFD extends the command plugin protocol ([RFD 072]) with live event
subscriptions and agent loop delegation so plugins can run full chat frontends.
Plugins can subscribe to streaming conversation events, answer interactive
approval and inquiry prompts, and trigger LLM queries that JP executes on their
behalf.

## Motivation

The request/response protocol in [RFD 072] covers workspace queries and
mutations: a plugin can list conversations, read and write events, acquire
locks, and render formatted output.
That model supports read-only tools (web viewers, exporters) and batch write
tools (importers, bulk editors), but it cannot drive a chat interface.

A chat plugin needs to:

1. **Stream LLM responses in real time**: Tokens must arrive at the plugin as
   they are generated, not after the full response completes.
2. **Handle tool approval prompts**: When the agent loop wants to run a tool,
   the plugin must present the approval UI and return the user's decision.
3. **Handle inquiry questions**: Tools may ask questions that require user
   input.
4. **Observe concurrent activity**: If another `jp query` session writes to the
   same conversation, the plugin must see those events as they happen.

All four require JP to push messages to the plugin, which the request/response
model from [RFD 072] does not support.

## Design

### Subscriptions

A plugin subscribes to a conversation to receive events as they occur:

```json
{
  "type": "subscribe",
  "sub_id": "chat-main",
  "conversation": "17127583920",
  "events": [
    "chat_response",
    "tool_call_request",
    "tool_call_response"
  ]
}
```

JP confirms the subscription:

```json
{
  "type": "subscribed",
  "sub_id": "chat-main"
}
```

The plugin chooses a `sub_id` that is unique among its active subscriptions.
JP echoes `sub_id` on every pushed event so the plugin can multiplex independent
subscriptions (for example, two side-by-side chat windows, or separate rendering
and logging streams).
The optional `events` filter restricts delivery to the listed event types; when
omitted, JP delivers all event types.

While a subscription is active, JP pushes conversation events as they occur,
whether they originate from a concurrent `jp query` session, another plugin, or
this plugin's own `query` operation:

```json
{
  "type": "event",
  "sub_id": "chat-main",
  "data": {
    "timestamp": "...",
    "type": "chat_response",
    "message": "Here's what I found..."
  }
}
```

To stop receiving events, the plugin sends `unsubscribe`:

```json
{
  "type": "unsubscribe",
  "sub_id": "chat-main"
}
```

JP confirms removal:

```json
{
  "type": "unsubscribed",
  "sub_id": "chat-main"
}
```

JP also removes all active subscriptions when the plugin exits.

### Interactive Events

> [!NOTE]
> T-1hetcnt plans this section on its own, for turns a plugin starts with
> `query` and without subscriptions, using the `tool_approval` and `inquiry`
> shapes below.
> It adds a `decline` answer, which makes JP do what it does with nobody there.

Pushed events without a `respond` field are informational: the plugin observes
them and does not reply.
Events that require user input carry a `respond` field naming the expected
response kind.

**Tool call approval:**

```json
{
  "type": "event",
  "sub_id": "chat-main",
  "respond": "tool_approval",
  "data": {
    "type": "tool_call_request",
    "id": "tc_1",
    "name": "cargo_check",
    "arguments": {}
  }
}
```

The plugin responds:

```json
{
  "type": "respond",
  "sub_id": "chat-main",
  "respond": "tool_approval",
  "tool_call_id": "tc_1",
  "action": "approve"
}
```

Valid `action` values are `approve`, `reject`, and `modify` (which includes an
`arguments` field containing the modified tool arguments).

**Inquiry question:**

```json
{
  "type": "event",
  "sub_id": "chat-main",
  "respond": "inquiry",
  "data": {
    "type": "inquiry_request",
    "id": "inq_1",
    "question": "Which file?",
    "options": [
      "a.rs",
      "b.rs"
    ]
  }
}
```

The plugin responds:

```json
{
  "type": "respond",
  "sub_id": "chat-main",
  "respond": "inquiry",
  "inquiry_id": "inq_1",
  "answer": "a.rs"
}
```

If the plugin does not reply within a configurable timeout, JP falls back to the
default behavior configured for non-interactive mode (auto-approve, reject,
etc.).

### Query Delegation

The `query` operation runs JP's agent loop on behalf of the plugin.
The plugin sends a user message, JP executes the full turn loop (streaming, tool
calls, and retries), and events stream back to the plugin through its
subscription:

```json
{
  "type": "query",
  "conversation": "17127583920",
  "content": "What files changed?"
}
```

The `query` payload accepts two optional fields covering the remaining
`run_turn_loop` inputs:

- `attachments` (array, optional): Attachments to include in the turn,
  equivalent to `jp query -a`.
  Each entry uses the serialized attachment format (inline content with a `type`
  discriminator).
  Defaults to an empty list.
- `schema` (object, optional): A JSON Schema for structured output, equivalent
  to the `schema` field on `ChatRequest`.
  Defaults to `null` (free-form response).

A `query` request with both optional fields:

```json
{
  "type": "query",
  "conversation": "17127583920",
  "content": "Summarize this file",
  "attachments": [
    {
      "type": "file_content",
      "path": "src/main.rs",
      "content": "fn main() {}"
    }
  ],
  "schema": {
    "type": "object",
    "properties": {
      "summary": {
        "type": "string"
      }
    },
    "required": [
      "summary"
    ]
  }
}
```

JP acknowledges that the query has started:

```json
{
  "type": "query_started",
  "conversation": "17127583920"
}
```

JP then locks the conversation, starts a turn, and runs the agent loop.
During the turn, the plugin receives all intermediate events on its active
subscription: `chat_response` chunks (for streaming tokens to a client),
`tool_call_request` events (which may require approval via `respond`),
`inquiry_request` events, and `tool_call_response` results.
When the turn completes, JP pushes a `query_complete` event:

```json
{
  "type": "event",
  "sub_id": "chat-main",
  "data": {
    "type": "query_complete"
  }
}
```

This makes the plugin an alternative frontend to the same agent loop that powers
`jp query` in the terminal.

A `query` request requires an active subscription on the target conversation;
without one, JP returns an error because it cannot deliver streaming events or
interactive prompts.
The conversation must also be either unlocked or already locked by the plugin.
When the conversation is unlocked, JP acquires the lock for the duration of the
query and releases it on completion.
If another process holds the lock, JP returns an error.

### Integration with the Agent Loop

The agent loop currently lives in `jp_cli::cmd::query::turn_loop`.
Three of its four external dependencies are already trait-based:

| Dependency      | Current abstraction    | Status          |
| --------------- | ---------------------- | --------------- |
| Prompt backend  | `PromptBackend` trait  | Exists          |
| Tool execution  | `ExecutorSource` trait | Exists          |
| Inquiry backend | `InquiryBackend` trait | Exists          |
| Response output | `ChatResponseRenderer` | Concrete struct |

`TurnCoordinator` is constructed inside `run_turn_loop` and owns the concrete
`ChatResponseRenderer` directly, with no `ResponseRenderer` trait boundary.
Injecting a protocol-backed renderer therefore requires extracting response
rendering behind a trait.

For `query` delegation, JP provides protocol-backed implementations of these
traits:

- **`ProtocolPromptBackend`**: Sends a `respond: "tool_approval"` event to the
  plugin when the turn loop asks for tool approval, and blocks until the plugin
  replies with `respond`.
- **`ProtocolResponseRenderer`**: Pushes `ChatResponse` chunks from the turn
  loop as `event` messages on the plugin's subscription (using the extracted
  `ResponseRenderer` trait).
- **`ProtocolInquiryBackend`**: Sends a `respond: "inquiry"` event when a tool
  asks a question, and awaits the plugin's answer.

[RFD 026] proposes extracting the turn loop into a standalone `jp_agent` crate
with explicit trait boundaries, including `ResponseRenderer`.
If [RFD 026] lands before Phase 3, the protocol bridges plug into
`jp_agent::run_turn_loop()` directly; otherwise, Phase 3 extracts
`ResponseRenderer` from `TurnCoordinator` in `jp_cli` first.

### Web Chat Example

A web server plugin (`jp-serve`) implements browser-based chat as follows:

1. When a browser opens a conversation, `subscribe` to it with a unique `sub_id`
   per browser session.
2. When the user sends a message, issue a `query` with the message content.
3. Receive streaming `chat_response` events via the subscription and forward
   them to the browser over SSE or WebSocket.
4. Receive `tool_call_request` events with `respond: "tool_approval"`, present
   an approval UI in the browser, and send the user's decision back via
   `respond`.
5. Receive `query_complete` and signal the browser that the turn is done.
6. Send `unsubscribe` when the browser disconnects.

JP handles LLM calls, conversation locks, and event persistence; the plugin is
purely a presentation layer.

## Drawbacks

- **Bidirectional complexity**: Subscriptions turn a request/response protocol
  into a bidirectional event stream.
  Plugins must handle unsolicited pushed events arriving at any time,
  interleaved with responses to their own requests.
  Multi-threaded plugins handle this naturally, whereas single-threaded plugins
  (such as shell scripts) cannot easily participate in subscriptions.

- **Timeout semantics**: Interactive events require the plugin to respond within
  a timeout, and the right default timeout and fallback behavior depend on the
  use case (a web UI needs a long timeout while a human decides, whereas a batch
  script may want to auto-approve immediately).

- **Agent loop coupling**: The `query` operation exposes JP's internal agent
  loop behavior as a protocol surface: changes to tool-call handling, retries,
  or interrupts become visible to plugins.
  The trait boundaries in [RFD 026] help, but the protocol-level contract (which
  event types arrive and in what order) remains an additional compatibility
  surface.

## Alternatives

### Plugin calls the LLM directly

The plugin could bypass JP's agent loop entirely: call the LLM API itself,
manage tool execution, and use JP only for conversation storage via
`push_events`.

Rejected because:

- It duplicates the agent loop (streaming, retries, tool coordination, and
  inquiry handling) in every chat-capable plugin.
- It requires the plugin to hold LLM API keys and provider configuration instead
  of keeping credential management in JP.
- It requires access to JP's tool definitions and MCP servers, which the current
  protocol does not expose.

### Polling instead of subscriptions

The plugin could repeatedly call `read_events` to check for new events.

Polling is rejected as the primary mechanism because it adds latency and wastes
work during token streaming.
Simple plugins that do not need real-time updates can still poll `read_events`
as a fallback.

## Non-Goals

- **Multi-conversation queries**: A single `query` operates on one conversation.
  Orchestrating parallel queries across conversations is the plugin's
  responsibility.
- **Plugin-side tool execution**: JP executes tools, not the plugin.
  The plugin can approve, reject, or modify tool calls, but cannot provide its
  own tool implementations through this protocol (Wasm plugins via [RFD 016]
  serve that purpose).
- **Streaming protocol optimization**: This protocol keeps the JSON-lines format
  for consistency with [RFD 072].
  A binary framing format (msgpack, protobuf) is deferred unless serialization
  latency becomes measurable.

## Risks and Open Questions

- **Cross-process event delivery**: Subscriptions deliver events from any
  source.
  Delivering events from the same plugin's `query` operation is direct because
  JP controls the turn loop in-process.
  Delivering events from a concurrent `jp query` session requires change
  detection on the conversation store, which currently uses append-only files
  with `flock` coordination and no change notifications.
  Polling the events file periodically is the simplest initial mechanism;
  `inotify`/`kqueue` notifications can follow as an optimization.

- **Interactive event ordering**: When multiple tool calls arrive in a batch,
  the plugin receives multiple `respond: "tool_approval"` events.
  Rather than requiring in-order replies, JP can accept `respond` messages in
  any order and match them by `tool_call_id`.

- **Query cancellation**: The protocol does not yet define a message to cancel a
  running `query` (for example, when the user navigates away in the browser).
  A `cancel_query` message type is a natural addition once the need is
  validated.

- **Backpressure**: If the LLM streams tokens faster than the plugin consumes
  them (for example, over a slow browser connection), pushed events queue in
  JP's pipe buffer.
  This is unlikely at human interaction speeds, but high-throughput scenarios
  may need flow control.

## Implementation Plan

### Phase 1: Subscriptions

- Add `subscribe` and `unsubscribe` message types to `jp_plugin`.
- Implement event pushing in JP's dispatcher: push same-process events (such as
  `push_events` from the same plugin) to active subscriptions immediately, and
  poll the events file for cross-process writes.
- Test with a plugin that watches a conversation while `jp query` writes to it.
- Depends on [RFD 072] Phase 1 (protocol core).

### Phase 2: Interactive Events

- Add the `respond` field to pushed events and the `respond` message type.
- Implement timeout and fallback behavior.
- Test with a shell script that auto-approves tool calls.
- Depends on Phase 1.

### Phase 3: Query Delegation

- If [RFD 026] has not landed, extract `TurnCoordinator`'s rendering into a
  `ResponseRenderer` trait so protocol-backed and terminal-backed renderers can
  be swapped.
- Implement `query`, `query_started`, and `query_complete` message types,
  including `attachments` and `schema` on `query`.
- Build protocol-backed implementations of `PromptBackend`, `ResponseRenderer`,
  and `InquiryBackend`, and bridge them into `run_turn_loop` (via `jp_agent` if
  [RFD 026] has landed, or `jp_cli` internals otherwise).
- Test with `jp-serve` serving a chat interface.
- Depends on Phases 1 and 2, and [RFD 072] Phase 4 (write operations).

## References

- [RFD 072: Command Plugin System][RFD 072]
- [RFD 016: Wasm Plugin Architecture][RFD 016]
- [RFD 026: Agent Loop Extraction][RFD 026]
- [RFD 027: Client-Server Query Architecture][RFD 027]

[RFD 016]: ../016-wasm-plugin-architecture.md
[RFD 026]: ../026-agent-loop-extraction.md
[RFD 027]: ../027-client-server-query-architecture.md
[RFD 072]: ../072-command-plugin-system.md
