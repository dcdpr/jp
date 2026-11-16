# RFD 025: Live Re-Attachment to Detached Queries

- **Status**: Abandoned
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-19
- **Summary**: Connects a terminal to a running detached query over a Unix
  domain socket via `jp conversation attach`.

> [!IMPORTANT]
> This RFD is **Abandoned**.
>
> Superseded by [RFD 027: Client-Server Query Architecture][RFD 027], which
> makes live attachment the default execution model rather than an add-on.
> The socket protocol and attach flow from this RFD are carried forward into RFD
> 027.
>
> The original text below is preserved for historical context.

## Summary

This RFD introduces `jp conversation attach`, which connects a terminal to a
running detached query process.
The attached client receives live streaming output and answers inquiries
interactively.
When the client disconnects, the detached process resumes its configured
detached policy.

This design depends on [RFD 024] (Detached Query Execution) for the `--detach`
flag, process registry, and daemonization; [RFD 021] (Printer Live Redirection)
for runtime output swapping; and [RFD 019] (Non-Interactive Mode) for prompt
routing.

## Motivation

[RFD 024] introduces `--detach` for running queries in the background.
When a detached process hits an inquiry, it persists the incomplete turn and
exits so the user can resume it later with `--continue`.

However, a user cannot observe or interact with a detached query while it is
still running.
They may want to read a long streaming response, check progress across several
tool calls, or answer an expected inquiry immediately instead of waiting for the
process to exit and resuming with `--continue`.
Without live re-attachment, the only option is to wait for the process to finish
or exit at an inquiry and then inspect the transcript with `conversation print`.

## Design

### `jp conversation attach`

```sh
jp conversation attach <cid>
jp conversation attach                # session's active conversation
```

`jp conversation attach` connects to the running detached process for the
specified conversation, or the session's active conversation when omitted.
If no process is running:

- If the conversation has an incomplete turn (a pending inquiry), the command
  suggests resuming with `--continue`:

  ```sh
  $ jp conversation attach jp-c17528832001
  Error: No running process for jp-c17528832001.
    Conversation has a pending inquiry (fs_modify_file).
  
      jp query --continue --id=jp-c17528832001
                              Resume the incomplete turn.
  ```

- If the conversation is idle, the command returns an error:

  ```sh
  $ jp conversation attach jp-c17528831000
  Error: No running process for jp-c17528831000.
  ```

`attach` opens a read-write connection to an existing process rather than
starting a new query, so `jp query` has no `--attach` flag.

### IPC via Unix Domain Sockets

Each detached process listens on a Unix domain socket:

```txt
~/.local/share/jp/workspace/<workspace-id>/processes/<conversation-id>.sock
```

The socket sits alongside the process registry entry from [RFD 024].
The detached process creates the socket on startup and removes it on exit.

Because the socket path is derived from the conversation ID, `attach` does not
need the registry to locate the socket, though it still checks PID liveness in
the registry to confirm the process is running before connecting.

### Attach Protocol

The detached process (server) and `attach` command (client) communicate over the
Unix domain socket using newline-delimited JSON (NDJSON).

#### Message types

**Server -> Client:**

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

**Client -> Server:**

```rust
enum ClientMessage {
    /// Answer to a pending inquiry.
    InquiryResponse { id: InquiryId, answer: Value },

    /// Client is disconnecting gracefully.
    Disconnect,
}
```

#### Connection lifecycle

1. The client connects to the socket.
2. The server accepts the connection and sets `has_client = true` in its prompt
   routing state.
3. The server forwards rendered output to the client as `Output` messages.
4. When an inquiry arrives, the server sends an `Inquiry` message instead of
   applying the detached policy.
   The client renders the prompt and collects the user's answer.
5. The client sends `InquiryResponse`, and the server delivers the answer to the
   tool and continues execution.
6. When the turn completes, the server sends `TurnComplete`.
7. The client can disconnect at any time by sending `Disconnect` or closing the
   socket.
   The server resets `has_client = false` and resumes the detached policy.

#### Single client

Only one client can be attached at a time.
If a second client attempts to connect while one is already attached, the server
rejects the connection:

```sh
$ jp conversation attach jp-c17528832001
Error: Another client is already attached to jp-c17528832001.
```

### Output Redirection on Attach

When a client attaches, the server redirects its terminal output to the socket
using `Printer::swap_writers()` from [RFD 021].

