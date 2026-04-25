# RFD D18: Plugin Event Subscriptions and Query Delegation

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Extends**: RFD 072
- **Date**: 2026-04-06

## Summary

This RFD extends the command plugin protocol ([RFD 072]) with event
subscriptions and agent loop delegation. Plugins can subscribe to live
conversation events, respond to interactive prompts (tool approval, inquiries),
and trigger LLM queries that JP executes on their behalf. Together, these
capabilities enable plugins to act as alternative frontends to JP's agent
loop — the key building block for a web-based chat interface.

## Motivation

[RFD 072] defines a request/response protocol that covers workspace queries
and mutations. A plugin can list conversations, read events, lock, write, and
produce formatted output. This is sufficient for read-only tools (web viewer,
exporters) and simple write tools (importers, bulk editors).

It is not sufficient for a chat interface. A chat plugin needs to:

1. **Stream LLM responses in real time** — tokens must arrive at the plugin as
   they are generated, not after the full response is complete.
2. **Handle tool approval prompts** — when the agent loop wants to run a tool,
   the plugin must present the approval UI and send the decision back.
3. **Handle inquiry questions** — tools may ask questions that need user input.
4. **Observe concurrent activity** — if another `jp query` session writes to
   the same conversation, the plugin should see those events too.

All four require JP to *push* messages to the plugin, which the request/response
model from [RFD 072] does not support. This RFD adds that capability.

## Design

### Subscriptions

A plugin subscribes to a conversation to receive events as they occur.

**Subscribe:**

```json
{"type": "subscribe", "sub_id": "chat-main", "conversation": "17127583920", "events": ["chat_response", "tool_call_request", "tool_call_response"]}
```

Response:

```json
{"type": "subscribed", "sub_id": "chat-main"}
```

The `sub_id` is chosen by the plugin and must be unique among its active
subscriptions. JP echoes it on every pushed event, allowing the plugin to
maintain multiple independent subscriptions — e.g., two chat windows
side-by-side, or one subscription for rendering and another for logging.

The `events` filter is optional. When omitted, all event types are delivered.

Once subscribed, JP pushes events as they occur on the conversation (from any
source — a concurrent `jp query` session, another plugin, or this plugin's own
`query` operation):

```json
{"type": "event", "sub_id": "chat-main", "data": {"timestamp": "...", "type": "chat_response", "message": "Here's what I found..."}}
```

**Unsubscribe:**

```json
{"type": "unsubscribe", "sub_id": "chat-main"}
```

Response:

```json
{"type": "unsubscribed", "sub_id": "chat-main"}
```

Subscriptions are automatically removed when the plugin exits.

### Interactive Events

Some pushed events require a response from the plugin. These carry a `respond`
field indicating what kind of response JP expects.

**Tool call approval:**

```json
{"type": "event", "sub_id": "chat-main", "respond": "tool_approval", "data": {"type": "tool_call_request", "id": "tc_1", "name": "cargo_check", "arguments": {}}}
```

The plugin responds:

```json
{"type": "respond", "sub_id": "chat-main", "respond": "tool_approval", "tool_call_id": "tc_1", "action": "approve"}
```

Valid actions: `approve`, `reject`, `modify` (with an `arguments` field
containing the modified arguments).

**Inquiry question:**

```json
{"type": "event", "sub_id": "chat-main", "respond": "inquiry", "data": {"type": "inquiry_request", "id": "inq_1", "question": "Which file?", "options": ["a.rs", "b.rs"]}}
```

The plugin responds:

```json
{"type": "respond", "sub_id": "chat-main", "respond": "inquiry", "inquiry_id": "inq_1", "answer": "a.rs"}
```

If the plugin does not respond within a configurable timeout, JP falls back to
the default behavior configured for non-interactive mode (auto-approve, reject,
etc.).

Events without a `respond` field are informational — the plugin observes them
but does not need to reply.

### Query (Agent Loop Delegation)

The `query` operation triggers JP's agent loop on behalf of the plugin. The
plugin sends a user message, JP runs the full turn loop (streaming, tool calls,
retries), and events flow back to the plugin via its subscription.

```json
{"type": "query", "conversation": "17127583920", "content": "What files changed?"}
```

The `query` payload supports optional fields for the full range of
`run_turn_loop` inputs:

- `attachments` (array, optional): Attachments to include in the turn,
  equivalent to `jp query -a`. Each entry follows the attachment
  serialization format (inline content with a `type` discriminator).
- `schema` (object, optional): A JSON Schema constraining the assistant's
  response format, triggering structured output. Equivalent to the `schema`
  field on `ChatRequest`.

Example with both:

