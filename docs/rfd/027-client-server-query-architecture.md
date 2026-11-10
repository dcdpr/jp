# RFD 027: Client-Server Query Architecture

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-19
- **Requires**: [RFD 023], [RFD 026], [RFD 049]
- **Summary**: Runs every `jp query` in a server process with an IPC terminal
  client to unify foreground, detached, and reattached execution.

> [!WARNING]
> This RFD is an early draft and incomplete.
> The overall direction is established but many details need work.

## Summary

This RFD restructures `jp query` as a client-server system.
Every query runs a server process that executes the agent loop ([RFD 026]) and a
client process that connects to the server, renders output, and handles user
interaction.
Detaching is a client operation: the client disconnects and exits while the
server keeps running.
This unifies foreground queries, `--detach`, Ctrl+Z mid-execution detach, and
`conversation attach` under one execution model.

This design depends on [RFD 026] (Agent Loop Extraction) for the `jp_agent`
crate that the server calls, [RFD 023] (Resumable Conversation Turns) for
incomplete turn persistence as a fallback, [RFD 020] (Parallel Conversations)
for conversation locks, and [RFD 019] (Non-Interactive Mode) for detached prompt
policies.
It supersedes [RFD 024] (Detached Query Execution) and [RFD 025] (Live
Re-Attachment to Detached Queries).

## Motivation

RFDs [024][RFD 024] and [025][RFD 025] proposed two separate mechanisms: a
persist-and-exit model for detached queries that hit inquiries, and IPC-based
live attachment for observing running processes.
While each mechanism works in isolation, together they split execution into
three distinct modes:

- **Foreground**: The agent loop runs in-process, writes directly to the
  terminal, and prompts interactively without IPC.
- **Detached**: The agent loop runs in a daemon and discards output.
  When it hits an inquiry, the process persists state and exits; `--continue`
  then starts a new process.
- **Attached**: An IPC client connects to a running daemon for live output and
  prompt forwarding.

Each mode requires its own code path and edge-case handling.
Mid-execution detach (Ctrl+Z) requires a fourth path that persists in-progress
state, re-executes as a daemon, and resumes, dropping the active LLM stream in
the process.

Running every query in a server process with the user's terminal as a client
eliminates these separate paths.
Foreground, detached, and attached queries differ only in whether a client is
connected, not in how the server is started.

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
(e.g. `jp _serve --config-fd=N --socket-path=...`), which is omitted from
`--help` and the public CLI surface.
It holds the conversation lock, runs the agent loop from `jp_agent`, persists
events, and accepts client connections over a local IPC transport.

The client is the user's `jp query` process.
It connects to the server, renders output to the terminal, forwards user input
(inquiry answers and interrupt signals), and exits when it disconnects or the
server finishes.

### Startup Flow

For a foreground query (`jp query "message"`):

1. The client parses arguments, resolves config, opens the editor if needed, and
   builds the `ChatRequest`.
2. The client resolves the target conversation and acquires the conversation
   lock.
