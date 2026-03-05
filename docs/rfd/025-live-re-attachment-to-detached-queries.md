# RFD 025: Live Re-Attachment to Detached Queries

- **Status**: Abandoned
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-19

> **Abandoned.** Superseded by [RFD 027: Client-Server Query Architecture](027-client-server-query-architecture.md),
> which makes live attachment the default execution model rather than an
> add-on. The socket protocol and attach flow from this RFD are carried
> forward into RFD 027.
>
> The original text below is preserved for historical context.

## Summary

This RFD introduces `jp conversation attach` for connecting a terminal to a
running detached query process. The attached client receives live streaming
output and can answer inquiries interactively. When the client disconnects, the
detached process continues with the configured detached policy.

This RFD depends on [RFD 024] (Detached Query Execution) for the `--detach`
flag, process registry, and daemonization, on [RFD 021] (Printer Live
Redirection) for runtime output swapping, and on [RFD 019] (Non-Interactive
Mode) for prompt routing.

## Motivation

[RFD 024] introduces `--detach` for running queries in the background. When a
detached process hits an inquiry, it persists the incomplete turn and exits.
The user later resumes with `--continue`. This works well when the user intends
to come back later.

But there's a gap: the user detaches a query, then wants to check on it while
it's still running. Maybe the LLM is streaming a long response. Maybe several
tool calls are executing and the user wants to see progress. Maybe the user
knows an inquiry is likely and wants to be there to answer it immediately
rather than waiting for the process to exit and resuming later.

Without live re-attachment, the user's only option is to wait for the process
to finish (or exit at an inquiry), then inspect the results via
`conversation print`. There is no way to observe or interact with a running
detached process.

## Design

### `jp conversation attach`

```bash
jp conversation attach <cid>
jp conversation attach                # session's active conversation
```

Connects to the running detached process for the specified conversation. If
no process is running:

- If the conversation has an incomplete turn (pending inquiry), the command
  suggests `--continue` instead:

  ```
  $ jp conversation attach jp-c17528832001
  Error: No running process for jp-c17528832001.
    Conversation has a pending inquiry (fs_modify_file).

      jp query --continue --id=jp-c17528832001
                              Resume the incomplete turn.
  ```

- If the conversation is idle, the command errors:

  ```
  $ jp conversation attach jp-c17528831000
  Error: No running process for jp-c17528831000.
  ```

`attach` is a read-write connection to the running process. It is not a new
query — there is no `--attach` flag on `jp query`.

### IPC via Unix Domain Sockets

Each detached process listens on a Unix domain socket:

```
~/.local/share/jp/workspace/<workspace-id>/processes/<conversation-id>.sock
```

This sits alongside the process registry entry from [RFD 024]. The socket is
created when the detached process starts and removed when it exits.

The socket path is derived from the conversation ID, so `attach` can find
it without consulting the registry entry (though it checks PID liveness from
the registry to confirm the process is actually running before connecting).

### Attach Protocol

The protocol uses newline-delimited JSON over the Unix domain socket. The
detached process is the server; the attach client connects.

#### Message types

**Server → Client:**

```rust
enum ServerMessage {
    /// Rendered output chunk. The client writes this to its terminal.
    Output { target: PrintTarget, data: String },

    /// An inquiry that needs the user's answer.
    Inquiry { inquiry: Inquiry, metadata: InquiryMetadata },

    /// The turn completed. The client can disconnect.
    TurnComplete,

    /// The process is exiting (error, shutdown).
    ProcessExiting { reason: String },
}
```

**Client → Server:**

```rust
enum ClientMessage {
    /// Answer to a pending inquiry.
    InquiryResponse { id: InquiryId, answer: Value },

    /// Client is disconnecting gracefully.
    Disconnect,
}
```

#### Connection lifecycle

1. Client connects to the socket.
2. Server detects the connection and sets `has_client = true` in its prompt
   routing state.
3. Server begins forwarding rendered output to the client via `Output`
   messages.
4. If an inquiry arrives, the server sends an `Inquiry` message instead of
   applying the detached policy. The client renders the prompt and collects
   the user's answer.
5. Client sends `InquiryResponse`. Server delivers the answer to the tool
   and continues execution.
6. When the turn completes, server sends `TurnComplete`.
7. Client can disconnect at any time by sending `Disconnect` or closing the
   socket. Server reverts to `has_client = false` and the detached policy
   resumes.

#### Single client

Only one client can be attached at a time. If a second client attempts to
connect while one is already attached, the connection is rejected:

```
$ jp conversation attach jp-c17528832001
Error: Another client is already attached to jp-c17528832001.
```

### Output Redirection on Attach