```json
{"type": "query", "conversation": "17127583920", "content": "Summarize this file", "attachments": [{"type": "file_content", "path": "src/main.rs", "content": "fn main() {}"}], "schema": {"type": "object", "properties": {"summary": {"type": "string"}}, "required": ["summary"]}}
```

When omitted, `attachments` defaults to an empty list and `schema` defaults
to `null` (free-form response).

Response (acknowledges the query has started):

```json
{"type": "query_started", "conversation": "17127583920"}
```

JP locks the conversation, starts a turn, and runs the agent loop. Events
stream to the plugin via its active subscription on that conversation. When the
turn completes:

```json
{"type": "event", "sub_id": "chat-main", "data": {"type": "query_complete"}}
```

During the turn, the plugin receives all intermediate events: `chat_response`
chunks (for streaming tokens to a browser), `tool_call_request` events (which
may require approval via `respond`), `inquiry_request` events, and
`tool_call_response` results. The plugin is effectively an alternative frontend
to the same agent loop that powers `jp query` in the terminal.

The conversation must either be already locked by the plugin or unlocked (in
which case JP acquires the lock for the duration of the query and releases it
on completion). If another process holds the lock, the request returns an error.

If the plugin sends `query` without an active subscription on that
conversation, JP returns an error — there would be no way to deliver the
streaming events or interactive prompts.

### Integration with the Agent Loop

The agent loop is currently implemented in `jp_cli::cmd::query::turn_loop`.
Some of its external dependencies are trait-based today:

| Dependency      | Current abstraction      | Status           |
|-----------------|--------------------------|------------------|
| Prompt backend  | `PromptBackend` trait    | Exists           |
| Tool execution  | `ExecutorSource` trait   | Exists           |
| Inquiry backend | `InquiryBackend` trait   | Exists           |
| Response output | `ChatResponseRenderer`   | Concrete struct  |

`ChatResponseRenderer` is a concrete struct instantiated inside
`TurnCoordinator::new()`. There is no `ResponseRenderer` trait in the current
codebase — `TurnCoordinator` owns the renderer directly and is itself created
inside `run_turn_loop`. This means a protocol-backed renderer cannot be
injected without first extracting rendering behind a trait.

For `query` delegation, JP provides protocol-backed implementations of the
required traits:

- **`ProtocolPromptBackend`**: When the turn loop asks for tool approval, this
  implementation sends a `respond: "tool_approval"` event to the plugin and
  blocks until the plugin sends `respond` back.
- **`ProtocolResponseRenderer`**: When the turn loop emits `ChatResponse`
  chunks, this implementation pushes them as `event` messages on the plugin's
  subscription. Requires the `ResponseRenderer` trait extraction described
  above.
- **`ProtocolInquiryBackend`**: When a tool asks a question, this
  implementation sends a `respond: "inquiry"` event and awaits the answer.

[RFD 026] proposes extracting the turn loop into a standalone `jp_agent` crate
with explicit trait boundaries, which would introduce a `ResponseRenderer`
trait as part of that work. If RFD 026 has landed by the time Phase 3 begins,
the protocol bridges plug into `jp_agent::run_turn_loop()` directly. If not,
Phase 3 of this RFD must extract `TurnCoordinator`'s rendering into a trait
as a prerequisite step before the protocol-backed implementations can be
wired in.

### Web Chat Example

A web server plugin (`jp-serve`) with chat support:

1. When a browser opens a conversation, `subscribe` to it with a unique
   `sub_id` per browser session.
2. When the user sends a message, issue a `query` with the message content.
3. Receive streaming `chat_response` events via the subscription and forward
   them to the browser over SSE or WebSocket.
4. Receive `tool_call_request` events with `respond: "tool_approval"` and
   present an approval UI in the browser. Send the user's decision back via
   `respond`.
5. Receive `query_complete` and signal the browser that the turn is done.
6. `unsubscribe` when the browser disconnects.

The plugin never calls the LLM directly, manages conversation locks, or
persists events — JP handles all of that. The plugin is purely a presentation
layer.

## Drawbacks

- **Bidirectional complexity**: The protocol shifts from pure request/response
  to a bidirectional event stream. Plugins must handle unsolicited messages
  (pushed events) arriving at any time, interleaved with responses to their
  own requests. Multi-threaded plugins handle this naturally; single-threaded
  plugins (shell scripts) cannot easily participate in subscriptions.

- **Timeout semantics**: Interactive events require the plugin to respond
  within a timeout. The right default timeout and fallback behavior may vary
  by use case (a web UI might want a long timeout to let the human think; a
  batch script might want to auto-approve immediately).

- **Agent loop coupling**: The `query` operation exposes JP's internal agent
  loop behavior as a protocol surface. Changes to how the turn loop handles
  tool calls, retries, or interrupts become visible to plugins. The trait
  boundaries from [RFD 026] help, but the protocol-level contract (which
  event types arrive and in what order) is an additional compatibility
  surface.

