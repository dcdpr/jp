# RFD 011: System Message Queue

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-02-23

## Summary

This RFD introduces a system message queue that allows JP's internal subsystems
to deliver asynchronous notifications to the assistant. Notifications are
collected in a queue and delivered at the next available communication
opportunity — piggybacking on existing messages rather than fabricating new
events. This solves the "stateful tool finished but nobody asked" problem from
[RFD 009](009-stateful-tool-protocol.md) and provides a general-purpose channel
for any subsystem that needs to inform the assistant of out-of-band events.

## Motivation

Several JP subsystems produce events that the assistant should know about but
that don't fit neatly into the request/response conversation model:

- **Stateful tool handles** (RFD 009): A background `cargo check` finishes
  while the assistant is doing other work. The assistant should know the result
  is ready, but the event model requires `ToolCallRequest → ToolCallResponse`
  pairs — JP can't inject a response without a request.
- **MCP server events**: A server disconnects or becomes unresponsive. The
  assistant might be relying on tools from that server.
- **Workspace changes**: A file the assistant is working with is modified
  externally (e.g., by the user in their editor).
- **Configuration changes**: A mid-conversation config reload affects available
  tools or model parameters.
- **Resource limits**: Token budget approaching exhaustion, rate limit warnings.

Today, none of these have a delivery path. Some are logged (and thus invisible
to the assistant), others are silently dropped. RFD 009 recommended
assistant-driven polling for stateful tools, which works but is fragile — the
assistant must remember to check.

The system message queue provides a delivery mechanism without bending the
conversation event model. Notifications piggyback on messages that JP is
already sending, so no fabricated events are needed.

## Design

### Queue and notifications

JP maintains a single, ordered notification queue. Any subsystem can enqueue
a notification:

```rust
pub struct SystemNotification {
    /// Unique identifier for deduplication and filtering.
    /// Format: "subsystem.event_type", e.g. "handle.stopped", "mcp.disconnected"
    pub kind: String,

    /// Human-readable message for the assistant.
    pub message: String,

    /// Optional structured data for programmatic consumption.
    pub data: Option<Value>,

    /// When this notification was created.
    pub timestamp: DateTime<Utc>,
}
```

The queue is append-only between delivery points. When notifications are
delivered, the queue is drained.

### Delivery points

Notifications are delivered at three points — moments where JP is already
composing a message to the assistant:

1. **Tool call responses.** When JP sends `ToolCallResponse`(s) back to the
   assistant after a tool execution cycle, any queued notifications are
   prepended to the first response's content.

2. **User-initiated in-turn messages.** When the user interrupts the stream
   (Ctrl+C) and chooses to reply, the queued notifications are prepended to
   the user's `ChatRequest`.

3. **Turn boundaries.** When a turn ends and a new turn begins (the user sends
   a new query), any remaining notifications from the previous turn are
   prepended to the new `ChatRequest`.

At each point, JP checks the queue. If it's empty, nothing happens. If there
are notifications, they are formatted into a clearly denoted block and
prepended to the message.

### Formatting

Notifications are formatted as a markdown block that is visually distinct from
the actual message content:

```markdown
---
**system notifications**

These are automated system messages from JP, unrelated to the response
which follows below. They are delivered in this message to make you aware
of them. You can ignore irrelevant notifications — they will NOT be
delivered again.

- Tool `cargo_check` (handle `h_3`) has stopped with result available.
- Tool `git` (handle `h_1`) is waiting for input.
- MCP server `github` has disconnected.
---
```

The block is prepended to the message content, followed by the actual content
(tool result, user query, etc.). The assistant sees both in a single message.

### Notification lifecycle

1. **Enqueue**: A subsystem calls `queue.push(notification)`.
2. **Accumulate**: Notifications sit in the queue until a delivery point.
3. **Deliver**: At a delivery point, all queued notifications are drained,
   formatted, and prepended to the outgoing message.