When a client attaches to a running process, the process needs to start
sending output to the client. This uses the `Printer::swap_writers()`
mechanism from [RFD 021].

#### Attach flow

1. **Replay from disk.** The client reads persisted events from the
   conversation's event stream and renders them using `conversation print`
   logic. By default, events from the current in-progress turn are shown,
   giving the user context for what's happening.

2. **Flush and swap.** The server calls `printer.flush_instant()` followed
   by `printer.swap_writers(socket_writer)`. All subsequent rendered output
   goes to the client via the socket instead of the sink.

3. **Live streaming.** From this point, the client's terminal shows the
   same output the user would see in a foreground query: LLM response
   chunks, tool call headers, progress indicators.

The gap between "events persisted to disk" and "live output from the printer"
is the content currently in the `EventBuilder` and renderer buffers that
hasn't been flushed as a complete event yet. This content is captured by
the `flush_instant()` call, which forces all buffered content through the
printer before the swap.

#### Detach flow

When the client disconnects:

1. Server calls `printer.flush_instant()` to send any pending output to the
   client.
2. Server calls `printer.swap_writers(sink)` to revert to discarding output.
3. Server sets `has_client = false`.
4. Subsequent inquiries follow the detached policy (`queue` by default).

### Context on Attach

When a client attaches mid-stream, it needs enough context to understand what's
happening. The client renders recent history before switching to live output:

| Flag | Behavior |
|---|---|
| (default) | Show events from the current in-progress turn. |
| `--tail=N` | Show the last N content events, plus the current turn. |
| `--tail=0` | Skip history, show only live output from this point. |

Content events are `ChatRequest`, `ChatResponse`, `ToolCallRequest`,
`ToolCallResponse`. Structural markers like `TurnStart` and `ConfigDelta` are
skipped when counting.

The client renders history locally from the persisted conversation file. This
avoids the server needing to replay or buffer historical output — the disk is
the source of truth for completed events.

### Relationship to Conversation Locks

The detached process holds the conversation lock ([RFD 020]) for its entire
execution. The attach client does **not** acquire a lock — it communicates with
the lock-holding process via the socket. The attach client is a viewer and
input provider, not a writer to the conversation stream.

### Relationship to Prompt Routing

The attach changes the prompt routing state dynamically:

- **Client attached**: `has_client = true`. Inquiries are sent to the client
  via the socket (`PromptAction::PromptClient`).
- **Client detaches**: `has_client = false`. Inquiries follow the detached
  policy (`queue` → persist and exit).

This means a detached query that would normally exit at an inquiry can instead
have the inquiry answered interactively if a client attaches before the
inquiry arrives. The process doesn't need to know in advance whether a client
will be available — the routing decision happens at inquiry time.

## Drawbacks

**IPC complexity.** Unix domain sockets, a message protocol, and output
redirection add meaningful infrastructure. This is the most complex piece
of the detached conversations feature set.

**Platform constraints.** Unix domain sockets are not available on Windows.
Windows support would require named pipes or a different IPC mechanism.
The initial implementation targets macOS and Linux only.

**Partial output gap.** Between the last persisted event and the current
renderer state, there may be content that hasn't been flushed as a complete
event (partial `ChatResponse` chunks in the `EventBuilder`). The
`flush_instant()` call minimizes this gap but doesn't eliminate it for
content that hasn't reached the printer yet. In practice, this is a few
tokens of LLM output at most.

**Socket cleanup on crash.** If the detached process is killed with SIGKILL,
the socket file orphans on disk. `conversation attach` checks PID liveness
before connecting, so stale sockets don't cause connection attempts to hang.
Stale socket files are cleaned up alongside stale registry entries during
`conversation ls`.

## Alternatives

### HTTP server for IPC

Use a local HTTP server instead of Unix domain sockets.

Rejected because it requires port allocation, firewall considerations, and
is heavier than needed. Unix domain sockets are the standard local IPC
mechanism on Unix with better security properties (filesystem permissions).

### Named pipe (FIFO) instead of socket

Use a named pipe for the output stream and a separate pipe for input.

Rejected because named pipes are unidirectional. The bidirectional
communication (output to client, inquiry answers from client) would require
two pipes and coordination logic. Unix domain sockets provide bidirectional
streams natively.

### No live output, just inquiry forwarding

Only forward inquiries to the attached client, not streaming output. The
client would see inquiry prompts but not the LLM's response or tool execution
progress.

Rejected because the primary value of attaching is seeing what's happening.
Inquiry forwarding alone doesn't justify the IPC infrastructure — the user
could just wait for the process to exit and use `--continue`.

### Shared memory buffer

Use shared memory (mmap) for the output stream instead of a socket.