3. The client serializes the resolved config, conversation ID, `ChatRequest`,
   and tool definitions to a temp file, then spawns the server as a detached
   process via re-exec: `jp _serve --config-fd=N --socket-path=PATH` (see
   [Platform Portability](#platform-portability)).
   The server inherits both the temp file descriptor and the conversation lock
   fd (see [Lock Handoff Timing](#lock-handoff-timing)).
4. The server writes its PID to the process registry, opens the IPC endpoint,
   and starts the agent loop.
5. The client connects to the IPC endpoint and enters the attach loop, writing
   server output to the terminal and forwarding terminal input (inquiry answers
   and interrupt signals) to the server.
6. When the turn finishes, the server sends a `TurnComplete` message, and the
   client renders any final output and exits.

For a detached query (`jp query --detach "message"`), steps 1 through 4 are
identical.
At step 5, the client prints the conversation ID and exits without connecting,
leaving the server to run unattended.

### Server Process

The server is a headless process with no controlling terminal.
It runs `jp_agent::run_turn_loop()` and accepts one client connection at a time
over the IPC transport.
Multiple concurrent clients are out of scope for this RFD (see
[Non-Goals](#non-goals)), though the design allows adding read-only observer
clients in the future.

#### Output Routing

The server uses a `ResponseRenderer` ([RFD 026]) that forwards raw
`ChatResponse` events to the connected client as structured protocol messages.
When no client is connected, the server buffers events in memory (see [Rendering
Continuity](#rendering-continuity)).

The server sends structured data without formatting, styling, or rendering it,
leaving presentation to the client.
Without knowing terminal capabilities, the same server stream can feed a
terminal client rendering Markdown with ANSI styling, a pipeline client emitting
JSON, or a future observer client rendering a minimal progress view.

#### Rendering Continuity

When a client detaches and a new client re-attaches, the new client must produce
valid Markdown output from the current stream position.
The server supports this with an **event replay buffer** and per-client read
pointers.

##### Event replay buffer

The server appends all `ChatResponse` chunks to an in-memory buffer regardless
of whether a client is connected.
Whenever `EventBuilder` flushes and persists a complete event to disk, the
server clears the buffer and resets all client read pointers.
The buffer holds only the content between the last persisted event and the
current stream position, typically at most a few hundred tokens.

Because each persisted event ends on a complete Markdown document boundary, the
in-memory buffer always starts at a clean boundary that a fresh `jp_md::Buffer`
can render without prior context.
The client reads persisted events from disk only to show prior conversation
context, not for renderer correctness.

##### Per-client read pointers

When a client connects, the server creates a read pointer for it at the start of
the current buffer, sends everything from the pointer to the buffer head, and
advances the pointer.
As new chunks arrive, the server appends them to the buffer, sends them to all
connected clients, and advances each pointer.

When a client disconnects, the server retains its pointer keyed by client ID.
If the client reconnects before the buffer is flushed, it resumes from where it
left off without missing or duplicating data.
If the buffer was flushed while the client was disconnected, the server discards
the old pointer, creates a new one at the start of the current buffer, and
relies on the client to read the newly persisted events from disk.

Because the server always appends to the buffer whether or not a client is
attached, this model supports multiple clients without special cases.

##### Re-attach flow

1. **Read history from disk.** The client reads persisted events from disk and
   renders them locally for context.
2. **Connect and receive the buffer.** The server sends all buffered chunks from
   the client's read pointer (or from the start of the buffer for new clients),
   and the client pushes them into its `ResponseRenderer`.
3. **Live streaming.** Once the read pointer catches up to the buffer head, new
   chunks flow in as they arrive.
   Because the buffer is the leading edge of the live stream, the client's
   `jp_md::Buffer` processes the buffered chunks and the live stream as a single
   continuous Markdown document.

#### Inquiry Routing

The server implements `PromptBackend` as a socket-backed variant.
When the agent loop calls the prompt backend:

- **Client attached**: The server serializes the inquiry and sends it to the
  client over the socket.
  The client renders the prompt on its terminal, collects the answer, and sends
  it back for the server to deliver to the agent loop.
- **No client**: The server applies the detached prompt policy ([RFD 019]).
  Under the `defer` policy, it persists the incomplete turn ([RFD 023]) and
  shuts down.
  Under `auto`, `defaults`, or `deny`, it follows the existing behavior from
  [RFD 019].

#### Shutdown and Lifecycle

Because the server has no controlling terminal, it does not rely on OS signals
for lifecycle management.
All shutdown triggers arrive as protocol messages or execution outcomes, making
shutdown coordination cross-platform without Unix signals:

- **Turn completes**: The server exits normally.
- **`defer` policy triggers**: The server persists the incomplete turn and
  exits.
- **`conversation kill`**: The client connects to the IPC endpoint and sends a
  `Shutdown` message; the server persists its state and exits.
- **Unrecoverable error**: The server persists what it can and exits.

On exit, the server removes its process registry entry and IPC endpoint, and
closing the lock file releases the conversation lock automatically.

### Client Process

The client is a thin terminal adapter that does not run the agent loop.

#### Attach Loop

After connecting to the server socket, the client runs a message loop:

1. Read messages from the server (output chunks, inquiry requests, turn
   completion, and errors).
2. Write output chunks to the terminal.
3. On an inquiry request, render the prompt using `TerminalPromptBackend`,
   collect the answer, and send it to the server.
4. On Ctrl+C, send an interrupt message to the server, receive menu options,
   render the interrupt menu locally, and send the chosen action back.
5. On `TurnComplete`, render final output and exit.

#### Detach (Ctrl+Z)

When the user presses Ctrl+Z, the client:

1. Sends a `Disconnect` message to the server.
2. Prints "Detached: <conversation-id\>".
3. Exits.

To the server, Ctrl+Z is an ordinary client disconnect: the server continues
running without re-executing, losing state, or interrupting the LLM stream.
It preserves the client's read pointer in case the client reconnects, continues
sending events to any other connected clients (or buffering them if none
remain), and handles subsequent inquiries with the detached policy.

#### Interrupt (Ctrl+C)

Pressing Ctrl+C on the client does not disconnect; instead, it triggers the
interrupt protocol:

1. The client sends `Signal { kind: Interrupt }` to the server.
2. The server pauses streaming (matching the in-process SIGINT handler) and
   sends `InterruptMenu { options }` to the client.
3. The client renders the menu using `InterruptHandler`, collects the user's
   choice, and sends `InterruptAction { action }` to the server.
4. The server processes the action (Stop, Abort, Continue, Reply, Detach).
5. If the action is `Detach`, the server sends `Acknowledged` and the client
   disconnects as on Ctrl+Z.

### `conversation attach`

```sh
jp conversation attach <cid>
jp conversation attach              # session's active conversation
```

Connects to an already-running server process and enters the [client attach
loop](#attach-loop) without spawning a new server.

#### Context on Attach

When a client attaches to a server mid-stream, it reads persisted events from
disk, replays the server's in-memory event buffer, and switches to live
streaming (see [Rendering Continuity](#rendering-continuity)).
By default, the client renders events from the current in-progress turn:

| Flag       | Behavior                                 |
| ---------- | ---------------------------------------- |
| (default)  | Show events from the current in-progress |
|            | turn.                                    |
| `--tail=N` | Show the last N content events.          |
| `--tail=0` | Skip history and replay, connect to live |
|            | stream only.                             |

If no server is running for the conversation, `conversation attach` suggests
`--continue` when there is a pending inquiry (`IncompleteTurn`), or returns an
error if the conversation is idle.

### `conversation kill`

Connects to the server's IPC endpoint and sends a `Shutdown` message, causing
the server to persist its current state, release the conversation lock, and exit
cleanly.

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

Each registry entry records only the PID and start time; `conversation ls`
cleans up stale entries with dead PIDs.
Because the server exits once a detached query defers an inquiry, the
`waiting-for-input` state is derived from the conversation's `IncompleteTurn` on
disk rather than from the registry.

### `conversation ls` (extended)

`conversation ls` derives conversation status from the process registry and the
conversation stream:

| Status                     | Source                                   |
| -------------------------- | ---------------------------------------- |
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

Unix implements these traits over `tokio::net::UnixListener` and `UnixStream`,
and Windows implements them over `tokio::net::windows::named_pipe`.
The client connects via a corresponding `connect` function returning `impl
IpcStream`.

### Protocol

The client and server communicate using newline-delimited JSON (NDJSON) over the
IPC transport, with one connected client at a time.

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

If the version does not match, the server rejects the connection with an error
message.

`can_prompt` indicates whether the client can render interactive prompts
(`/dev/tty` available).
When `false`, the server treats the client as an output-only consumer and
applies the detached policy for inquiries.

The handshake omits terminal capabilities (width, ANSI support, output format)
because the server sends structured events and the client renders them for its
own output context.

#### Server → Client

| Message                         | When                                   |
| ------------------------------- | -------------------------------------- |
| `Event { kind, data }`          | Structured agent event (chat response, |
|                                 | tool call, progress)                   |
| `Inquiry { inquiry, metadata }` | Tool needs user input                  |
| `InterruptMenu { options }`     | Response to client's interrupt signal  |
| `TurnComplete`                  | Turn finished normally                 |
| `ProcessExiting { reason }`     | Server shutting down                   |

#### Client → Server

| Message                          | When                                  |
| -------------------------------- | ------------------------------------- |
| `InquiryResponse { id, answer }` | User answered an inquiry              |
| `Signal { kind }`                | Ctrl+C or other interrupt             |
| `InterruptAction { action }`     | User's menu choice                    |
| `Resize { width, height }`       | Terminal resized (reserved for future |
|                                  | use)                                  |
| `Disconnect`                     | Client detaching (Ctrl+Z)             |
| `Shutdown`                       | Clean shutdown request (`conversation |
|                                  | kill`)                                |

### The `defer` Detached Policy

This RFD carries forward the fourth detached policy from [RFD 024], renamed from
`queue` to `defer`:

| Mode        | Behavior                                      |
| ----------- | --------------------------------------------- |
| `auto`      | Auto-approve or route to LLM ([RFD 019]). |
| `defaults`  | Use default values ([RFD 019]).           |
| `deny`      | Fail the tool call ([RFD 019]).           |
| **`defer`** | **Persist the incomplete turn and exit.**     |

`defer` is the default policy when no client is attached.
The server lets other tools in the batch complete, persists their results
incrementally ([RFD 023]), persists the `InquiryRequest`, and exits.

Renaming `queue` to `defer` reflects that the inquiry is deferred to disk until
a user is available, not held in a running process queue.
The server exits cleanly, and `jp query --continue --id=<cid>` spawns a new
server and client to resume the turn.

### End-to-End Workflows

#### Normal Foreground Query

```sh
$ jp query "Refactor the auth module"
# Client spawns server, attaches, shows streaming output.
# Tools execute, inquiries are prompted interactively.
# Turn completes. Client exits.
```

#### Detached Query

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

#### Detach Mid-Execution (Ctrl+Z)

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

#### Detached Query Hits Inquiry

```sh
$ jp query --detach "Refactor auth"
Detached: jp-c17528832001

$ jp conversation ls
jp-c17528832001   Refactor auth   waiting-for-input (fs_modify_file)

$ jp query --continue --id=jp-c17528832001
# New server spawns, resumes the incomplete turn.
# Client attaches, prompts for the inquiry, continues.
```

#### Attach to a Running Query from Another Terminal

```sh
# Terminal 1:
$ jp query --detach "Long running task"
Detached: jp-c17528832001

# Terminal 2:
$ jp conversation attach jp-c17528832001
# Live output streams in. Inquiries are prompted here.
```

### Piped Execution

The client-server model is transparent to shell pipelines.
The client is the process in the pipeline, reading stdin and writing to stdout
and stderr, while the server remains invisible to the shell:

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

The client reads stdin before spawning the server and includes that input in the
`ChatRequest`; the server never touches the pipeline's stdin or stdout.

#### Output Routing in Pipelines

The server sends structured events (chat response chunks, tool call
notifications, and progress updates), and the client renders them for its local
output context:

- **Terminal stdout**: Markdown with ANSI styling on stdout, chrome on stderr.
- **Piped stdout**: Plain text on stdout, chrome on stderr.
- **`--format json`**: NDJSON on stdout.

#### Prompts in Pipelines

When stdout is piped but `/dev/tty` is available, the client still prompts
interactively.
It receives inquiry messages from the server over IPC and renders them on
`/dev/tty` via `TerminalPromptBackend` ([RFD 019]), keeping prompt I/O off the
pipeline's stdin and stdout.

When no terminal is available (CI, cron, `ssh -T`), the client sets `can_prompt
= false` in the handshake.
The server applies the detached policy for inquiries while the client continues
forwarding output to stdout.

#### SIGPIPE Handling

When a downstream process closes early (such as `jp query | head -10`), the
client receives `SIGPIPE` on its next write to stdout.
In an in-process model, `SIGPIPE` terminates JP and loses the in-progress turn;
in the client-server model, the server is a separate process and would otherwise
keep running.

To match standard pipeline behavior, the client stops the server by default and
provides a flag to preserve background execution:

- **Pipeline semantics (default)**: On `SIGPIPE`, the client sends `Shutdown` to
  the server before exiting so the entire pipeline stops cleanly.
- **Background semantics (`--persist-on-sigpipe`)**: The client disconnects
  without sending `Shutdown`, leaving the server to finish and persist the turn.

## Drawbacks

**Two processes for every query.** Even a simple `jp query "hello"` spawns a
server and a client.
Process creation (~10-50ms) and socket setup add a small overhead relative to
LLM latency.

**IPC is on the critical path.** All output flows through the IPC transport,
adding a hop before terminal rendering compared to direct stdout writes.
In practice, LLM token generation is the bottleneck rather than local IPC
throughput.

**Interrupt handling becomes a protocol exchange.** In-process Ctrl+C is a
synchronous sequence (signal, menu, action).
With a client and server, the client catches the signal, notifies the server,
receives the menu options, collects the user's choice, and sends the chosen
action back.
These round trips add minor latency before the menu appears and make interrupt
handling more complex than an in-process handler.

**Platform-specific code.** The IPC transport and process spawning require
platform-specific implementations (see [Platform
Portability](#platform-portability)).
Trait abstractions isolate this code, but each platform still requires testing.

**Upfront infrastructure.** Unlike the incremental approach in [RFD 024] and
[RFD 025] (foreground first, detach second, attach third), this model requires
the client-server infrastructure before detached queries work, making Phase 1
larger.

## Alternatives

### Two Execution Models (RFDs 024 + 025)

Run foreground queries in-process, run detached queries in a daemon that
persists and exits on inquiries, and layer live attachment on top via IPC.

Rejected because three execution modes require three code paths with different
behaviors, and mid-execution detach (Ctrl+Z) requires a fourth path that drops
the active LLM stream during re-exec.
The unified client-server model eliminates these separate paths.

### Spawn Server Only When Detaching

Start queries in-process, and re-exec as a daemon only on Ctrl+Z or `--detach`.

Rejected because re-executing drops the active LLM stream and in-memory state,
forcing the daemon to retry the LLM call or resume from disk.
Starting every query in a server process avoids this state loss.

### PTY Proxy (abduco Model)

Spawn a PTY, run the agent inside it as a subprocess, and proxy raw terminal I/O
over a socket so the agent is unaware of the client-server split.

Rejected because JP's I/O is structured (inquiries, interrupt menus, tool
prompts) rather than raw terminal bytes.
A PTY proxy discards that structure and requires parsing terminal output to
reconstruct it, whereas the NDJSON protocol preserves message semantics.

### tmux or screen Wrapper

Rely on users running JP inside `tmux` or `screen` for detach and reattach.

Rejected because an external multiplexer requires a separate dependency, does
not integrate with `conversation ls`, and cannot participate in the structured
inquiry protocol.

## Non-Goals

- **Multi-client attachment.** Each server accepts one client at a time.
  Supporting read-only observer clients or first-answer-wins inquiry handling
  requires extra coordination and is deferred to a future RFD.

- **Cross-machine attachment.** IPC sockets are local to the host; there is no
  network transport.

- **Public server API.** The socket protocol is internal and can change between
  releases without notice.

- **Non-query commands as servers.** Only `jp query` spawns a server;
  `conversation ls`, `conversation print`, and other commands run in-process.

## Risks and Open Questions

### Lock Handoff Timing

The client acquires the conversation lock before spawning the server.

On Unix, file descriptors without `FD_CLOEXEC` survive `fork()` and `exec()`.
The client opens the lock file, acquires `flock`, and spawns the server, which
inherits the locked fd across `fork`.
The client closes its copy of the fd after confirming that the server started,
transferring the lock without ever releasing it.
If the server fails to start, the client still holds the lock, reports the
error, and exits without leaving an orphaned lock.

On Windows, the client passes the lock file path to the server, which
re-acquires the lock via `LockFileEx`.
To mitigate the brief window between the client releasing the lock and the
server acquiring it, the server acquires the lock before signaling readiness to
the client; if another process takes the lock in that window, the server fails
to start and the client reports the error.

### Config and State Transfer

The client resolves config, builds the `ChatRequest`, and resolves tool
definitions before spawning the server.
Three options exist for passing this state to the server:

- **Temp file with inherited fd (chosen)**: The client writes the serialized
  state to a temp file and passes the open fd to the server.
  This handles arbitrary payload sizes without extra dependencies.
- **Command-line arguments**: Too limited for large configurations and tool
  definitions.
- **Shared memory**: Unnecessary complexity for a one-time startup transfer.

### Server Startup Latency

Before accepting connections, the server must read config from the temp file,
start the Tokio runtime, open the IPC socket, and connect to MCP servers.
To avoid polling for the socket file on fast queries, the server signals
readiness by writing a byte to an inherited pipe once the socket is open, and
the client blocks on that pipe before connecting.

### Terminal Resize

Because the client performs all rendering, it handles terminal resize events
(`SIGWINCH` on Unix, console events on Windows) locally by updating its
formatter's width without sending a protocol message.
The `Resize` message in the protocol table is reserved in case the server later
needs client dimensions to format tool output.

### Structured Output (`--format json`)

When `--format json` is set, the client's `ResponseRenderer` emits events as
NDJSON rather than formatted text; the server sends the same structured events
regardless of output format.

### Fallback for Restricted Environments

Containers with restricted IPC or strict sandboxes may block local sockets or
named pipes.
A `--no-server` flag can retain in-process execution as a compatibility
fallback, though it is not required for launch.

### Platform Portability

Platform-specific behavior is isolated to IPC transport, process spawning,
shutdown, PID checks, lock handoff, and terminal events:

**IPC transport.** Abstracted behind the `IpcListener` and `IpcStream` traits so
the NDJSON protocol layer remains transport-agnostic.

| Platform      | Transport           | Rust ecosystem                    |
| ------------- | ------------------- | --------------------------------- |
| macOS / Linux | Unix domain sockets | `tokio::net::UnixListener`        |
| Windows       | Named pipes         | `tokio::net::windows::named_pipe` |

The `interprocess` crate provides `LocalSocketListener` and `LocalSocketStream`
as a cross-platform abstraction, or JP can implement the traits directly per
platform.

**Process spawning.** The server must outlive the client.
On Unix, spawning uses the [double-fork pattern][double-fork]: fork once, call
`setsid()` in the child to detach from the controlling terminal as a new session
leader, and fork again so the intermediate process exits immediately and the
grandchild is reparented to PID 1.
On Windows, `CreateProcess` with detached creation flags achieves the same
result without forking.

| Platform | Mechanism                  | Rust                      |
| -------- | -------------------------- | ------------------------- |
| Unix     | Double-fork + `setsid`     | `nix::unistd`             |
| Windows  | `DETACHED_PROCESS` +       | `std::process::Command` + |
|          | `CREATE_NEW_PROCESS_GROUP` | `.creation_flags()`       |

**Shutdown.** Normal shutdown uses the `Shutdown` IPC message rather than OS
signals; `conversation kill --force` uses `SIGKILL` on Unix and
`TerminateProcess` on Windows.

**PID liveness.** Stale registry cleanup checks process liveness via `kill(pid,
0)` on Unix and `OpenProcess(SYNCHRONIZE, ...)` on Windows.

**Lock handoff.** Unix inherits `flock` file descriptors across `fork`; Windows
passes the lock file path and re-acquires the lock via `LockFileEx` (handled on
both platforms by the `fd-lock` crate, per [RFD 020]).

**Terminal events.** `crossterm` handles `SIGWINCH` on Unix and console events
on Windows inside the client; the server never interacts with a terminal.

## Implementation Plan

### Phase 1: Server Process and IPC

Implement the server process with platform-specific detached spawning, the
process registry, and the IPC listener (Unix sockets on Unix, named pipes on
Windows).
The server runs `jp_agent::run_turn_loop()` with sink writers when no client is
connected.
Wire `jp query --detach` to spawn the server and exit, `conversation kill` to
send `Shutdown` over IPC, and `conversation ls` to list running servers.

Foreground `jp query` still runs in-process without client attachment.
Depends on [RFD 026] (agent loop extraction).

### Phase 2: Client Attach Loop

Implement the client attach loop to connect to the server socket, render output,
forward inquiries, and handle disconnects, and wire `conversation attach` to use
it.
The server forwards structured events to the attached client and buffers events
for replay when no client is connected (see [Rendering
Continuity](#rendering-continuity)).
Foreground `jp query` still runs in-process.

### Phase 3: Interrupt Protocol

Implement the Ctrl+C interrupt exchange: the client sends `Signal`, the server
replies with `InterruptMenu`, and the client responds with `InterruptAction`.
Split `InterruptHandler` between the client (menu rendering) and the server
(action processing).

### Phase 4: Foreground Queries as Client-Server

Update foreground `jp query` (without `--detach`) to spawn a server and attach
immediately, replacing the in-process turn loop.
Add Ctrl+Z handling so the client sends `Disconnect` and exits.
Retain the in-process code path behind `--no-server` as a fallback.

### Phase 5: Cleanup

Remove the in-process turn loop from `jp_cli` (or keep it behind `--no-server`),
unify `--detach` with the foreground spawn path by skipping the attach step, and
update `--continue` to spawn a new server for resumed turns.

## References

- [RFD 026: Agent Loop Extraction][RFD 026]: `jp_agent` crate that the server
  calls.
- [RFD 023: Resumable Conversation Turns][RFD 023]: incomplete turn persistence;
  fallback when the `defer` (`queue`) policy triggers.
- [RFD 021: Printer Live Redirection][RFD 021]: `swap_writers()` remains useful
  for non-rendering output (tracing, diagnostics) but is not part of the
  client-server output model.
- [RFD 020: Parallel Conversations][RFD 020]: conversation locks inherited by
  the server.
- [RFD 019: Non-Interactive Mode][RFD 019]: detached prompt policies; `defer`
  (`queue`) is the default when no client is attached.
- [RFD 024: Detached Query Execution][RFD 024]: superseded; process registry and
  `defer` (`queue`) policy carried forward.
- [RFD 025: Live Re-Attachment][RFD 025]: superseded; socket protocol and attach
  flow carried forward.
- [abduco]: prior art for minimal session detach/reattach over Unix sockets.

[RFD 019]: 019-non-interactive-mode.md
[RFD 020]: 020-parallel-conversations.md
[RFD 021]: 021-printer-live-redirection.md
[RFD 023]: 023-resumable-conversation-turns.md
[RFD 024]: 024-detached-query-execution.md
[RFD 025]: 025-live-re-attachment-to-detached-queries.md
[RFD 026]: 026-agent-loop-extraction.md
[RFD 049]: 049-non-interactive-mode-and-detached-prompt-policy.md
[abduco]: https://github.com/martanne/abduco
[double-fork]: https://0xjet.github.io/3OHA/2022/04/11/post.html
