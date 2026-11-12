# RFD 011: System Notification Queue

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-02-23
- **Required by**: [RFD 037]
- **Summary**: Delivers asynchronous subsystem notifications to the assistant by
  attaching queued messages to existing conversation events.

## Summary

This RFD introduces a system notification queue that delivers asynchronous
subsystem events to the assistant.
JP records notifications in the conversation stream and attaches them to the
next outgoing message instead of fabricating standalone events.
This gives background tools from [RFD 009] and other subsystems a general
out-of-band delivery channel.

## Motivation

Several JP subsystems produce asynchronous events that the assistant needs to
know about, but that do not fit into the request/response conversation model:

- **Stateful tool handles** ([RFD 009]): A background `cargo check` finishes
  while the assistant is doing other work.
  The assistant needs to know the result is ready, but the event model requires
  `ToolCallRequest → ToolCallResponse` pairs, so JP cannot inject a response
  without a matching request.
- **MCP server events**: A server disconnects or becomes unresponsive while the
  assistant relies on its tools.
- **Workspace changes**: A file the assistant is working with is modified
  externally, such as in the user's editor.
- **Configuration changes**: A mid-conversation config reload changes the
  available tools or model parameters.
- **Resource limits**: The token budget approaches exhaustion or a provider
  warns about rate limits.

Without a delivery path, JP either logs these events (leaving them invisible to
the assistant) or drops them.
[RFD 009] proposed assistant-driven polling for stateful tools, which works but
is fragile: the assistant must remember to poll.

The system notification queue delivers these events without altering the
request/response model by attaching notifications to messages JP is already
sending.

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

