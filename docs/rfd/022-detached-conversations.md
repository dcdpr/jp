# RFD 022: Detached Conversations

- **Status**: Abandoned
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-19
- **Summary**: Proposes `jp query --detach`, a running-process registry, queued
  inquiry pausing, and live terminal attachment for background conversations.

> [!IMPORTANT]
> This RFD is **Abandoned**.
>
> Split and reorganised:
>
> - [RFD 023: Resumable Conversation Turns][RFD 023] (incomplete turn
>   persistence and `--continue`) remains active.
> - The detached execution and live re-attachment portions were superseded by
>   [RFD 027: Client-Server Query Architecture][RFD 027], which unifies both
>   under a single client-server model.
>
> The original text below is preserved for historical context.

## Summary

This RFD introduces background query execution (`jp query --detach`), a process
registry that tracks running conversations across terminal sessions, a `queue`
detached policy that pauses a conversation on an inquiry, and `jp conversation
attach` for streaming output and answering pending prompts.

It builds on [RFD 020] (Parallel Conversations) for per-session conversation
targeting, conversation locks, and `--id`, and on [RFD 021] (Printer Live
Redirection) for redirecting output during detach and attach.

## Motivation

[RFD 020] lets users run multiple conversations in parallel across terminal
sessions, with per-session tracking, `--id` targeting, and conversation locks.
Each running query still ties up a terminal until it finishes, and closing the
terminal kills the query.

[RFD 019] adds `auto`, `defaults`, and `deny` policies for non-interactive runs
in scripts and CI, where no user is present.
Neither RFD supports starting a long-running query, detaching while it works,
and reattaching later when a tool prompt requires human judgment.
Today, the user must keep the terminal open and wait.

Users also cannot inspect or interact with a query running in another terminal.

## Design

### Running Conversations

Every `jp query` invocation operates on a conversation ([RFD 020]).
The query process runs either **attached** to a terminal or **detached** in the
background.

| Command                          | Conversation    | Renders? | Can prompt? |
| -------------------------------- | --------------- | -------- | ----------- |
| `jp query "..."`                 | session default | Yes      | Yes         |
| `jp query --id=<cid> "..."`      | specified       | Yes      | Yes         |
| `jp query "..." \| less`         | session default | No       | Yes *       |
| `echo foo \| jp query \| script` | session default | No       | Maybe *     |
| `jp query --detach "..."`        | session default | No       | No          |
| `jp conversation attach <cid>`   | specified       | Via IPC  | Via IPC     |

**Renders?** indicates whether the process writes UI chrome (progress, tool
headers) to stderr.
**Can prompt?** indicates whether it can ask interactive questions via
`/dev/tty` ([RFD 019]).
Entries marked `*` depend on `/dev/tty` availability, which remains accessible
from an interactive terminal even when stdin or stdout is piped.

This RFD does not change conversation persistence.
Completed turns, tool call requests and responses, configuration deltas, and
metadata continue to be written to `.jp/conversations/<cid>/` after every turn.
Instead, this RFD introduces **process state**: ephemeral runtime metadata (PID,
socket, status, pending inquiry) that exists only while the query process is
running and is deleted when it exits.

### The `queue` Detached Policy

This RFD adds a fourth detached policy mode to the three defined in [RFD 019]:

| Mode        | Behavior                                              |
| ----------- | ----------------------------------------------------- |
| `auto`      | Auto-approve or route to LLM (from [RFD 019]).    |
| `defaults`  | Use default values (from [RFD 019]).              |
| `deny`      | Fail the tool call (from [RFD 019]).              |
| **`queue`** | **Pause the conversation, persist the prompt, wait.** |

When `queue` is active and an inquiry arrives without an attached client, JP
records the pending inquiry in the process registry and pauses the conversation
until a user attaches and answers:

```sh
$ jp conversation ls
ID                TITLE            STATUS
jp-c17528832001   Refactor auth    waiting-for-input (RunTool: fs_modify_file)
jp-c17528831500   Fix tests        running (pid 12345)
jp-c17528831000   Debug service    idle

$ jp conversation attach jp-c17528832001
> Run local fs_modify_file tool? [y/n]: y
[jp-c17528832001] Resumed.
```

`queue` replaces `deny` ([RFD 019]) as the **default detached policy**, so tool
calls never run unattended unless the user explicitly configures `detached =
"auto"`.

Detachment itself requires `--detach`, which spawns a background process and
exits.
A missing TTY does not imply detachment: piped invocations (`echo foo | jp query
| cat`) still run in the foreground under the calling script or pipeline.

