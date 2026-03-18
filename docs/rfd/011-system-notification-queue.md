# RFD 011: System Notification Queue

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-02-23

## Summary

This RFD introduces a system notification queue that allows JP's internal
subsystems to deliver asynchronous notifications to the assistant. Notifications
are recorded in the conversation event stream and delivered at the next
available communication opportunity — piggybacking on existing messages rather
than fabricating new events. This solves the "stateful tool finished but nobody
asked" problem from [RFD 009] and provides a general-purpose channel for any
subsystem that needs to inform the assistant of out-of-band events.

## Motivation

Several JP subsystems produce events that the assistant should know about but
that don't fit neatly into the request/response conversation model:

- **Stateful tool handles** ([RFD 009]): A background `cargo check` finishes
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
to the assistant), others are silently dropped. [RFD 009] recommended
assistant-driven polling for stateful tools, which works but is fragile — the
assistant must remember to check.

The system notification queue provides a delivery mechanism without bending the
conversation event model. Notifications piggyback on messages that JP is already
sending, so no fabricated events are needed.

## Design

### Notifications

A notification is a small, human-readable message from a JP subsystem:

```rust
/// A system notification from a JP subsystem.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemNotification {
    /// The kind of notification, identifying the source subsystem and
    /// event type. Used for configuration filtering.
    pub kind: NotificationKind,

    /// The severity level of the notification.
    #[serde(default, skip_serializing_if = "NotificationLevel::is_info")]
    pub level: NotificationLevel,

    /// Human-readable message for the assistant.
    pub message: String,
}

/// Identifies the source and type of a system notification.
///
/// The `Display` implementation produces the dotted form `"{source}.{name}"`
/// (e.g. `"tool.stopped"`). `FromStr` parses it back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationKind {
    /// The subsystem that produced this notification.
    /// e.g. "tool", "mcp"
    pub source: String,

    /// The event type within that subsystem.
    /// e.g. "stopped", "disconnected"
    pub name: String,
}

/// The severity level of a system notification.
///
/// Levels serve two purposes: they help the LLM prioritize which
/// notifications to act on, and they control delivery urgency —
/// `Critical` notifications trigger forced delivery rather than waiting
/// for the next natural contact point.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationLevel {
    /// Informational. No action expected from the assistant.
    #[default]
    Info,

    /// Something may need attention soon.
    Warning,

    /// Something failed or needs attention.
    Error,

    /// Immediate attention required. Triggers forced delivery.
    Critical,
}
```