`SystemNotification` is intentionally minimal: `kind` drives [configuration
filtering](#configuration), `level` controls delivery urgency and priority, and
`message` holds the text shown to the assistant.
The struct omits a unique ID, a timestamp (which the event envelope already
provides), and structured payloads until a concrete use case requires them.

Both `source` and `name` are free-form strings rather than enums so new
subsystems and plugins can define notification kinds without changing
`NotificationKind`.

### Event stream integration

JP tracks notifications in the conversation event stream: queued notifications
are recorded as standalone events, and delivered notifications are embedded in
carrier events.

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

`EventKind` includes a `SystemNotificationQueued` variant:

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

`is_provider_visible()` returns `false` for this variant, filtering it out
before provider message conversion runs.

#### Delivery

JP delivers notifications by embedding them in the `notifications` field of a
carrier event (`ChatRequest` or `ToolCallResponse`):

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

Embedding the full `SystemNotification` rather than referencing it by ID keeps
each carrier event self-contained: delivered notifications appear directly
alongside the message content in the serialized stream JSON.
Because notifications are short text messages, the duplication is minimal.

#### Determining pending notifications

At each delivery point, JP collects all `SystemNotificationQueued` events
recorded after the most recent `ChatRequest` or `ToolCallResponse`.
Delivery drains all pending notifications at once; there is no partial delivery.
Because conversation streams typically contain under 1,000 events, a linear scan
is sufficient without caching or indexing.

### Delivery points

JP delivers notifications at three points where it is already sending a message
to the assistant, plus an immediate forced-delivery path for `Critical`
notifications:

1. **Tool call responses.** After a tool execution cycle, JP attaches pending
   notifications to the `notifications` field of the first `ToolCallResponse`.
2. **User-initiated in-turn messages.** When the user interrupts the stream
   (`Ctrl+C`) and replies, JP attaches pending notifications to the user's
   `ChatRequest`.
3. **Turn boundaries.** When the user starts a new turn with a query, JP
   attaches any remaining notifications to the new `ChatRequest`.
4. **Forced delivery (`Critical`).** Queuing a `Critical` notification triggers
   immediate delivery instead of waiting for the next message boundary, so
   urgent events (such as a tool error or resource failure) are not held until
   the user's next prompt.
   See [Forced delivery for Critical
   notifications](#forced-delivery-for-critical-notifications).

At each delivery point, JP moves any pending notifications into the carrier
event's `notifications` field and passes them to the provider with the query.
When no notifications are pending, `notifications` stays empty and is omitted
from serialization.

### Forced delivery for Critical notifications

How JP forces delivery for a `Critical` notification depends on the current turn
state:

- **Between turns (waiting for user input).** JP immediately sends a
  system-initiated `ChatRequest` containing the pending notifications and a
  short message explaining the interruption.
- **Mid-stream (assistant is generating a response).** JP aborts the active
  stream, records the partial output as a truncated `ChatResponse`, and
  immediately sends a system-initiated `ChatRequest`.
  Aborting prevents the assistant from continuing to generate a response under
  stale assumptions (for example, after a tool it depends on has failed).
  JP distinguishes this abort from a transient provider error so it does not
  trigger retry logic.
- **During tool execution.** JP delivers the notification with the upcoming
  `ToolCallResponse`, since the end of the tool cycle already provides a
  delivery point.

The `source` field distinguishes system-initiated requests
(`ChatRequestSource::System`) from user-initiated ones
(`ChatRequestSource::User`).
The turn loop processes both the same way, allowing the assistant to respond,
call tools, or acknowledge the notification.
The distinct `source` value also lets the turn loop and UI treat
system-initiated turns differently when needed, such as in visual styling or
interrupt handling.

### Provider-level formatting

JP passes notifications to the provider alongside the message content, similar
to attachments.
Each provider formats them using the mechanism best suited to its API:

- **OpenAI**: Send notifications in a separate `developer`-role message before
  the user or tool message.
- **Anthropic**: Inject a system message or prepend notifications to the message
  content.
- **Fallback**: Prepend a formatted Markdown block to the message content.

A `SystemNotifications` wrapper type implements `Display` to provide consistent
default formatting for providers without a dedicated mechanism:

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

Providers that need structured access to individual notifications can iterate
over the slice directly; providers that only need formatted text use `Display`.

### Notification lifecycle

1. **Enqueue**: A subsystem produces a notification, and JP writes a
   `SystemNotificationQueued` event to the conversation stream.
2. **Accumulate**: The notification remains pending in the stream until JP
   reaches a delivery point.
3. **Deliver**: At a delivery point, JP collects all pending notifications into
   the carrier event's `notifications` field, and the provider formats and sends
   them with the message content.
   `Critical` notifications trigger immediate forced delivery instead of waiting
   for the next message boundary.
4. **Done**: The persisted carrier event records delivery, and the notification
   is not sent again.

Notifications are fire-and-forget; the queue does not track acknowledgment.

### Crash recovery

Persisting notifications as stream events makes them crash-safe.
If JP crashes after writing `SystemNotificationQueued` but before writing the
carrier event that delivers it, the notification remains pending on restart and
is picked up at the next delivery point.

### Producers

Any JP subsystem can produce notifications.
Initial producers:

| Producer             | Source | Name           | Level   | When                                     |
| -------------------- | ------ | -------------- | ------- | ---------------------------------------- |
| Tool handle registry | `tool` | `stopped`      | Info    | A stateful tool reaches `Stopped`        |
|                      |        |                |         | without being polled                     |
| Tool handle registry | `tool` | `waiting`      | Warning | A stateful tool enters `Waiting` without |
|                      |        |                |         | being polled                             |
| Tool handle registry | `tool` | `failed`       | Error   | A stateful tool fails unexpectedly       |
| MCP client           | `mcp`  | `disconnected` | Error   | An MCP server connection drops           |
| MCP client           | `mcp`  | `reconnected`  | Info    | An MCP server reconnects                 |

Future producers include a workspace file watcher, a token budget tracker, and
the configuration reload system.

### Configuration

Users can configure notification delivery per tool or globally.

#### Per-tool notification control

Tools declare which notification names they emit, and users configure which of
those are delivered.
Because the table is already scoped to the tool, the source is implicit and only
the notification name is specified:

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

The nested TOML keys map directly to `NotificationKind`'s `source` and `name`
fields, supporting both source-wide filtering (disabling all `mcp`
notifications) and per-name filtering within a source.

JP applies configuration filters at delivery time rather than enqueue time.
Every notification is recorded as a `SystemNotificationQueued` event so the
stream preserves a complete history; filtered notifications are simply omitted
from the carrier event's `notifications` field.

## Drawbacks

**Delivery latency for non-critical notifications.** If a turn ends without
hitting a delivery point (for example, when the assistant responds without
calling tools and the user does not interrupt), `Info`, `Warning`, and `Error`
notifications wait in the queue until the user's next query.
Only `Critical` notifications bypass this delay through forced delivery.

**Formatting fragility.** The fallback Markdown block relies on the model
recognizing the block and following its instruction to ignore irrelevant
notifications.
Provider-specific mechanisms such as OpenAI's `developer` role reduce this risk,
but the fallback format still needs validation across models and providers.

## Alternatives

### In-memory queue only

Maintain notifications in an in-memory queue without persisting them as events.
Drain the queue at delivery points and embed the formatted text directly in
message content.

**Rejected because:** An in-memory queue loses pending notifications on a crash,
cannot be queried or audited in the conversation history, and requires
special-case copying when forking a conversation.
Recording notifications in the event stream handles all three automatically.

### Deliver notifications as synthetic tool calls

Fabricate `ToolCallRequest`/`ToolCallResponse` pairs for notifications.

**Rejected because:** Synthetic tool calls violate the event model because the
assistant never requested them, pollute the conversation history, and break
providers that validate matching tool call IDs.

### Push notifications via streaming

Inject notification events into the LLM's response stream, interrupting the
assistant's output.

**Rejected because:** Pausing a response stream to inject content and resume is
complex and prone to rendering artifacts.
Moreover, the assistant cannot act on a notification while it is still
generating its response.

### Separate `SystemNotificationDelivered` event

Instead of embedding notifications in carrier events, emit a separate
`SystemNotificationDelivered` event that references notification IDs.

**Rejected because:** Splitting delivery across two events introduces temporal
coupling: if JP writes the carrier event and crashes before writing
`SystemNotificationDelivered`, the stream becomes inconsistent.
Embedding notifications directly in the carrier event keeps delivery atomic in a
single write, avoids an extra level of indirection when inspecting the stream,
and eliminates the need for notification IDs.

## Non-Goals

- **Notification acknowledgment.** Delivery is fire-and-forget; the assistant
  has no mechanism to acknowledge or dismiss notifications.
- **User-facing notification display.** This RFD covers delivering notifications
  to the assistant, not rendering alerts (such as "MCP server disconnected") in
  the user's terminal.
- **Structured notification data.** Notifications carry only a human-readable
  `message`.
  Structured payloads such as JSON can be added if a concrete use case arises.

## Risks and Open Questions

### Token budget impact

Each notification adds roughly 20 to 50 tokens to a message (200 to 500 tokens
for a batch of 10), which could displace useful context when a conversation is
near the context window limit.
Should notifications be subject to a token budget?

### Provider formatting validation

OpenAI's `developer` role, Anthropic's system message injection, and the
`SystemNotifications` `Display` fallback need testing across models to determine
which approach produces the most reliable assistant behavior and clearest
formatting.

### Queue ordering

Should JP deliver notifications in chronological order (oldest first) or reverse
chronological order (newest first)?
Chronological order matches the event stream, but the assistant reads
top-to-bottom and the newest notification is usually the most relevant.

### Interaction with conversation compaction

Conversation compaction ([RFD 036]) will compact both `SystemNotificationQueued`
events and their carrier events.
Because old notifications are stale by definition, dropping them during
compaction is appropriate.
Dedicated event types and fields let a compaction pass strip notifications
without parsing message content.

> [!TIP]
> [RFD 134] places entries recorded between turns into a pending turn outside
> every compaction range.
> A notification queued between turns is not summarized into the preceding turn
> and instead becomes part of the next turn when it starts.

### System-initiated turn semantics

Forced delivery for `Critical` notifications introduces system-initiated turns
(`ChatRequestSource::System`) that are not triggered by the user.
While the `source` field identifies these turns, several design questions
remain: how should the system-initiated `ChatRequest` content be phrased, should
the conversation UI render these turns distinctly, and can the user interrupt
them the same way as a regular turn?

## Implementation Plan

### Phase 1: Event types and formatting

1. Define `SystemNotification`, `NotificationKind`, and `NotificationLevel` in
   `jp_conversation`.
2. Define `SystemNotificationQueued` and add the `EventKind` variant.
3. Add `notifications: Vec<SystemNotification>` to `ChatRequest` and
   `ToolCallResponse`.
4. Implement the `SystemNotifications` wrapper with `Display` formatting grouped
   by level.
5. Return `false` from `is_provider_visible()` for `SystemNotificationQueued`.
6. Add unit tests for serialization, formatting, and `is_provider_visible`.

This phase introduces no behavioral changes and can be merged independently.

### Phase 2: Delivery point integration

1. Implement pending notification collection by scanning the stream for
   `SystemNotificationQueued` events after the last carrier event.
2. Populate `notifications` on the first `ToolCallResponse` after a tool
   execution cycle.
3. Populate `notifications` on `ChatRequest` when the user sends a new message
   or interrupts a stream.
4. Implement provider-level formatting for at least two providers (one using a
   dedicated mechanism such as the `developer` role, and one using the `Display`
   fallback).
5. Add integration tests verifying delivery at each point.

Depends on Phase 1.
All notifications use standard queued delivery in this phase; forced delivery
lands in Phase 5.

### Phase 3: Configuration

1. Add the `conversation.notifications.kinds` config section with nested
   source/name structure.
2. Add per-tool `notifications` config for name-level filtering.
3. Apply configuration filters at delivery time.

Depends on Phase 2 and can be iterated independently.

### Phase 4 (deferred): Tool handle producer

1. Emit `SystemNotificationQueued` events (`tool.stopped`, `tool.waiting`,
   `tool.failed`) from the tool handle registry ([RFD 009]) when a handle
   changes state without being polled.
2. Add integration tests with a stateful tool that finishes in the background.

Depends on Phase 2 and [RFD 009] Phase 4.
Because only this phase requires [RFD 009], the delivery mechanism,
configuration surface, and MCP producers can land without waiting for the
stateful tool protocol.

### Phase 5: Critical notification forced delivery

1. Implement the forced delivery path for `Critical` notifications across all
   three contexts: between turns, mid-stream, and during tool execution.
2. Construct system-initiated `ChatRequest` events and integrate them into the
   turn loop.
3. Add integration tests for forced delivery in each context.

Depends on Phase 2 and can be developed in parallel with Phases 3 and 4.

### Phase 6: Additional producers

Add MCP event producers and producers for any other subsystems that benefit from
notifications.
Each producer is independent and can be added incrementally.

## References

- [RFD 009: Stateful Tool Protocol][RFD 009]: Defines stateful tool handles and
  identifies the proactive delivery problem that this RFD solves.
- [RFD 010: PTY Infrastructure and Interactive Tool SDK][RFD 010]: Defines
  interactive tools that benefit from notifications when sessions finish.
- [RFD 036: Conversation Compaction][RFD 036]: Compacts conversation history,
  which interacts with the notification event lifecycle.
- [Query Stream Pipeline]: Describes the turn loop and delivery points where
  notifications are injected.
- [RFD 028: Structured Inquiry System][RFD 028]: Defines the inquiry system, a
  potential notification producer.

[Query Stream Pipeline]: ../wiki/00dxyb6-query-stream-pipeline-architecture.md
[RFD 009]: 009-stateful-tool-protocol.md
[RFD 010]: 010-pty-infrastructure-and-interactive-tool-sdk.md
[RFD 028]: 028-structured-inquiry-system-for-tool-questions.md
[RFD 036]: 036-conversation-compaction.md
[RFD 037]: 037-await-tool-for-stateful-handle-synchronization.md
[RFD 134]: 134-pending-turn.md