4. **Gone**: Delivered notifications are not re-delivered. If the assistant
   ignores them, they are lost.

Notifications are fire-and-forget. The queue does not track acknowledgment.
If a notification is critical (e.g., a tool waiting for input), the subsystem
can re-enqueue it periodically, but the default is single delivery.

### Producers

Any JP subsystem can produce notifications. Initial producers:

| Producer | Notification kind | When |
|----------|------------------|------|
| Handle registry | `handle.stopped` | A stateful tool reaches `Stopped` without being polled |
| Handle registry | `handle.waiting` | A stateful tool enters `Waiting` without being polled |
| MCP client | `mcp.disconnected` | An MCP server connection drops |
| MCP client | `mcp.reconnected` | An MCP server reconnects |

Future producers might include: workspace file watcher, token budget tracker,
configuration reload system.

### Configuration

Notification delivery is configurable at two levels:

#### Per-tool notification control

Tools can declare which notification kinds they emit, and users can configure
which are delivered:

```toml
[conversation.tools.cargo_check]
source = "builtin"
stateful = true

# Control which notifications this tool can deliver.
# Default: all notifications enabled.
[conversation.tools.cargo_check.notifications]
stopped = true    # notify when the tool stops
waiting = false   # don't notify for waiting state (not applicable anyway)
```

#### Global notification control

Users can disable notifications entirely or filter by kind:

```toml
[conversation.notifications]
enable = true     # master switch (default: true)

# Fine-grained control by notification kind.
[conversation.notifications.kinds]
"handle.stopped" = true
"handle.waiting" = true
"mcp.disconnected" = true
"mcp.reconnected" = false   # don't bother the assistant with reconnects
```

### Interaction with the event stream

Notifications are **not** persisted as separate events in the
`ConversationStream`. They are embedded in the content of existing events
(`ToolCallResponse` content, `ChatRequest` content). This means:

- The conversation history naturally contains the notifications as part of the
  messages where they were delivered.
- No new `EventKind` variant is needed.
- Replay and conversation forking work without special handling.

The trade-off: notifications are not independently queryable from the event
stream. If we later need to search for "all notifications in this conversation,"
we'd need to parse them from message content. This is acceptable for the
initial design.

## Drawbacks

**Noisy context.** Notifications consume tokens in the assistant's context
window. If many notifications accumulate, the prepended block can be large.
A cap on the number of notifications per delivery (e.g., max 10, oldest
dropped) would mitigate this.

**No guaranteed delivery.** If the turn ends without hitting any delivery
point (e.g., the assistant responds with no tool calls and the user doesn't
interrupt), notifications from that turn are carried to the next turn's first
message. In the worst case, a notification sits in the queue until the next
user query.

**Formatting fragility.** The markdown block format depends on the assistant
recognizing and correctly interpreting it. Different models may handle the
"ignore irrelevant notifications" instruction differently. The format should
be validated with multiple providers.

**Hidden in content.** Embedding notifications in message content means they
are mixed with actual tool results or user queries. The separator (`---`) helps,
but a model that doesn't handle markdown well might be confused.

## Alternatives

### New event type for notifications

Add a `SystemNotification` variant to `EventKind` and a corresponding event
in the conversation stream.

**Rejected because:** LLM providers expect a strict alternation of user/
assistant/tool messages. Injecting a new event type would require every
provider implementation to handle it — either by filtering it out or by
mapping it to an existing role. Embedding in existing messages avoids this.

### Deliver notifications as synthetic tool calls

Fabricate `ToolCallRequest`/`ToolCallResponse` pairs for notifications.

**Rejected because:** It violates the event model (the assistant didn't
request these tool calls) and would confuse providers that validate tool
call ID matching. It also pollutes the conversation history with fake tool
calls.

### Push notifications via streaming

Inject notification events into the LLM's response stream, interrupting the
assistant's output.