## Alternatives

### Plugin calls the LLM directly

The plugin could bypass JP's agent loop entirely: call the LLM API itself,
manage tool execution, and use JP only for conversation storage via
`push_events`.

Rejected because:

- Duplicates the agent loop (streaming, retries, tool coordination, inquiry
  handling) in every chat-capable plugin.
- The plugin would need LLM API keys and provider configuration, breaking
  the principle that JP manages credentials.
- Tool execution requires access to JP's tool definitions and MCP servers,
  which are not exposed through the current protocol.

### Polling instead of subscriptions

The plugin could repeatedly call `read_events` to check for new events.

Rejected as the primary mechanism because polling adds latency and wastes
resources for streaming use cases. However, polling remains available as a
fallback for simple plugins that don't need real-time updates.

## Non-Goals

- **Multi-conversation queries**: A single `query` operates on one
  conversation. Orchestrating parallel queries across conversations is the
  plugin's responsibility.
- **Plugin-side tool execution**: Tools are executed by JP, not by the plugin.
  The plugin can approve, reject, or modify tool calls, but cannot provide
  its own tool implementations through this protocol. (Wasm plugins via
  [RFD 016] serve that purpose.)
- **Streaming protocol optimization**: The JSON-lines format is kept for
  consistency with [RFD 072]. A binary framing format (msgpack, protobuf) is
  future work if latency becomes measurable.

## Risks and Open Questions

- **Cross-process event delivery**: Subscriptions deliver events from any
  source. For events generated by the same plugin's `query` operation,
  delivery is straightforward (JP controls the turn loop). For events from a
  concurrent `jp query` session writing to the same conversation, JP needs a
  notification mechanism. The current storage layer uses append-only files
  with flock coordination but has no change notification. Polling the events
  file periodically is the simplest approach; inotify/kqueue-based
  notification is an optimization for later.

- **Interactive event ordering**: When multiple tool calls arrive in a batch,
  the plugin receives multiple `respond: "tool_approval"` events. The
  protocol does not currently define whether the plugin must respond in order
  or can respond out of order. The simplest rule: responses can arrive in any
  order, matched by `tool_call_id`.

- **Query cancellation**: If the plugin wants to cancel a running `query`
  (e.g., the user navigated away in the browser), there is no cancellation
  message defined. A `cancel_query` message type would be a natural addition
  but is deferred until the need is validated.

- **Backpressure**: If the LLM streams tokens faster than the plugin can
  consume them (e.g., the browser connection is slow), pushed events queue up
  in JP's pipe buffer. For human-speed interactions this is unlikely to be a
  problem. For high-throughput scenarios, a flow control mechanism may be
  needed.

## Implementation Plan

### Phase 1: Subscriptions

- Add `subscribe`, `unsubscribe` message types to `jp_plugin`.
- Implement the event push mechanism in JP's dispatcher.
- For events from the same process (e.g., `push_events` by the same plugin),
  push to active subscriptions immediately.
- For cross-process events, implement file-based polling as the initial
  notification mechanism.
- Test with a plugin that watches a conversation while `jp query` writes to
  it.
- Depends on [RFD 072] Phase 1 (protocol core).

### Phase 2: Interactive events

- Add `respond` field to pushed events and `respond` message type.
- Implement timeout and fallback behavior.
- Test with a shell script that auto-approves tool calls.
- Depends on Phase 1.

### Phase 3: Query delegation

- If [RFD 026] has not landed: extract `TurnCoordinator`'s rendering into a
  `ResponseRenderer` trait so protocol-backed and terminal-backed renderers
  can be swapped.
- Implement `query`, `query_started`, `query_complete` message types.
- Build protocol-backed implementations of `PromptBackend`,
  `ResponseRenderer`, and `InquiryBackend`.
- Support `attachments` and `schema` fields in the `query` payload.
- Bridge into `run_turn_loop` (via `jp_agent` if [RFD 026] has landed,
  otherwise via `jp_cli` internals with the trait extraction above).
- Test with `jp-serve` serving a chat interface.
- Depends on Phases 1 and 2, and [RFD 072] Phase 4 (write operations).

## References

- [RFD 072: Command Plugin System][RFD 072]
- [RFD 016: Wasm Plugin Architecture][RFD 016]
- [RFD 026: Agent Loop Extraction][RFD 026]
- [RFD 027: Client-Server Query Architecture][RFD 027]

[RFD 072]: 072-command-plugin-system.md
[RFD 016]: 016-wasm-plugin-architecture.md
[RFD 026]: 026-agent-loop-extraction.md
[RFD 027]: 027-client-server-query-architecture.md