#### Attach flow

1. **Replay from disk.** The client reads persisted events from the
   conversation's event stream and renders them using the `conversation print`
   logic.
   By default, it renders events from the current in-progress turn so the user
   sees what has happened so far.

2. **Flush and swap.** The server calls `printer.flush_instant()` followed by
   `printer.swap_writers(socket_writer)` to redirect subsequent rendered output
   from the sink to the client socket.

3. **Live streaming.** The client terminal renders the same live output as a
   foreground query: LLM response chunks, tool call headers, and progress
   indicators.

Between the events persisted on disk and the live output from the printer sits
any in-flight content in the `EventBuilder` and renderer buffers that has not
yet been flushed as a complete event.
Calling `flush_instant()` pushes all buffered content through the printer before
the writer swap.

#### Detach flow

When the client disconnects:

1. The server calls `printer.flush_instant()` to send any pending output to the
   client.
2. The server calls `printer.swap_writers(sink)` to resume discarding output.
3. The server sets `has_client = false`.
4. Subsequent inquiries follow the detached policy (`queue` by default).

### Context on Attach

When attaching mid-stream, the client renders recent history before switching to
live output:

| Flag       | Behavior                                 |
| ---------- | ---------------------------------------- |
| (default)  | Show events from the current in-progress |
|            | turn.                                    |
| `--tail=N` | Show the last N content events, plus the |
|            | current turn.                            |
| `--tail=0` | Skip history, show only live output from |
|            | this point.                              |

Content events are `ChatRequest`, `ChatResponse`, `ToolCallRequest`, and
`ToolCallResponse`.
Structural markers such as `TurnStart` and `ConfigDelta` are skipped when
counting.

The client renders history locally from the persisted conversation file.
Because disk storage is the source of truth for completed events, the server
does not need to buffer or replay historical output.

### Conversation Locks

The detached process holds the conversation lock ([RFD 020]) for its entire
execution.
The attach client acts only as a viewer and input provider: it does **not**
acquire a lock or write to the conversation stream, and instead communicates
with the lock-holding process over the socket.

### Prompt Routing

Attaching and detaching update the prompt routing state dynamically:

- **Client attached**: `has_client = true`.
  Inquiries are sent to the client over the socket
  (`PromptAction::PromptClient`).
- **Client detaches**: `has_client = false`.
  Inquiries follow the detached policy (`queue` → persist and exit).

Because the server makes the routing decision when each inquiry arrives, it does
not need to know in advance whether a client will be available.
A detached query that would otherwise exit at an inquiry can have that inquiry
answered interactively if a client attaches beforehand.

## Drawbacks

**IPC complexity.** Unix domain sockets, a message protocol, and output
redirection make live attachment the most complex part of the detached
conversations feature set.

**Platform constraints.** Unix domain sockets are not available on Windows.
Supporting Windows would require named pipes or another IPC mechanism, so the
initial implementation targets macOS and Linux only.

**Partial output gap.** Partial `ChatResponse` chunks still accumulating in the
`EventBuilder` have not yet been flushed as a complete event.
Calling `flush_instant()` flushes the printer buffer before the swap, but cannot
capture content that has not reached the printer yet.
In practice, this gap is at most a few tokens of LLM output.

**Socket cleanup on crash.** If the detached process is killed with `SIGKILL`,
its socket file remains on disk.
`conversation attach` checks PID liveness before connecting so stale sockets do
not cause connection attempts to hang, and `conversation ls` cleans up stale
socket files alongside stale registry entries.

## Alternatives

### HTTP server for IPC

Using a local HTTP server instead of Unix domain sockets would require port
allocation and firewall handling and is heavier than needed.
Unix domain sockets are the standard local IPC mechanism on Unix and enforce
access control through filesystem permissions.

### Named pipe (FIFO) instead of socket

Because named pipes are unidirectional, bidirectional communication (streaming
output to the client and receiving inquiry answers from it) would require two
pipes and extra coordination logic.
Unix domain sockets provide bidirectional streams natively.

### Inquiry forwarding without live output

Forwarding only inquiries would show the client inquiry prompts without the
LLM's response or tool execution progress.
The primary reason to attach is to observe a running query; if the user only
needs to answer inquiries, they can wait for the process to exit and run
`--continue` without any IPC infrastructure.

### Shared memory buffer

