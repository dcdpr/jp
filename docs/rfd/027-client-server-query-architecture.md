# RFD 027: Client-Server Query Architecture

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-19

> [!WARNING]
> This RFD is an early draft and incomplete. The overall direction is
> established but many details need work.

## Summary

This RFD restructures `jp query` as a client-server system. Every query spawns a
server process that runs the agent loop ([RFD 026]) and a client process that
connects to the server, renders output, and handles user interaction. Detaching
is a client operation (disconnect and exit), not a server operation. This
unifies foreground queries, `--detach`, Ctrl+Z mid-execution detach, and
`conversation attach` under a single execution model.

This RFD depends on [RFD 026] (Agent Loop Extraction) for the `jp_agent` crate
that the server calls, on [RFD 023] (Resumable Conversation Turns) for
incomplete turn persistence as a fallback, on [RFD 020] (Parallel Conversations)
for conversation locks, and on [RFD 019] (Non-Interactive Mode) for detached
prompt policies.

This RFD supersedes [RFD 024] (Detached Query Execution) and [RFD 025] (Live
Re-Attachment to Detached Queries).

## Motivation

RFDs [024][RFD 024] and [025][RFD 025] proposed two separate mechanisms: a
persist-and-exit model for detached queries that hit inquiries, and an IPC-based
live attachment for observing running processes. These are correct in isolation
but create two execution models with different behavior:

- **Foreground**: The agent loop runs in-process. Output goes to the terminal.
  Prompts are interactive. No IPC.
- **Detached**: The agent loop runs in a daemon. Output is discarded. On
  inquiry, the process persists and exits. Resumption via `--continue` starts a
  new process.
- **Attached**: An IPC client connects to a running daemon for live output and
  prompt forwarding.

Three models, three code paths, three sets of edge cases. Mid-execution detach
(Ctrl+Z) requires a fourth path: persist the in-progress state, re-exec as a
daemon, and resume — losing the active LLM stream in the process.

A simpler architecture: every query is a server process. The user's terminal is
always a client. The difference between foreground, detached, and attached is
whether a client is connected, not how the server was started.

## Design

### Architecture

```
jp query "message"          jp query --detach "message"
        │                              │
        ▼                              ▼
 ┌─────────────┐                ┌─────────────┐
 │   Client    │                │   Client    │
 │  (attach)   │                │  (exit)     │
 └──────┬──────┘                └──────┬──────┘
        │ IPC                          │ IPC (brief)
        ▼                              ▼
 ┌─────────────┐                ┌─────────────┐
 │   Server    │                │   Server    │
 │ (jp_agent)  │                │ (jp_agent)  │
 └─────────────┘                └─────────────┘
```

The server is the `jp` binary re-executed with a hidden `_serve` subcommand
(e.g. `jp _serve --config-fd=N --socket-path=...`). This subcommand is not
visible in `--help` and is not part of the public CLI surface. It runs the agent
loop from `jp_agent`, manages the conversation lock, persists events, and
accepts client connections over a local IPC transport.

The client is the user's `jp query` process. It connects to the server, renders
output to the terminal, forwards user input (inquiry answers, interrupt
signals), and exits when it disconnects or the server finishes.

### Startup Flow

`jp query "message"`:

1. Client process starts. Parses args, resolves config, opens editor if needed,
   builds `ChatRequest`.