### CLI Interface

#### `jp query --detach`

`jp query --detach` spawns a background process for the conversation, registers
it in the process registry, and exits immediately while the query continues in
the background:

```sh
jp query --detach "Refactor the auth module"
```

`--detach` combines with the conversation targeting flags from [RFD 020], and
the same targeting and lock errors apply:

```sh
jp query --detach --id=jp-c17528832001 "Continue this in the background"
jp query --detach --new "Start something new in the background"
```

#### `jp conversation attach [<cid>]`

Connects the user's terminal to the running process for a conversation to stream
output and answer pending inquiries:

```sh
jp conversation attach jp-c17528832001
jp conversation attach jp-c17528832001 --tail=5
jp conversation attach                          # session's active conversation
```

On attach, the client replays recent events from disk, flushes any partial
rendering output buffered in memory (see [Output on Attach](#output-on-attach)),
and switches to live streaming.

`--tail=N` controls how many persisted content events are replayed on attach,
skipping structural markers such as `TurnStart` and `ConfigDelta`:

| Flag       | Behavior                                 |
| ---------- | ---------------------------------------- |
| (no flag)  | Show the current in-progress turn.       |
| `--tail=N` | Show the last N content events, plus the |
|            | current in-progress turn.                |
| `--tail=0` | Show only the pending inquiry prompt. No |
|            | context.                                 |

`attach` opens a read-write connection to the running process over its IPC
socket rather than starting a new query, so there is no `--attach` flag on `jp
query`.

#### `jp conversation ls` (extended)

Merges persisted conversation metadata from `.jp/conversations/` with ephemeral
state from the process registry to show each conversation's status:

- `idle`: no running process
- `running (pid NNN)`: process active, no pending inquiry
- `waiting-for-input (inquiry kind)`: process paused on a pending inquiry

#### `jp conversation kill <cid>`

Sends a cancellation signal to the running process, removes its registry entry
and socket, and releases the conversation lock ([RFD 020]).
Persisted conversation data in `.jp/conversations/` is untouched.

#### `jp conversation show <cid>` (extended)

Includes process status when the conversation has a running process.

### Process Registry

Running conversations register in the user-local data directory:

```
~/.local/share/jp/workspace/<workspace-id>/processes/
├── <conversation-id>.json    # process metadata
└── <conversation-id>.sock    # IPC socket
```

The workspace ID scopes the registry per project.
Entries are keyed by conversation ID, since the conversation lock ([RFD 020])
allows at most one running process per conversation.

#### Process Entry Format

```json
{
  "conversation_id": "jp-c17528832001",
  "pid": 12345,
  "workspace_id": "a1b2c3",
  "started_at": "2025-07-19T14:30:00.000Z",
  "status": "waiting_for_input",
  "pending_inquiry": {
    "kind": "RunTool",
    "tool_name": "fs_modify_file",
    "answer_type": "boolean"
  }
}
```

#### Stale Entry Cleanup

`jp conversation ls` checks PID liveness for each entry and removes the metadata
file and socket if the process is dead.
This recovers from unclean exits (SIGKILL, machine reboot, power loss) using the
same background cleanup task as the lock and session files in [RFD 020].

### Attach Protocol

Each running conversation listens on its Unix domain socket
(`~/.local/share/jp/workspace/<workspace-id>/processes/<conversation-id>.sock`)
and communicates over newline-delimited JSON.

While attached, the client receives live streaming output (tool results and LLM
responses) and handles inquiries through the following loop:

1. The server sends the pending inquiry (serialized `Inquiry` and metadata).
2. The client renders the inquiry and collects the user's answer.
3. The client sends the answer back to the server.
4. The server resumes tool execution with the answer, repeating from step 1 if
   further inquiries arrive.

The client can disconnect at any time, returning the conversation to its
configured detached policy.

### Conversation Locks

A detached process holds the conversation lock ([RFD 020]) for its entire run,
preventing other sessions from starting a query on the same conversation.
Attaching does not acquire a lock; the client connects over the socket to the
background process that already holds it.
`jp conversation kill` terminates that process and releases the lock for new
queries.

### Prompt Routing

`route_prompt` from [RFD 019] gains a `DetachedMode::Queue` branch:

```rust
fn route_prompt(
    inquiry: &Inquiry,
    has_client: bool,
    policy: DetachedMode,
    config_exclusive: Option<bool>,
) -> PromptAction {
    if has_client {
        return PromptAction::PromptClient;
    }

    let exclusive = config_exclusive.unwrap_or_else(|| inquiry.exclusive());

    match policy {
        DetachedMode::Queue => PromptAction::Queue,
        DetachedMode::Auto if exclusive => PromptAction::Fail,
        DetachedMode::Auto => match inquiry {
            Inquiry::RunTool { .. } => PromptAction::AutoApprove,
            Inquiry::DeliverToolResult { .. } => PromptAction::AutoDeliver,
            Inquiry::ToolQuestion { .. } => PromptAction::LlmInquiry,
        },
        DetachedMode::Defaults => PromptAction::UseDefault,
        DetachedMode::Deny => PromptAction::Fail,
    }
}
```

On `PromptAction::Queue`, the tool coordinator serializes the inquiry to the
process registry and suspends the tool in the `AwaitingInput` state ([RFD 009])
until an attached client answers.

## Drawbacks

- **Infrastructure complexity.** The process registry, Unix domain sockets, and
  IPC protocol add substantial machinery to pause a query and wait for user
  input.
- **Platform constraints.** The initial implementation targets macOS and Linux
  only; supporting Windows requires named pipes or another IPC transport in
  place of Unix domain sockets.
- **Process lifecycle management.** `--detach` requires daemonization (re-exec
  or double-fork), signal handling, and stdio redirection.
- **New failure modes.** Stale sockets, permission errors, and filesystem limits
  introduce failure cases that require clear recovery and error messages.

## Alternatives

### Separate Task Abstraction

A `jp task` subcommand could assign each `jp query` run a task ID distinct from
its conversation ID, with a separate task registry mapping tasks to
conversations.

This adds a second identifier without adding capability.
Because the conversation is already the unit of work and allows at most one
active process, runtime state belongs on the conversation itself rather than on
a separate task entity.

### Local HTTP Server for IPC

Using a local HTTP server instead of Unix domain sockets would require port
allocation and firewall handling for local-only traffic.
Unix domain sockets are lighter and enforce access control through filesystem
permissions.

### Process Registry in `.jp/`

Storing the process registry in `.jp/processes/` alongside conversations would
place ephemeral, machine-local state inside a directory meant for version
control.
The user data directory (`~/.local/share/jp/workspace/`) avoids polluting the
repository.

## Non-Goals

- **Crash resume.** If a detached process dies, its stale registry entry is
  cleaned up.
  Automatically spawning a replacement process to resume from persisted
  conversation state is out of scope; users can start a new query manually.
- **Multi-client attachment.** Only one client can attach to a conversation at a
  time.
  Coordinating output streaming and prompt answers across concurrent clients is
  out of scope.
- **Non-query commands.** Only `jp query` registers a running process; other
  subcommands exit immediately and do not use the process registry.
- **Sub-agent support.** While the process model is compatible with sub-agents,
  this RFD does not propose agent infrastructure.
- **Cross-machine attachment.** Processes and IPC sockets are local to the host
  machine.

## Risks and Open Questions

### Lock Contention on a Detached Conversation

When a user runs `jp query --id=<cid> "new question"` while a detached process
is already running for that conversation, the conversation lock ([RFD 020])
rejects the new query.
The lock contention error should suggest attaching (`--attach` / `jp
conversation attach`) alongside the standard [RFD 020] options (`--fork`,
`--new`, `jp conversation kill`):

```txt
Error: Conversation jp-c17528832001 is locked by pid 12345 (detached).

Suggestions:
    jp conversation attach jp-c17528832001   connect to the running process.
    --fork     branch from this conversation.
    --new      start a new conversation.
    jp conversation kill jp-c17528832001     terminate the running process.
```

### Attaching During Piped Execution

If a user attaches (`jp query --id=<cid> --attach`) while a piped command such
as `echo foo | jp query | script` is running, two views of the conversation
exist at once: the piped process continues writing final output to `script` on
stdout, while the attached client receives interactive rendering over the
socket.
Although potentially surprising, allowing attachment here lets a user answer a
prompt that blocks a piped query.

### Daemonization Strategy

Detaching the query process from the controlling terminal has three main
options:

1. **Double-fork**: the classic Unix daemon pattern.
2. **`nohup` + background**: simpler, but offers less control over signals.
3. **Re-exec**: `jp query --detach` spawns a detached child process with an
   internal flag and exits immediately.

Re-exec is the cleanest approach: the child process inherits the resolved
conversation state and runs independently after the parent exits.

### Output on Attach

An attaching client needs enough context to show the conversation's state,
especially when a `waiting-for-input` prompt requires a decision.
Attach output proceeds in three steps:

1. **Replay from disk.** Read persisted events from the conversation stream on
   disk and render them using the `jp conversation print` pipeline.
   `--tail` controls how many events are replayed (defaulting to the in-progress
   turn).
   Because the stream is flushed to disk after every streaming and tool
   execution phase, all events up to and including a pending `InquiryRequest`
   are already on disk when the conversation enters `waiting-for-input`.

2. **Flush the memory buffer.** While detached, the `Printer` writes to a 1 MB
   in-memory buffer via `swap_writers()` ([RFD 021]).
   This buffer holds rendered output from the active streaming cycle that has
   passed through `ChatResponseRenderer` and other renderers but has not yet
   been persisted as a complete event.
   On attach, the buffer is flushed to the client's terminal; if output exceeded
   1 MB, JP prints a truncation notice directing the user to `jp conversation
   print`.

3. **Swap back to the terminal.** `Printer::swap_writers()` switches output from
   the memory buffer to the client's terminal writer, and subsequent streaming
   output goes directly to the client.

This flow reuses existing components: `EventBuilder` for accumulating chunks and
flushing complete events, `ChatResponseRenderer` and its peer renderers for
stateful markdown formatting across writer swaps, `conversation print` for disk
replay, and `Printer::swap_writers()` ([RFD 021]) for writer redirection.
The only new piece is the in-memory buffer; on detach, the caller runs
`flush_instant()` before `swap_writers(memory_buffer)` to minimize the
transition window.

### Default Detached Policy Change

Changing the default detached policy from `deny` ([RFD 019]) to `queue` causes
piped queries that hit a prompt to pause and wait rather than fail immediately.
Users who rely on fast failure in non-interactive scripts can set `detached =
"deny"`.

## Implementation Plan

### Phase 1: Process Registry (Read-Only)

Write a process entry (PID, status, conversation ID) to the user-local data
directory on every `jp query`, display process status in `jp conversation ls`,
and clean up stale entries via PID liveness checks.
This phase adds status visibility without IPC, attach, or `--detach`, and can
merge independently.

### Phase 2: Prompt Queue

Implement the `queue` detached policy in the tool coordinator so that inquiries
without an attached client are serialized to the process registry and tool
execution is suspended.
Until Phase 3 adds IPC, queued conversations remain paused until the process
exits or is killed.
Depends on Phase 1 and [RFD 019] Phase 2 (routing integration).

### Phase 3: Attach IPC

Add a Unix domain socket listener and JSON-lines protocol to each running
conversation.
`jp query --id=<cid> --attach` connects to the socket, forwards streaming output
to the client, and sends inquiry answers to resume the conversation.
Depends on Phase 2.

### Phase 4: Detach Mode

Implement `jp query --detach` to daemonize the query process, register it, and
exit the CLI immediately, along with `jp conversation kill <cid>` to terminate
detached processes and clean up their state.
Depends on Phase 3 and targets Unix initially.

## References

- [RFD 021: Printer Live Redirection][RFD 021]: runtime output redirection via
  `swap_writers()`, used for the detach/attach memory buffer.
- [RFD 020: Parallel Conversations][RFD 020]: per-session conversation tracking
  and conversation locks, a prerequisite for this RFD.
- [RFD 019: Non-Interactive Mode][RFD 019]: defines the `auto`, `defaults`, and
  `deny` policies that `queue` extends.
- [RFD 018: Typed Inquiry System][RFD 018]: defines the `Inquiry` enum used to
  serialize pending prompts.
- [RFD 005: First-Class Inquiry Events][RFD 005]: persists `InquiryRequest` and
  `InquiryResponse` events.
- [RFD 009: Stateful Tool Protocol][RFD 009]: long-running tool handles whose
  `AwaitingInput` state supports prompt queuing.
- `tmux` session model: precedent for background sessions with attach and
  detach.
- `docker attach`: precedent for connecting a terminal to a running container.

[RFD 005]: 005-first-class-inquiry-events.md
[RFD 009]: 009-stateful-tool-protocol.md
[RFD 018]: 018-typed-prompt-routing-enum.md
[RFD 019]: 019-non-interactive-mode.md
[RFD 020]: 020-parallel-conversations.md
[RFD 021]: 021-printer-live-redirection.md
[RFD 023]: 023-resumable-conversation-turns.md
[RFD 027]: 027-client-server-query-architecture.md