Streaming output through shared memory (`mmap`) instead of a socket would
require synchronization and capacity management for no practical benefit.
Output is a low-bandwidth text stream bounded by LLM token generation speed,
which fits well within socket throughput.

## Non-Goals

- **Multi-client attachment.** Only one client can attach at a time.
  Supporting concurrent clients would require deciding which client receives
  output and which answers inquiries.

- **Persistent output buffer.** The detached process discards output while no
  client is attached rather than buffering it for future clients.
  The persisted event stream on disk is the record of completed events.

- **Cross-machine attachment.** Communication uses local Unix domain sockets
  with no network protocol.

- **Attaching to foreground queries.** `attach` only targets detached processes,
  as a foreground query already has a terminal.

## Risks and Open Questions

### Renderer state after swap

When `swap_writers()` redirects output to the socket, `ChatResponseRenderer` and
the markdown buffer continue mid-stream with their existing internal state (open
code blocks, list nesting, and ANSI color stack).
The client receives output from the renderer's current position rather than the
start of the current chunk, so live output may begin mid-paragraph or
mid-code-block.

Replaying completed events from disk provides prior context, and any visual
roughness at the transition resolves once the renderer reaches the next
paragraph or code-block boundary.
Resetting the renderer state and replaying from the last complete event would
smooth the transition, but the extra complexity is not worth the minor UX gain
for an initial implementation.

### Socket permissions

The socket file inherits the umask of the creating process, and the parent
directory uses `0700` permissions so only the owning user can attach.
If the user runs JP under different UIDs (for example, via `sudo`), the socket
may be inaccessible.
This edge case can be documented rather than engineered around.

### Attach during the persist-and-exit window

When a detached process hits an inquiry under the `queue` policy, it enters a
brief window where it persists the incomplete turn and shuts down.
If a client attaches during this window after the process has already committed
to exiting, the server sends `ProcessExiting` and closes the socket.
The client sees the message and suggests `--continue` without extra
coordination.

### Protocol versioning

Without a version field in the NDJSON protocol, a message-format change in a
future release would cause parse errors when an older client connects to a newer
server (or vice versa).
The initial handshake should include a `version` field from the start, beginning
at `1`.

## Implementation Plan

### Phase 1: Socket Listener

Each detached process opens a Unix domain socket alongside its registry entry.
The socket accepts connections without sending messages to validate socket
creation, cleanup, and stale-socket detection.

Depends on [RFD 024] Phase 3 (`--detach` and daemonization) and can be merged
independently.

### Phase 2: Output Forwarding

When a client connects, the server swaps the printer to a socket-backed writer
([RFD 021]) and streams rendered output chunks for the client to print to its
terminal.
On disconnect, the printer reverts to a sink.
Inquiries continue to follow the detached policy.

Depends on Phase 1 and [RFD 021].

### Phase 3: Inquiry Forwarding

When a client is attached and an inquiry arrives, the server sends an `Inquiry`
message instead of applying the detached policy.
The client renders the prompt, collects the answer, and sends `InquiryResponse`,
which the server delivers to the tool to continue execution.

Depends on Phase 2 and [RFD 019] Phase 2 (routing integration for `has_client`).

### Phase 4: `jp conversation attach` CLI

Implement the `attach` subcommand with history replay (`--tail`), live output
rendering, inquiry prompting, connection error handling, stale-socket detection,
and single-client enforcement.

Depends on Phase 3.

## References

- [RFD 024: Detached Query Execution][RFD 024]: `--detach`, the process
  registry, and the `queue` policy that this RFD builds on.
- [RFD 023: Resumable Conversation Turns][RFD 023]: incomplete-turn persistence,
  used as the fallback when no client is attached.
- [RFD 021: Printer Live Redirection][RFD 021]: `swap_writers()`, used to route
  output to the attached client.
- [RFD 020: Parallel Conversations][RFD 020]: conversation locks, held by the
  detached process rather than the attach client.
- [RFD 019: Non-Interactive Mode][RFD 019]: prompt routing and the `has_client`
  state updated on attach and detach.

[RFD 019]: 019-non-interactive-mode.md
[RFD 020]: 020-parallel-conversations.md
[RFD 021]: 021-printer-live-redirection.md
[RFD 023]: 023-resumable-conversation-turns.md
[RFD 024]: 024-detached-query-execution.md
[RFD 027]: 027-client-server-query-architecture.md