**Rejected because:** This would require pausing the stream, injecting content,
and resuming — complex and likely to cause rendering artifacts. The assistant
also can't act on a notification mid-stream (it's still generating its
response).

## Non-Goals

- **Notification acknowledgment.** The queue is fire-and-forget. No mechanism
  for the assistant to acknowledge or dismiss notifications.
- **Priority or urgency levels.** All notifications are treated equally.
  Subsystems that need urgent delivery should re-enqueue periodically.
- **User-facing notification display.** This RFD covers delivery to the
  assistant, not to the user's terminal. User-facing notifications (e.g.,
  "MCP server disconnected" shown in the terminal) are a separate concern.
- **Notification history or search.** Notifications are embedded in message
  content and not independently indexed.

## Risks and Open Questions

### Delivery latency

If the assistant spawns a background tool and then produces a long response
with no tool calls, the notification has no delivery point until the turn
ends. The assistant won't know the tool finished until the user sends the
next message. Is this acceptable, or do we need a mechanism to interrupt the
assistant?

### Token budget impact

Each notification adds ~20-50 tokens to the message. With 10 notifications,
that's 200-500 tokens of overhead. For conversations near the context window
limit, this could push out useful context. Should notifications be subject to
a token budget?

### Format standardization

The markdown block format is ad-hoc. Should we define a structured format
(e.g., JSON in a code fence) that the assistant can parse reliably? This
trades readability for parseability. Worth experimenting with different models
to see what works best.

### Queue ordering

Should notifications be delivered in chronological order (oldest first) or
reverse (newest first)? Chronological is more natural, but the assistant reads
top-to-bottom and the most recent notification is likely the most relevant.

### Interaction with conversation compaction

If JP ever implements conversation compaction (summarizing old messages to
save tokens), notifications embedded in old messages would be lost in the
summary. This is probably fine — old notifications are stale by definition —
but worth noting.

## Implementation Plan

### Phase 1: Queue infrastructure

1. Define `SystemNotification` struct in a shared crate (likely `jp_tool` or
   a new `jp_notify` crate).
2. Implement the notification queue (thread-safe, append-only between drains).
3. Implement the formatting function (notifications → markdown block).
4. Unit tests for queue operations and formatting.

Can be merged independently. No behavioral changes.

### Phase 2: Delivery point integration

1. Wire the queue into tool call response delivery — prepend notifications
   to `ToolCallResponse` content.
2. Wire the queue into `ChatRequest` construction — prepend notifications
   when the user sends a new message or interrupts.
3. Integration tests verifying delivery at each point.

Depends on Phase 1.

### Phase 3: Handle registry producer

1. The handle registry (from RFD 009) enqueues `handle.stopped` and
   `handle.waiting` notifications when a handle changes state without being
   polled.
2. Integration tests with a stateful tool that finishes in the background.

Depends on Phase 2 and RFD 009 Phase 4.

### Phase 4: Configuration

1. Add `conversation.notifications` config section.
2. Add per-tool `notifications` config.
3. Wire configuration into the queue's delivery logic (filter by kind, respect
   enable flag).

Depends on Phase 3. Can be iterated independently.

### Phase 5: Additional producers

Add MCP event producers, and any other subsystems that benefit from
notifications. Each producer is independent and can be added incrementally.

## References

- [RFD 009: Stateful Tool Protocol](009-stateful-tool-protocol.md) — defines
  stateful tool handles and identifies the proactive delivery problem that
  this RFD solves.
- [RFD 010: PTY Infrastructure and Interactive Tool SDK](010-pty-infrastructure-and-interactive-tool-sdk.md) —
  interactive tools that benefit from notifications when sessions finish.
- [Query Stream Pipeline](../architecture/query-stream-pipeline.md) — the turn
  loop and delivery points where notifications are injected.
- [RFD 028: Structured Inquiry System](028-structured-inquiry-system-for-tool-questions.md)
  — the inquiry system, a potential future notification producer.