2. Client resolves the target conversation, acquires the conversation lock.
3. Client spawns the server as a detached process via re-exec: `jp _serve
   --config-fd=N --socket-path=PATH`. The resolved config, conversation ID,
   `ChatRequest`, and tool definitions are serialized to a temp file; the file
   descriptor is inherited by the server process. The conversation lock fd is
   also inherited (see [Lock Handoff](#lock-handoff-timing)). The
   platform-specific spawning mechanism is described in [Platform
   Portability](#platform-portability).
4. Server writes its PID to the process registry, opens the IPC endpoint, and
   starts the agent loop.
5. Client connects to the IPC endpoint and enters the attach loop: read output
   from server, write to terminal; read terminal input (inquiry answers,
   interrupt signals), forward to server.
6. When the server finishes the turn, it sends a `TurnComplete` message. Client
   renders any final output and exits.

`jp query --detach "message"`:

Steps 1-4 are identical. At step 5, the client prints the conversation ID and
exits instead of connecting. The server runs unattended.

### Server Process

The server is a headless process that runs `jp_agent::run_turn_loop()`. It has
no terminal. It accepts one client connection at a time over the IPC transport.
Multiple concurrent clients are not supported in this RFD (see
[Non-Goals](#non-goals)), though the architecture does not preclude adding
read-only observer clients in the future.

#### Output routing

The server uses a `ResponseRenderer` ([RFD 026]) that forwards raw
`ChatResponse` events to connected clients as structured protocol messages. When
no client is connected, events are buffered in memory (see [Rendering
Continuity](#rendering-continuity)). The server does not format, style, or
render output — it sends structured data and leaves presentation to the client.

This means different clients can render the same stream differently: one client
renders markdown with ANSI styling for a terminal, another renders JSON for a
pipeline, a future observer client might render a minimal progress view. The
server doesn't need to know about terminal capabilities.

#### Rendering Continuity

When a client detaches and a new client re-attaches, the new client must be able
to produce correct markdown output from the current stream position. The server
supports this with an **event replay buffer** and per-client read pointers.

##### Event replay buffer

The server always appends `ChatResponse` chunks to an in-memory buffer,
regardless of whether any client is connected. Every time a complete event is
flushed by the `EventBuilder` and persisted to disk, the buffer is cleared and
all client read pointers are reset. The buffer only holds the content between
the last persisted event and the current stream position — typically a few
hundred tokens at most.

Each persisted event represents a complete markdown document boundary. The
in-memory buffer therefore always starts at a clean boundary, and can be
rendered correctly by a fresh `jp_md::Buffer` without any prior context. The
client reads persisted events from disk for *context* (so the user sees what
happened), not for renderer correctness.

##### Per-client read pointers

When a client connects, the server creates a read pointer for that client at the
start of the current buffer. The server sends everything from the pointer to the
buffer head, then advances the pointer. As new chunks arrive, the server appends
to the buffer and sends them to all connected clients, advancing each pointer.

When a client disconnects, its pointer is kept (keyed by client ID). If the
client reconnects before the buffer is flushed, it resumes from where it left
off — no data is missed, no data is duplicated. If the buffer was flushed
between disconnect and reconnect (the events are now on disk), the pointer is
discarded and a new one is created at the start of the current buffer. The
client re-reads the now-persisted events from disk.

This model naturally supports multiple clients without special-casing. From the
server's perspective, there is no behavioral difference between "client
attached" and "no client" — it always appends to the buffer.

##### Re-attach flow

1. **Read history from disk.** The client reads persisted events and renders
   them locally for context.

2. **Connect and receive the buffer.** The server sends all buffered chunks from
   the client's read pointer (or from the start of the buffer for new clients).
   The client pushes these into its `ResponseRenderer`, producing correct
   markdown from the clean boundary.

3. **Live streaming.** New chunks flow in as they arrive. The client's renderer
   continues seamlessly — the buffer content and the live stream are one
   continuous markdown flow.

The critical seam is between steps 2 and 3: the buffered content and the live
stream must be processed as a single continuous document by the client's
`jp_md::Buffer`. This happens naturally because the buffer IS the leading edge
of the live stream — the client's read pointer catches up to the head, and then
new chunks extend it.

#### Inquiry routing

The server implements `PromptBackend` as a socket-backed variant. When the agent
loop calls the prompt backend:

- **Client attached**: The server serializes the inquiry and sends it to the
  client via the socket. The client renders the prompt on its terminal, collects
  the answer, and sends it back. The server delivers the answer to the agent
  loop.
- **No client**: The server applies the detached prompt policy ([RFD 019]). For
  `defer` policy, this means persisting the incomplete turn ([RFD 023]) and
  shutting down. For `auto`, `defaults`, or `deny`, the existing behavior from
  [RFD 019] applies.

#### Shutdown

The server has no controlling terminal and does not rely on OS signals for
lifecycle management. All shutdown triggers arrive as protocol messages or
natural execution outcomes:

- Turn completes → server exits.
- `defer` policy triggers → server persists and exits.
- `conversation kill` connects to the IPC endpoint and sends a `Shutdown`
  message → server persists and exits.
- Unrecoverable error → server persists what it can and exits.

This is cross-platform by design — no dependence on Unix signals for shutdown
coordination.

#### Lifecycle

On exit, the server removes its process registry entry and IPC endpoint. The
conversation lock is released automatically (lock file close).

### Client Process

The client is a thin terminal adapter. It does not run the agent loop.

#### Attach loop

After connecting to the server socket, the client enters a loop:

1. Read messages from the server (output chunks, inquiry requests, turn
   completion, errors).
2. Write output chunks to the terminal.
3. On inquiry request: render the prompt using `TerminalPromptBackend`, collect
   the answer, send it to the server.
4. On Ctrl+C: send an interrupt message to the server, receive menu options,
   render the interrupt menu locally, send the chosen action back.
5. On `TurnComplete`: render final output, exit.

#### Ctrl+Z (detach)

When the user presses Ctrl+Z, the client:

1. Sends a `Disconnect` message to the server.
2. Prints "Detached: \<conversation-id\>".
3. Exits.

The server continues running. The client's read pointer is preserved in case it
reconnects. If other clients are connected, they continue receiving events. If
no clients remain, events continue buffering. Subsequent inquiries use the
detached policy.

No state is lost. No re-exec. No LLM stream interruption. The server doesn't
even know the user pressed Ctrl+Z — it just sees a client disconnect.

#### Ctrl+C (interrupt)

Ctrl+C on the client does NOT disconnect. It triggers the interrupt protocol:

1. Client sends `Signal { kind: Interrupt }` to the server.
2. Server pauses streaming (same as today's SIGINT handler).
3. Server sends `InterruptMenu { options }` to the client.
4. Client renders the menu using `InterruptHandler`, collects the user's choice.
5. Client sends `InterruptAction { action }` to the server.
6. Server processes the action (Stop, Abort, Continue, Reply, Detach).
7. If the action is `Detach`, the server sends `Acknowledged` and the client
   disconnects (same as Ctrl+Z).

### `conversation attach`

```sh
jp conversation attach <cid>
jp conversation attach              # session's active conversation
```

Connects to a running server process. The flow is the same as the client attach
loop described above, but without spawning a server — the server already exists.

#### Context on attach

When a client attaches to a server that's already mid-stream, it follows the
rendering continuity flow described in [Rendering
Continuity](#rendering-continuity):

1. Read persisted events from disk and render them locally.
2. Request and replay the server's in-memory event buffer.
3. Switch to live streaming.

By default, the client renders events from the current in-progress turn.

| Flag       | Behavior                                 |
|------------|------------------------------------------|
| (default)  | Show events from the current in-progress |
|            | turn.                                    |
| `--tail=N` | Show the last N content events.          |
| `--tail=0` | Skip history and replay, connect to live |
|            | stream only.                             |

#### No running process

If no server is running for the conversation:

- If there's a pending inquiry (`IncompleteTurn`), suggest `--continue`.
- If the conversation is idle, error.

### `conversation kill`

Connects to the server's IPC endpoint and sends a `Shutdown` message. The server
persists its current state and exits cleanly. The conversation lock is released.

If the IPC endpoint is unresponsive (server hung), `conversation kill --force`
falls back to OS-level process termination (`SIGKILL` on Unix,
`TerminateProcess` on Windows) using the PID from the registry.

### Process Registry

Running servers register in the user-local data directory:

```txt
~/.local/share/jp/workspace/<workspace-id>/processes/
├── <conversation-id>.json    # PID, start time
└── <conversation-id>.sock    # IPC endpoint (Unix socket or named pipe path)
```

The registry entry is minimal (PID and start time). The `waiting-for-input`
state is derived from the conversation's `IncompleteTurn` on disk, not from the
registry — by the time a query is waiting, the server has exited.

Stale entries (dead PID) are cleaned up by `conversation ls`.

### `conversation ls` (extended)

| Status                     | Source                                   |
|----------------------------|------------------------------------------|
| `running (pid NNN)`        | Process registry (PID liveness check)    |
| `waiting-for-input (tool)` | Conversation stream (last event is       |
|                            | `InquiryRequest`)                        |
| `interrupted (...)`        | Conversation stream (incomplete turn, no |
|                            | inquiry)                                 |
| (idle)                     | No process, no incomplete turn           |

### IPC Transport

The IPC layer is abstracted behind traits so that the protocol logic is
transport-agnostic:

```rust
/// Listens for incoming client connections.
pub trait IpcListener: Send + Sync {
    type Stream: IpcStream;

    /// Accept a new client connection.
    async fn accept(&self) -> io::Result<Self::Stream>;
}

/// A bidirectional byte stream between client and server.
pub trait IpcStream: AsyncRead + AsyncWrite + Send + Unpin {}
```

On Unix, these are implemented over `tokio::net::UnixListener` / `UnixStream`.
On Windows, over `tokio::net::windows::named_pipe`. The client side uses a
corresponding `connect` function that returns an `impl IpcStream`.

### Protocol

Newline-delimited JSON over the IPC transport. Single client at a time.

#### Handshake

On connect, the client sends:

```json
{
  "type": "hello",
  "version": 1,
  "can_prompt": true
}
```

The server acknowledges:

```json
{
  "type": "hello",
  "version": 1
}
```

Version mismatch → server rejects with an error message.

`can_prompt` indicates whether the client can render interactive prompts
(`/dev/tty` available). When `false`, the server treats the client as an
output-only consumer and applies the detached policy for inquiries.

The handshake deliberately omits terminal capabilities (width, ANSI support,
output format). The server sends structured events; the client decides how to
render them based on its own context.

#### Server → Client

| Message                         | When                                     |
|---------------------------------|------------------------------------------|
| `Event { kind, data }`          | Structured agent event (chat response,   |
|                                 | tool call, progress)                     |
| `Inquiry { inquiry, metadata }` | Tool needs user input                    |
| `InterruptMenu { options }`     | Response to client's interrupt signal    |
| `TurnComplete`                  | Turn finished normally                   |
| `ProcessExiting { reason }`     | Server shutting down                     |
#### Client → Server

| Message                          | When                                     |
|----------------------------------|------------------------------------------|
| `InquiryResponse { id, answer }` | User answered an inquiry                 |
| `Signal { kind }`                | Ctrl+C or other interrupt                |
| `InterruptAction { action }`     | User's menu choice                       |
| `Resize { width, height }`       | Terminal resized (reserved for future    |
|                                  | use)                                     |
| `Disconnect`                     | Client detaching (Ctrl+Z)                |
| `Shutdown`                       | Clean shutdown request (`conversation    |
|                                  | kill`)                                   |
### The `defer` Detached Policy

This RFD carries forward the fourth detached policy from [RFD 024], renamed from
`queue` to `defer`:

| Mode        | Behavior                                 |
|-------------|------------------------------------------|
| `auto`      | Auto-approve or route to LLM ([RFD       |
|             | 019]).                                   |
| `defaults`  | Use default values ([RFD 019]).          |
| `deny`      | Fail the tool call ([RFD 019]).          |
| **`defer`** | **Persist the incomplete turn and        |
|             | exit.**                                  |

`defer` is the default policy when no client is attached. The server lets other
tools in the batch complete, persists their results incrementally ([RFD 023]),
persists the `InquiryRequest`, and exits.

The name reflects the intent: the inquiry is deferred until a user is available,
not queued in a running process. The server exits cleanly; resumption happens
via `jp query --continue --id=<cid>`, which spawns a new server and client for
the resumed turn.

### End-to-End Workflows

#### Normal foreground query

```sh
$ jp query "Refactor the auth module"
# Client spawns server, attaches, shows streaming output.
# Tools execute, inquiries are prompted interactively.
# Turn completes. Client exits.
```

#### Detached query

```sh
$ jp query --detach "Run cargo check on all crates"
Detached: jp-c17528832001

$ jp conversation ls
jp-c17528832001   Run cargo check   running (pid 12345)

# Later:
$ jp conversation ls
jp-c17528832001   Run cargo check   idle

$ jp conversation print --last --id=jp-c17528832001
# Shows the completed turn.
```

#### Detach mid-execution (Ctrl+Z)

```sh
$ jp query "Refactor the auth module"
# Streaming output appears...
# User presses Ctrl+Z
Detached: jp-c17528832001
$
# Server continues running in the background.

$ jp conversation attach jp-c17528832001
# Live output resumes from where it was.
```

#### Detached query hits inquiry

```sh
$ jp query --detach "Refactor auth"
Detached: jp-c17528832001

$ jp conversation ls
jp-c17528832001   Refactor auth   waiting-for-input (fs_modify_file)

$ jp query --continue --id=jp-c17528832001
# New server spawns, resumes the incomplete turn.
# Client attaches, prompts for the inquiry, continues.
```

#### Attach to a running query from another terminal

```sh
# Terminal 1:
$ jp query --detach "Long running task"
Detached: jp-c17528832001

# Terminal 2:
$ jp conversation attach jp-c17528832001
# Live output streams in. Inquiries are prompted here.
```

### Piped Execution

The client-server model is transparent to pipelines. The client is the process
in the pipeline — it reads stdin, writes to stdout/stderr. The server is
invisible:

```txt
echo foo | jp query "fix" | handler.sh
           ┌──────────────┐
stdin ──▶  │    Client    │ ──▶ stdout (to handler.sh)
           │  (jp query)  │ ──▶ stderr (chrome, progress)
           └───────┬──────┘
                   │ IPC
           ┌───────┴──────┐
           │    Server    │
           │  (jp _serve) │
           └──────────────┘
```

The client reads stdin before spawning the server (stdin content becomes part of
the `ChatRequest`). The server never touches the pipeline's stdin/stdout.

#### Output routing

The server sends structured events (chat response chunks, tool call
notifications, progress updates). The client renders them according to its own
output context:

- Terminal stdout: markdown with ANSI styling, chrome on stderr.
- Piped stdout: plain text on stdout, chrome on stderr.
- `--format json`: NDJSON on stdout.

The client makes all rendering decisions locally. The server is unaware of the
client's output format or terminal capabilities.

#### Prompts in pipelines

When stdout is piped but the user is at a terminal (`/dev/tty` available), the
client can still prompt interactively. Inquiry messages arrive from the server
via IPC; the client renders them via `/dev/tty` using `TerminalPromptBackend`
(per [RFD 019]). The pipeline is unaffected — prompt I/O never touches stdout or
stdin.

When no terminal is available at all (CI, cron, `ssh -T`), the client cannot
prompt. It signals `can_prompt = false` in the handshake. The server applies the
detached policy for inquiries. The client still receives and forwards output to
stdout.

#### SIGPIPE handling

If the downstream process closes early (`jp query | head -10`), the client
receives SIGPIPE when writing to stdout. Today, JP dies and the work is lost.
With client-server, the client dies but the server is a separate process — it
keeps running and finishes the turn.

Whether this is desirable depends on the user's intent. Two options:

- **Pipeline semantics (default):** Client sends `Shutdown` to the server before
  exiting on SIGPIPE. The entire pipeline stops cleanly.
- **Background semantics (`--persist-on-sigpipe`):** Client disconnects without
  sending `Shutdown`. The server finishes the turn. The work is preserved.

The default should match user expectations for pipeline behavior: if the
downstream consumer is gone, stop producing. Users who want the server to
continue can opt in.

## Drawbacks

**Two processes for every query.** Even a simple `jp query "hello"` spawns a
server and a client. The overhead is a process spawn (~10-50ms) plus socket
setup, which is small relative to LLM latency but nonzero.

**IPC is on the critical path.** All output flows through the IPC transport. For
terminal rendering, this adds a hop compared to direct stdout writes. In
practice, the bottleneck is LLM token generation speed, not local IPC
throughput.

**Interrupt handling becomes a protocol exchange.** Today, Ctrl+C is a
synchronous in-process operation: signal → menu → action. With client-server,
it's: signal on client → message to server → menu options back → user choice →
action message. More round trips, slightly higher latency before the menu
appears. Likely imperceptible but architecturally more complex.

**Platform-specific code.** The IPC transport and process spawning have
platform-specific implementations (see [Platform
Portability](#platform-portability)). The abstractions minimize this, but each
platform needs testing.

**Complexity upfront.** Unlike the incremental approach in RFDs 024/025
(foreground first, detach later, attach last), this model requires the full
client-server infrastructure before any query works. Phase 1 is larger.

## Alternatives

### Two execution models (RFDs 024 + 025)

Foreground queries run in-process. Detached queries use a daemon with
persist-and-exit at inquiries. Live attachment is a separate IPC layer.

Rejected because three execution models create three code paths with different
behaviors. Mid-execution detach (Ctrl+Z) requires a fourth path with state loss
(active LLM stream dropped during re-exec). The unified model eliminates these
problems.

### Spawn server only when detaching

Start queries in-process. On Ctrl+Z or `--detach`, re-exec as a daemon.

Rejected because the re-exec loses the active LLM stream and in-memory state.
The server would need to retry the LLM call or resume from persisted state,
which is lossy and complex. Always starting as a server avoids this entirely.

### PTY proxy (abduco model)

Spawn a PTY, run the agent as a subprocess inside it, proxy terminal I/O over a
socket. The agent doesn't know about the client-server split.

Rejected because JP's I/O is structured (inquiries, interrupt menus, tool
prompts), not raw terminal bytes. A PTY proxy would lose the structure and
require parsing terminal output to reconstruct it. The structured NDJSON
protocol preserves semantics.

### tmux/screen wrapper

Tell users to run JP inside tmux for detach/reattach.

Rejected because it doesn't integrate with `conversation ls`, requires an
external dependency, and doesn't support the structured inquiry protocol.

## Non-Goals

- **Multi-client attachment.** One client at a time per server. The architecture
  could support read-only observer clients (streaming output to multiple
  terminals) or first-answer-wins inquiry handling, but the coordination adds
  complexity. Deferred to a future RFD if the need arises.

- **Cross-machine attachment.** Sockets are local. No network protocol.

- **Public server API.** The socket protocol is internal. It may change between
  releases without notice.

- **Non-query commands as servers.** Only `jp query` spawns servers.
  `conversation ls`, `conversation print`, etc. run in-process as today.

## Risks and Open Questions

### Lock handoff timing

The client acquires the conversation lock before spawning the server.

On Unix, file descriptors survive `fork()` and `exec()` unless marked
`FD_CLOEXEC`. The client opens the lock file, acquires `flock`, then spawns the
server (which inherits the fd via `fork`). The child process holds the same lock
from birth. The client closes its copy of the fd after confirming the server
started. The lock transfers without ever being released.

On Windows, fd inheritance works differently. The lock file path is passed to
the server process, which re-acquires the lock via `LockFileEx`. There is a
brief window between the client releasing and the server acquiring. This is
mitigated by the server acquiring the lock before signaling readiness to the
client; if the lock is lost in the window, the server fails to start and the
client reports the error.

If the server fails to start for any reason, the client still holds the lock and
reports the error. No orphaned locks.

### Config and state transfer

The client resolves config, builds the `ChatRequest`, resolves tool definitions.
This data needs to reach the server. Options:

- **Serialized to a temp file, fd inherited.** The client writes the resolved
  config to a temp file, passes the fd to the server. Simple, works with
  arbitrary data sizes.
- **Command-line arguments.** Too limited for complex config.
- **Shared memory.** Overkill for a one-time transfer.

Temp file with fd inheritance is the pragmatic choice.

### Server startup latency

The server needs to initialize: read config from the temp file, set up the tokio
runtime, open the socket, connect to MCP servers, and signal readiness. The
client waits for the socket to appear before connecting.

For a fast query, this adds startup latency. Mitigation: the server signals
readiness via a pipe (write a byte when the socket is open). The client blocks
on this pipe instead of polling for the socket file.

### Terminal resize

Since the client handles all rendering, terminal resize (SIGWINCH on Unix,
console events on Windows) is handled entirely client-side. The client updates
its own formatter's terminal width. No protocol message needed.

The `Resize` message in the protocol table is reserved for future use (e.g., if
the server ever needs to know the client's dimensions for formatting tool
output), but is not required for the initial implementation.

### Structured output (`--format json`)

When `--format json` is set, the client renders events as NDJSON instead of
formatted text. This is a client-side rendering decision — the server sends the
same structured events regardless. The client's `ResponseRenderer`
implementation determines the output format.

### Fallback for restricted environments

Some environments (containers with restricted IPC, unusual sandboxes) may not
support the IPC transport. A fallback to in-process execution (today's model)
could be provided behind a flag (`--no-server`) for compatibility. This is not a
launch requirement but should be considered.

### Platform Portability

The architecture is cross-platform by design. Platform-specific details are
isolated behind two abstractions:

**IPC transport.** Abstracted behind the `IpcListener`/`IpcStream` traits. The
protocol layer (NDJSON messages) is transport-agnostic.

| Platform      | Transport           | Rust ecosystem                    |
|---------------|---------------------|-----------------------------------|
| macOS / Linux | Unix domain sockets | `tokio::net::UnixListener`        |
| Windows       | Named pipes         | `tokio::net::windows::named_pipe` |

The `interprocess` crate provides `LocalSocketListener`/`LocalSocketStream` as a
cross-platform abstraction, or JP can implement the trait directly per platform.

**Process spawning.** The server must outlive the client. On Unix, this uses the
[double-fork pattern][double-fork]: fork once to create a child, the child calls
`setsid()` to become a session leader (detaching from the client's terminal),
then forks again. The intermediate process exits immediately, so the grandchild
is reparented to PID 1 and cannot acquire a controlling terminal. On Windows,
`CreateProcess` with detached flags achieves the same result without forking.

| Platform | Mechanism                                | Rust                                     |
|----------|------------------------------------------|------------------------------------------|
| Unix     | Double-fork + `setsid`                   | `nix::unistd`                            |
| Windows  | `DETACHED_PROCESS` +                     | `std::process::Command` +                |
|          | `CREATE_NEW_PROCESS_GROUP`               | `.creation_flags()`                      |

[double-fork]: https://0xjet.github.io/3OHA/2022/04/11/post.html

**Shutdown.** Uses protocol messages (`Shutdown` via IPC), not OS signals.
`conversation kill --force` uses `SIGKILL` on Unix, `TerminateProcess` on
Windows.

**PID liveness.** `kill(pid, 0)` on Unix, `OpenProcess(SYNCHRONIZE, ...)` on
Windows. Used only for stale registry cleanup.

**Lock handoff.** `flock` fds are inherited across fork on Unix. On Windows, the
lock file path is passed and the server re-acquires via `LockFileEx` (the
`fd-lock` crate handles both platforms, per [RFD 020]).

**Terminal events.** SIGWINCH on Unix, console events on Windows. Both handled
by `crossterm` in the client. The server never interacts with a terminal
directly.

## Implementation Plan

### Phase 1: Server Process and IPC

Implement the server process: platform-specific detached spawning, process
registry, IPC listener (Unix sockets on Unix, named pipes on Windows). The
server runs `jp_agent::run_turn_loop()` with sink writers (no client). `jp query
--detach` uses this: spawn server, exit.

`conversation kill` sends `Shutdown` via IPC. `conversation ls` shows running
servers.

No client attachment yet — foreground `jp query` still runs in-process using the
old code path.

Depends on [RFD 026] (agent loop extraction).

### Phase 2: Client Attach Loop

Implement the client: connect to server socket, render output, forward
inquiries, handle disconnects. `conversation attach` uses this to connect to
running servers.

The server forwards structured events to the attached client. When no client is
connected, events are buffered for replay on re-attach (see [Rendering
Continuity](#rendering-continuity)).

Foreground `jp query` still runs in-process.

### Phase 3: Interrupt Protocol

Implement the Ctrl+C interrupt exchange: client sends `Signal`, server responds
with `InterruptMenu`, client sends `InterruptAction`. The `InterruptHandler`
logic moves to the client (menu rendering) and server (action processing).

### Phase 4: Foreground Queries as Client-Server

Wire `jp query` (without `--detach`) to spawn a server and immediately attach.
This replaces the in-process turn loop for all foreground queries.

Add Ctrl+Z handling: client sends `Disconnect` and exits.

The old in-process code path can be retained behind `--no-server` as a fallback.

### Phase 5: Cleanup

Remove the old in-process turn loop from `jp_cli` (or keep behind
`--no-server`). Update `--detach` to use the same spawn path as foreground
queries (skip step 5). Ensure `--continue` spawns a new server for resumed
turns.

## References

- [RFD 026: Agent Loop Extraction][RFD 026] — `jp_agent` crate that the server
  calls.
- [RFD 023: Resumable Conversation Turns][RFD 023] — incomplete turn
  persistence; fallback when `queue` policy triggers.
- [RFD 021: Printer Live Redirection][RFD 021] — `swap_writers()` may still be
  useful for non-rendering output (tracing, diagnostics) but is no longer
  central to the client-server output model.
- [RFD 020: Parallel Conversations][RFD 020] — conversation locks inherited by
  the server.
- [RFD 019: Non-Interactive Mode][RFD 019] — detached prompt policies; `queue`
  is the default when no client attached.
- [RFD 024: Detached Query Execution][RFD 024] — superseded; process registry
  and `queue` policy carried forward.
- [RFD 025: Live Re-Attachment][RFD 025] — superseded; socket protocol and
  attach flow carried forward.
- [abduco](https://github.com/martanne/abduco) — prior art for minimal session
  detach/reattach over Unix sockets.

[RFD 019]: 019-non-interactive-mode.md
[RFD 020]: 020-parallel-conversations.md
[RFD 021]: 021-printer-live-redirection.md
[RFD 023]: 023-resumable-conversation-turns.md
[RFD 024]: 024-detached-query-execution.md
[RFD 025]: 025-live-re-attachment-to-detached-queries.md
[RFD 026]: 026-agent-loop-extraction.md