Rejected because it adds complexity (synchronization, capacity management)
without meaningful benefit. The output rate is bounded by LLM token generation
speed, which is well within socket throughput. Shared memory is useful for
high-bandwidth IPC; this is a low-bandwidth text stream.

## Non-Goals

- **Multi-client attachment.** Only one client at a time. Concurrent
  attachment would require deciding which client receives output and which
  answers inquiries. Out of scope.

- **Persistent output buffer.** The detached process does not buffer output
  for future attach clients. If nobody is attached, output is discarded. The
  persisted event stream on disk is the record of what happened.

- **Cross-machine attachment.** Sockets are local. No network protocol.

- **Attaching to foreground queries.** `attach` only targets detached
  processes. A foreground query already has a terminal — there's nothing to
  attach to.

## Risks and Open Questions

### Renderer state after swap

When `swap_writers()` redirects output to the socket, the `ChatResponseRenderer`
and markdown buffer continue mid-stream. Their internal state (open code blocks,
list nesting, ANSI color stack) carries over. The client receives output that
continues from wherever the renderer was — not from the start of the current
chunk.

This means the client may see output that starts mid-paragraph or mid-code-block.
The replay-from-disk step provides context for completed events, but the live
transition may be visually rough for a few tokens until the renderer reaches a
natural boundary (end of paragraph, end of code block).

This is acceptable for an initial implementation. A smoother transition could
reset the renderer state and replay from the last complete event, but that adds
complexity for marginal UX improvement.

### Socket permissions

The socket file inherits the umask of the creating process. On most systems this
means owner-only access (0700 on the parent directory). This is correct — only
the same user should be able to attach.

If the user runs JP under different UIDs (e.g., via sudo), the socket may not
be accessible. This is an edge case that can be documented rather than
engineered around.

### Attach during the persist-and-exit window

When a detached process decides to persist and exit (queue policy, inquiry hit),
there's a brief window where it's persisting the incomplete turn and shutting
down. If a client attaches during this window, the process has already committed
to exiting.

The simplest behavior: the server sends `ProcessExiting` and closes the socket.
The client sees the message and suggests `--continue`. No special coordination
needed.

### Protocol versioning

The NDJSON protocol has no version field. If the message format changes in a
future release, an older client connecting to a newer server (or vice versa)
would see parse errors.

Adding a `version` field to the initial handshake is straightforward and should
be included from the start, even if the first version is just `1`.

## Implementation Plan

### Phase 1: Socket Listener

Each detached process opens a Unix domain socket alongside its registry entry.
The socket accepts connections but does nothing — no messages are sent. This
validates the socket lifecycle (creation, cleanup, stale detection).

Depends on [RFD 024] Phase 3 (`--detach` and daemonization).

Can be merged independently.

### Phase 2: Output Forwarding

When a client connects, the server swaps the printer to a socket-backed writer
([RFD 021]). Rendered output flows to the client. On disconnect, the printer
reverts to a sink. No inquiry handling yet — inquiries follow the detached
policy as before.

The client renders received output chunks to its terminal.

Depends on Phase 1 and [RFD 021].

### Phase 3: Inquiry Forwarding

When a client is attached and an inquiry arrives, the server sends an `Inquiry`
message instead of applying the detached policy. The client renders the prompt,
collects the answer, and sends `InquiryResponse`. The server delivers the
answer and continues execution.

Depends on Phase 2 and [RFD 019] Phase 2 (routing integration for
`has_client`).

### Phase 4: `jp conversation attach` CLI

Implement the `attach` subcommand with history replay (`--tail`), live output
rendering, and inquiry prompting. Handle connection errors, stale sockets, and
single-client enforcement.

Depends on Phase 3.

## References

- [RFD 024: Detached Query Execution](024-detached-query-execution.md) —
  `--detach`, process registry, and the `queue` policy that this RFD builds on.
- [RFD 023: Resumable Conversation Turns](023-resumable-conversation-turns.md) —
  incomplete turn persistence; the fallback when no client is attached.
- [RFD 021: Printer Live Redirection](021-printer-live-redirection.md) —
  `swap_writers()` used for routing output to the attached client.
- [RFD 020: Parallel Conversations](020-parallel-conversations.md) —
  conversation locks; the detached process holds the lock, the attach client
  does not.
- [RFD 019: Non-Interactive Mode](019-non-interactive-mode.md) — prompt
  routing and `has_client` state that changes dynamically on attach/detach.

[RFD 024]: 024-detached-query-execution.md
[RFD 023]: 023-resumable-conversation-turns.md
[RFD 021]: 021-printer-live-redirection.md
[RFD 020]: 020-parallel-conversations.md
[RFD 019]: 019-non-interactive-mode.md