Notifications are intentionally minimal. The `kind` field drives configuration
filtering (see [Configuration](#configuration)). The `level` field controls
delivery urgency and helps the LLM prioritize. The `message` field is what the
assistant sees. No unique ID, timestamp, or structured data — the event envelope
provides the timestamp, and there is no current need for the rest. These can be
added later if a concrete use case arises.

Both `source` and `name` are free-form strings rather than enums. The set of
producers will grow over time as subsystems are added, and future plugins may
produce their own notification kinds. String fields keep the type open for
extension without code changes.

### Event stream integration

Notifications are tracked through the conversation event stream using a new
event type and by embedding delivered notifications in carrier events.

#### Queueing

When a subsystem produces a notification, JP records it in the event stream:

```rust
/// A queued system notification event.
///
/// This event records that a notification has been produced by a subsystem
/// and is waiting for delivery to the assistant. The inner notification is
/// delivered by embedding it in the next carrier event (ChatRequest or
/// ToolCallResponse).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemNotificationQueued {
    /// The notification to deliver.
    #[serde(flatten)]
    pub notification: SystemNotification,
}
```

`SystemNotificationQueued` wraps `SystemNotification` so the notification type
is defined once and reused in both the queue event and the carrier events.
`#[serde(flatten)]` keeps the serialized JSON flat.

A new `EventKind` variant is added:

```rust
pub enum EventKind {
    // ... existing variants ...

    /// A system notification has been queued for delivery.
    ///
    /// This is an internal bookkeeping event. It is not provider-visible —
    /// providers never see this event directly. Instead, the notification
    /// content is embedded in the next carrier event.
    SystemNotificationQueued(SystemNotificationQueued),
}
```

This variant returns `false` from `is_provider_visible()`, so it is filtered out
before any provider's message conversion logic runs.

#### Delivery

Notifications are delivered by embedding them in carrier events. `ChatRequest`
and `ToolCallResponse` gain a `notifications` field:

```rust
pub struct ChatRequest {
    pub content: String,
    pub schema: Option<Map<String, Value>>,

    /// The source of this chat request.
    #[serde(default)]
    pub source: ChatRequestSource,

    /// System notifications delivered with this message.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notifications: Vec<SystemNotification>,
}

/// The origin of a chat request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatRequestSource {
    /// User-initiated chat request (the default).
    #[default]
    User,

    /// System-initiated chat request (e.g., critical notification delivery).
    System,
}

pub struct ToolCallResponse {
    pub id: String,
    pub result: Result<String, String>,

    /// System notifications delivered with this message.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notifications: Vec<SystemNotification>,
}
```

Embedding the full `SystemNotification` in the carrier event (rather than
referencing it by ID) means each event is self-contained. When inspecting the
stream JSON, the delivered notifications are visible right there in the carrier
event, alongside the content they were delivered with. The duplication is
minimal — notifications are short text messages.

#### Determining pending notifications

At a delivery point, JP collects all `SystemNotificationQueued` events that
appear after the last `ChatRequest` or `ToolCallResponse` in the stream. These
are the pending notifications. Delivery always drains the full set — there is no
partial delivery.

The event stream is typically under 1000 events, so a linear scan at each
delivery point is sufficient. No caching or indexing is needed.

### Delivery points

Notifications are delivered at four points — moments where JP is already
composing a message to the assistant, plus a forced delivery path for critical
notifications:

1. **Tool call responses.** When JP sends `ToolCallResponse`(s) back to the
   assistant after a tool execution cycle, any pending notifications are
   included in the first response's `notifications` field.

2. **User-initiated in-turn messages.** When the user interrupts the stream
   (Ctrl+C) and chooses to reply, pending notifications are included in the
   user's `ChatRequest`.

3. **Turn boundaries.** When a turn ends and a new turn begins (the user sends a
   new query), any remaining notifications are included in the new
   `ChatRequest`.

4. **Forced delivery (Critical).** When a `Critical` notification is queued JP
   forces immediate delivery rather than waiting for a natural contact point. If
   the assistant is mid-stream, JP aborts the stream and sends a
   system-initiated `ChatRequest` containing the critical notification(s) plus
   any other pending notifications. This avoids the scenario where a critical
   event (e.g., a tool error, a resource failure) goes undelivered until the
   user's next message. See [Forced delivery for Critical
   notifications](#forced-delivery-for-critical-notifications) for details.

At each point, JP checks for pending notifications. If there are none, the
`notifications` field remains empty and is omitted from serialization. If there
are pending notifications, they are collected into the carrier event's
`notifications` field and passed to the provider as part of the query.

### Forced delivery for Critical notifications

When a `Critical` notification is queued, JP forces delivery rather than waiting
for the next natural contact point. The mechanism depends on what JP is
currently doing:

- **Between turns (waiting for user input).** JP immediately sends a
  system-initiated `ChatRequest` with the pending notifications and a short
  context message explaining the interruption.
- **Mid-stream (assistant is generating a response).** JP aborts the current
  stream, records the partial response as a truncated `ChatResponse`, then
  immediately sends the system-initiated `ChatRequest`. This ensures the
  assistant isn't left generating a response based on stale assumptions — e.g.,
  if a tool it was relying on just failed. The abort must be distinguished from
  a transient provider error so the retry logic is not triggered.
- **During tool execution.** The notification is delivered with the tool
  responses at the next natural contact point, since tool execution already
  provides a delivery opportunity.

A system-initiated `ChatRequest` is distinguished from a user-initiated one by
its `source` field (`ChatRequestSource::System` vs `ChatRequestSource::User`).
The turn loop treats it like any other `ChatRequest`, so the assistant can
respond, call tools, or acknowledge the notification as it sees fit. The
`source` field also allows the turn loop and UI to handle system-initiated turns
distinctly if needed (e.g., visual treatment, interruptibility).

### Provider-level formatting

Providers decide how to surface notifications to the LLM. The notifications are
included in the query alongside the message content, similar to how attachments
are handled. Each provider can choose the most appropriate delivery mechanism
for its API:

- **OpenAI**: Use the `developer` role to send notifications as a separate
  message preceding the user/tool message.
- **Anthropic**: Use a system message injection or prepend to content.
- **Fallback**: Format notifications as a markdown block and prepend to the
  message content.

A `SystemNotifications` wrapper type implements `Display` to provide consistent
default formatting for providers that don't have a dedicated mechanism:

```rust
/// A collection of system notifications, with a `Display` implementation
/// that formats them as a markdown block suitable for prepending to message
/// content.
pub struct SystemNotifications<'a>(pub &'a [SystemNotification]);

impl fmt::Display for SystemNotifications<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Renders the markdown block shown below.
    }
}
```

The default markdown format groups notifications by level:

```markdown
---
**JP System Notifications**

These are automated system messages from JP, unrelated to the response which
follows below. They are delivered in this message to make you aware of them. You
can ignore irrelevant notifications — they will NOT be delivered again.

**Critical:**
- Tool `cargo_check` failed with exit code 101.

**Error:**
- MCP server `github` has disconnected.

**Info:**
- Tool `cargo_check` (handle `h_3`) has stopped with result available.
- Tool `git` (handle `h_1`) is waiting for input.
---
```

Providers that want structured access to individual notifications can iterate
over the slice directly. Providers that just need a string use `Display`.

### Notification lifecycle

1. **Enqueue**: A subsystem produces a notification. JP writes a
   `SystemNotificationQueued` event to the conversation stream.
2. **Accumulate**: The notification sits in the stream as a pending event until
   a delivery point is reached.
3. **Deliver**: At a delivery point, all pending notifications are collected
   into the carrier event's `notifications` field. The provider formats and
   delivers them alongside the message content. For `Critical` notifications,
   delivery is forced immediately rather than waiting for a natural contact
   point.
4. **Done**: The carrier event in the stream serves as proof of delivery. The
   notification will not be delivered again.

Notifications are fire-and-forget. The queue does not track acknowledgment.

### Crash recovery

Because notifications are persisted as events in the stream, they survive
crashes. If JP crashes after writing a `SystemNotificationQueued` event but
before writing the carrier event that delivers it, the notification is still
pending on restart. The next delivery point will pick it up.

### Producers

Any JP subsystem can produce notifications. Initial producers:

| Producer             | Source | Name           | Level   | When                                     |
|----------------------|--------|----------------|---------|------------------------------------------|
| Tool handle registry | `tool` | `stopped`      | Info    | A stateful tool reaches `Stopped`        |
|                      |        |                |         | without being polled                     |
| Tool handle registry | `tool` | `waiting`      | Warning | A stateful tool enters `Waiting` without |
|                      |        |                |         | being polled                             |
| Tool handle registry | `tool` | `failed`       | Error   | A stateful tool fails unexpectedly       |
| MCP client           | `mcp`  | `disconnected` | Error   | An MCP server connection drops           |
| MCP client           | `mcp`  | `reconnected`  | Info    | An MCP server reconnects                 |

Future producers might include: workspace file watcher, token budget tracker,
configuration reload system.

### Configuration

Notification delivery is configurable at two levels:

#### Per-tool notification control

Tools can declare which notification names they emit, and users can configure
which are delivered. The source is implicit — it's scoped to the tool — so only
the notification name is needed:

```toml
[conversation.tools.cargo_check]
source = "builtin"
stateful = true

# Control which notifications this tool can deliver.
# Default: all notifications enabled.
[conversation.tools.cargo_check.notifications]
stopped = true
waiting = false
failed = true
```

#### Global notification control

Users can filter notifications by source, or by source and name:

```toml
# Source-level filtering: disable all notifications from a source.
[conversation.notifications.kinds.mcp]
enable = false

# Name-level filtering within a source.
[conversation.notifications.kinds.tool]
stopped = true
waiting = true
failed = true

[conversation.notifications.kinds.mcp]
disconnected = true
reconnected = false # don't bother the assistant with reconnects
```

The nested TOML structure maps directly to the `NotificationKind` struct's
`source` and `name` fields. This enables both source-level control (disable all
`mcp` notifications) and fine-grained per-name control within a source.

Configuration filtering is applied at delivery time, not at enqueue time. A
`SystemNotificationQueued` event is always written to the stream regardless of
configuration — this ensures the stream is a complete record of what happened.
Filtered notifications simply never appear in a carrier event's `notifications`
field.

## Drawbacks

**Delivery latency for non-critical notifications.** If the turn ends without
hitting any delivery point (e.g., the assistant responds with no tool calls and
the user doesn't interrupt), non-critical notifications are carried to the next
turn's first message. In the worst case, a notification sits in the queue until
the next user query. `Critical` notifications mitigate this through forced
delivery, but `Info`/`Warning`/`Error` notifications may be delayed.

**Formatting fragility.** The default markdown block format depends on the
assistant recognizing and correctly interpreting it. Different models may handle
the "ignore irrelevant notifications" instruction differently. Provider-level
formatting mitigates this by allowing each provider to use the best available
mechanism (e.g., OpenAI's `developer` role), but the fallback format should be
validated with multiple providers.

## Alternatives

### In-memory queue only

Maintain notifications in an in-memory queue without persisting them as events.
Drain the queue at delivery points and embed the formatted text directly in
message content.

**Rejected because:** An in-memory queue loses all pending notifications on
crash. Notifications are not independently queryable or auditable. Conversation
forking requires special handling to carry over pending notifications. The
event-stream approach solves all of these naturally.

### Deliver notifications as synthetic tool calls

Fabricate `ToolCallRequest`/`ToolCallResponse` pairs for notifications.

**Rejected because:** It violates the event model (the assistant didn't request
these tool calls) and would confuse providers that validate tool call ID
matching. It also pollutes the conversation history with fake tool calls.

### Push notifications via streaming

Inject notification events into the LLM's response stream, interrupting the
assistant's output.

**Rejected because:** This would require pausing the stream, injecting content,
and resuming — complex and likely to cause rendering artifacts. The assistant
also can't act on a notification mid-stream (it's still generating its
response).

### Separate `SystemNotificationDelivered` event

Instead of embedding notifications in carrier events, emit a separate
`SystemNotificationDelivered` event that references notification IDs.

**Rejected because:** It creates a temporal coupling between two events that
must stay in sync. If JP writes the carrier event but crashes before writing the
`Delivered` event, the stream is inconsistent. Embedding in the carrier event
makes delivery atomic — one event write, one source of truth. It also removes a
layer of indirection when inspecting the stream and removes the need for
notification IDs.

## Non-Goals

- **Notification acknowledgment.** The queue is fire-and-forget. No mechanism
  for the assistant to acknowledge or dismiss notifications.
- **User-facing notification display.** This RFD covers delivery to the
  assistant, not to the user's terminal. User-facing notifications (e.g., "MCP
  server disconnected" shown in the terminal) are a separate concern.
- **Structured notification data.** Notifications carry a human-readable
  `message` only. Structured data (e.g., JSON payloads) can be added later if a
  concrete use case arises.

## Risks and Open Questions

### Token budget impact

Each notification adds ~20-50 tokens to the message. With 10 notifications,
that's 200-500 tokens of overhead. For conversations near the context window
limit, this could push out useful context. Should notifications be subject to a
token budget?

### Provider formatting validation

Different providers have different mechanisms for delivering system-level
messages. The `developer` role (OpenAI), system message injection (Anthropic),
and content prepending (fallback) should be tested across models to determine
which approach produces the best assistant behavior. The `SystemNotifications`
`Display` format should also be validated for clarity across models.

### Queue ordering

Should notifications be delivered in chronological order (oldest first) or
reverse (newest first)? Chronological is more natural, but the assistant reads
top-to-bottom and the most recent notification is likely the most relevant.

### Interaction with conversation compaction

When conversation compaction ([RFD 036]) is implemented,
`SystemNotificationQueued` events and their corresponding carrier events will be
subject to compaction. Old notifications are stale by definition, so dropping
them during compaction is appropriate. The explicit event types make this
straightforward — a compaction pass can identify and handle notification events
without parsing message content.

### System-initiated turn semantics

The forced delivery mechanism for `Critical` notifications introduces the
concept of a system-initiated turn — a turn not triggered by the user
(`ChatRequestSource::System`). The `source` field makes these turns explicitly
identifiable, but questions remain: how is the system-initiated `ChatRequest`
content phrased? Should it be visually distinct in the conversation UI? Can the
user interrupt a system-initiated turn the same way as a regular turn?

## Implementation Plan

### Phase 1: Event types and formatting

1. Define `SystemNotification`, `NotificationKind`, and `NotificationLevel` in
   `jp_conversation`.
2. Define `SystemNotificationQueued` event and add the `EventKind` variant.
3. Add `notifications: Vec<SystemNotification>` to `ChatRequest` and
   `ToolCallResponse`.
4. Implement `SystemNotifications` wrapper with `Display` formatting (grouped by
   level).
5. Ensure `SystemNotificationQueued` returns `false` from
   `is_provider_visible()`.
6. Unit tests for serialization, formatting, and `is_provider_visible`.

Can be merged independently. No behavioral changes.

### Phase 2: Delivery point integration

1. Implement pending notification collection (scan stream for
   `SystemNotificationQueued` events after the last carrier event).
2. Wire into tool call response delivery — populate `notifications` on the first
   `ToolCallResponse`.
3. Wire into `ChatRequest` construction — populate `notifications` when the user
   sends a new message or interrupts.
4. Implement provider-level formatting for at least two providers (one using a
   dedicated mechanism like `developer` role, one using the `Display` fallback).
5. Integration tests verifying delivery at each point.

Depends on Phase 1. All notifications queue normally at this phase — no forced
delivery yet.

### Phase 3: Tool handle producer

1. The tool handle registry (from [RFD 009]) writes `SystemNotificationQueued`
   events when a handle changes state without being polled (`tool.stopped`,
   `tool.waiting`, `tool.failed`).
2. Integration tests with a stateful tool that finishes in the background.

Depends on Phase 2 and [RFD 009] Phase 4.

### Phase 4: Configuration

1. Add `conversation.notifications.kinds` config section with nested source/name
   structure.
2. Add per-tool `notifications` config (name-level filtering).
3. Wire configuration into the delivery logic (filter at delivery time).

Depends on Phase 3. Can be iterated independently.

### Phase 5: Critical notification forced delivery

1. Implement the forced delivery path for `Critical` notifications.
2. Handle the three contexts: between turns, mid-stream, during tool execution.
3. System-initiated `ChatRequest` construction and turn loop integration.
4. Integration tests for forced delivery in each context.

Depends on Phase 2. Can be developed in parallel with Phases 3-4.

### Phase 6: Additional producers

Add MCP event producers, and any other subsystems that benefit from
notifications. Each producer is independent and can be added incrementally.

## References

- [RFD 009: Stateful Tool Protocol][RFD 009] — defines stateful tool handles and
  identifies the proactive delivery problem that this RFD solves.
- [RFD 010: PTY Infrastructure and Interactive Tool SDK][RFD 010] — interactive
  tools that benefit from notifications when sessions finish.
- [RFD 036: Conversation Compaction][RFD 036] — compaction of conversation
  history, which interacts with notification event lifecycle.
- [Query Stream Pipeline](../architecture/query-stream-pipeline.md) — the turn
  loop and delivery points where notifications are injected.
- [RFD 028: Structured Inquiry System][RFD 028] — the inquiry system, a
  potential future notification producer.

[RFD 009]: 009-stateful-tool-protocol.md
[RFD 010]: 010-pty-infrastructure-and-interactive-tool-sdk.md
[RFD 028]: 028-structured-inquiry-system-for-tool-questions.md
[RFD 036]: 036-conversation-compaction.md
