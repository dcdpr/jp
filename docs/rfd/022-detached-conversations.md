# RFD 022: Detached Conversations

- **Status**: Abandoned
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-19

> [!IMPORTANT]
> This RFD is **Abandoned**.
>
> Split and reorganised:
>
> - [RFD 023: Resumable Conversation Turns][RFD 023] — incomplete turn
>   persistence and `--continue` — remains active.
> - The detached execution and live re-attachment portions were superseded by
>   [RFD 027: Client-Server Query Architecture][RFD 027], which unifies both
>   under a single client-server model.
>
> The original text below is preserved for historical context.

## Summary

This RFD introduces background execution for conversations via `jp query
--detach`, a process registry for visibility into running conversations across
terminal sessions, a `queue` detached prompt policy that pauses conversations
when they hit inquiries, and conversation attachment for monitoring output and
answering pending prompts.

This RFD depends on [RFD 020] (Parallel Conversations) for per-session
conversation targeting, conversation locks, and the `--id` flag, and on [RFD
021] (Printer Live Redirection) for runtime output redirection during
detach/attach.

## Motivation

[RFD 020] introduces parallel conversations: per-session conversation tracking,
`--id` for explicit targeting, and conversation locks to prevent concurrent
mutations. With those foundations in place, a user can run multiple
conversations simultaneously across terminal sessions.

But each conversation still requires a terminal for the duration of the query.
Long-running queries with tool calls occupy a terminal tab until they complete.
If a query hits a prompt that requires user judgment, the terminal blocks until
the user answers. If the user closes the terminal, the query dies.

[RFD 019] introduces non-interactive mode with `auto`, `defaults`, and `deny`
detached policies. These cover scripting and CI use cases where no user is
available. But they don't address a common workflow:

1. User starts a long-running query.
2. Query hits a tool prompt requiring user judgment.
3. User wants to come back later and answer the prompt.

Today, the user must keep the terminal open and wait. There is no way to detach
from a running query and reattach later.

Additionally, users have limited visibility into running queries. If a query is
running in another terminal, there's no way to check its status or interact with
it from elsewhere.

## Design

### Running Conversations

Every `jp query` invocation operates on a conversation (per [RFD 020]). The
process running the query can be **attached** to a terminal or **detached**
(running in the background).

| Command                                  | Conversation    | Renders? | Can prompt? |
|------------------------------------------|-----------------|----------|-------------|
| `jp query "..."`                         | session default | Yes      | Yes         |
| `jp query --id=<cid> "..."`              | specified       | Yes      | Yes         |
| `jp query "..." \| less`                 | session default | No       | Yes *       |
| `echo foo \| jp query \| script`         | session default | No       | Maybe *     |
| `jp query --detach "..."`                | session default | No       | No          |
| `jp conversation attach <cid>`           | specified       | Via IPC  | Via IPC     |

**Renders?** indicates whether the process writes chrome (progress, tool
headers) to the user's terminal via stderr. **Can prompt?** indicates whether
the process can ask the user interactive questions via `/dev/tty` (per [RFD
019]). Entries marked * depend on `/dev/tty` availability — if the user is at a
terminal, `/dev/tty` works even when stdin/stdout are piped.

The process is ephemeral runtime state — PID, socket, streaming state. When the
query completes or is interrupted, the process exits. The conversation state
persists in `.jp/conversations/<cid>/` as always.

### The `queue` Detached Policy

This RFD adds a fourth detached policy mode to the three defined in [RFD 019]:

| Mode        | Behavior                                                    |
|-------------|-------------------------------------------------------------|
| `auto`      | Auto-approve or route to LLM (from [RFD 019]).              |
| `defaults`  | Use default values (from [RFD 019]).                        |
| `deny`      | Fail the tool call (from [RFD 019]).                        |
| **`queue`** | **Pause the conversation, persist the prompt, wait.**       |

When `queue` is active and an inquiry arrives without an attached client, the
conversation pauses. The pending inquiry is written to the process registry. The
user answers it by attaching:

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

With `queue` available, it becomes the **default detached policy** — replacing
`deny` from [RFD 019]. Nothing runs unattended unless explicitly configured.
Users who want automation set `detached = "auto"` in their config.

`--detach` is an explicit opt-in. Piped execution (`echo foo | jp query | cat`)
is **not** detached — the process runs in the foreground, owned by the script or
pipeline. `--detach` means "spawn a background process and exit"; the absence of
a TTY does not imply detachment.

### CLI Interface

#### `jp query --detach`

Spawns a background process for the conversation, registers it in the process
registry, and exits immediately. The conversation continues in the background.

```sh
jp query --detach "Refactor the auth module"
```

Combines with all conversation targeting flags from [RFD 020]:

```sh
jp query --detach --id=jp-c17528832001 "Continue this in the background"
jp query --detach --new "Start something new in the background"
```

All conversation targeting and lock errors from [RFD 020] apply.

#### `jp conversation attach [<cid>]`

Connects to the running process for the specified conversation. The user's
terminal receives streaming output and can answer pending inquiries.

```sh
jp conversation attach jp-c17528832001
jp conversation attach jp-c17528832001 --tail=5
jp conversation attach                          # session's active conversation
```

On attach, the client replays recent context from the persisted conversation
events on disk, then flushes any partial rendering output buffered in memory
(see [Output on Attach](#output-on-attach)), and finally switches to live
streaming.

`--tail=N` controls how many persisted events are shown on attach. Only content
events are counted — structural markers like `TurnStart` and `ConfigDelta` are
skipped. Without `--tail`, the default is to show all events from the current
(in-progress) turn, giving the user enough context to understand the
conversation's current state.

| Flag       | Behavior                                 |
|------------|------------------------------------------|
| (no flag)  | Show the current in-progress turn.       |
| `--tail=N` | Show the last N content events, plus the |
|            | current in-progress turn.                |
| `--tail=0` | Show only the pending inquiry prompt. No |
|            | context.                                 |

Attach is a read-write connection to the running process via the IPC socket. It
is not a new query — there is no `--attach` flag on `jp query`.

#### `jp conversation ls` (extended)

Shows all conversations with their process status:

- `idle` — no running process
- `running (pid NNN)` — process active, no pending inquiry
- `waiting-for-input (inquiry kind)` — process paused, inquiry pending

The output merges persisted conversation metadata from `.jp/conversations/` with
ephemeral process state from the process registry.

#### `jp conversation kill <cid>`

Sends a cancellation signal to the running process, cleans up the registry entry
and socket. The conversation data remains intact — this only terminates the
process, which releases the conversation lock ([RFD 020]).

#### `jp conversation show <cid>` (extended)

Includes process status if the conversation has a running process.

### Process Registry

Running conversations register in the user-local data directory:

```
~/.local/share/jp/workspace/<workspace-id>/processes/
├── <conversation-id>.json    # process metadata
└── <conversation-id>.sock    # IPC socket
```

The workspace ID scopes the registry to avoid collisions between projects. The
conversation ID is the key — there can be at most one running process per
conversation (enforced by the conversation lock from [RFD 020]).

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

`jp conversation ls` checks PID liveness for each entry. If the process is dead,
the entry and socket are removed. This handles cases where JP crashes without
cleaning up (SIGKILL, machine reboot, power loss). This uses the same background
task cleanup approach as [RFD 020]'s lock and session file cleanup.

### Attach Protocol

Each running conversation listens on a Unix domain socket at
`~/.local/share/jp/workspace/<workspace-id>/processes/<conversation-id>.sock`.

When a user attaches, JP connects to the socket and enters a protocol:

1. Server sends pending inquiry (serialized `Inquiry` + metadata).
2. Client renders the inquiry and collects the user's answer.
3. Client sends the answer back.
4. Server resumes the tool with the answer.
5. If more inquiries arrive during execution, repeat from step 1.
6. Client can disconnect at any time (conversation reverts to detached mode).

The protocol uses newline-delimited JSON over the socket.

When attached, the client receives streaming output (tool results, LLM
responses) in real time. When the client disconnects, the conversation continues
with whatever detached policy is configured.

### Relationship to Conversation Locks

A detached conversation holds the conversation lock ([RFD 020]) for the duration
of its execution. This prevents other sessions from starting a new query on the
same conversation while it runs in the background.

Attaching to a detached conversation does not acquire a new lock — the attach
client communicates with the lock-holding process via the socket. The lock
holder is the background process, not the attach client.

Killing a detached conversation (`jp conversation kill`) terminates the process,
which releases the lock. The conversation is then available for new queries.

### Integration with Prompt Routing

The `route_prompt` function from [RFD 019] is extended with the `queue` policy:

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

`PromptAction::Queue` causes the tool coordinator to serialize the inquiry,
write it to the process registry, and suspend tool execution. The tool's state
machine enters `AwaitingInput` (already supported from [RFD 009]). When a client
attaches and answers, execution resumes.

### Relationship to Conversation State

Conversation state is **already persisted** in `.jp/conversations/<cid>/`. This
includes all completed turns, tool call requests and responses, configuration
deltas, and metadata. The persisted state is written after every turn.

**This RFD does not change conversation persistence.** It introduces **process
state** — ephemeral runtime information about the running process (PID, socket,
status, pending inquiry). Process state exists only while the process runs. When
the process exits, the process state is deleted. The conversation state remains.

## Drawbacks

**Infrastructure complexity.** The process registry, Unix domain sockets, and
IPC protocol add significant new infrastructure for what starts as "pause and
wait for user input."

**Platform constraints.** Unix domain sockets are not available on Windows. The
initial implementation targets macOS and Linux only. Windows support requires
named pipes or a different IPC mechanism.

**Process lifecycle management.** `--detach` requires proper daemonization:
double-fork or re-exec, signal handling, stdout/stderr redirection. Non-trivial
to implement correctly.

**New failure modes.** Stale sockets, permission issues, and filesystem limits
are new error cases that need robust handling and clear user-facing messages.

## Alternatives

### Introduce a separate task abstraction

Add `jp task` as a subcommand with task IDs distinct from conversation IDs.
Every `jp query` spawns a task. Tasks have their own registry keyed by task ID,
and each task maps to a conversation.

Rejected because it adds a concept without adding capability. Users would need
to understand the relationship between tasks and conversations ("this task is
running on that conversation"). The conversation is already the unit of work.
Process state is runtime metadata about a conversation, not a first-class
entity.

"What conversations are running?" is clearer than "What tasks exist and which
conversations do they operate on?"

### HTTP server for IPC

Use a local HTTP server for attach instead of Unix domain sockets.

Rejected because it requires port allocation, firewall considerations, and is
heavier than needed. Unix domain sockets are the standard local IPC mechanism on
Unix and have better security properties (filesystem permissions).

### Store process registry in `.jp/`

Put the process registry in `.jp/processes/` alongside conversations.

Rejected because `.jp/` is typically committed to version control. Process state
is ephemeral, machine-local, and user-local. The user data directory
(`~/.local/share/jp/workspace/`) is the correct location.

## Non-Goals

- **Crash resume.** If a detached process dies (crash, SIGKILL, machine reboot),
  the process registry entry becomes stale and is cleaned up. Automatically
  spawning a new process that resumes from persisted conversation state is a
  potential future enhancement but out of scope. Users can start a new query on
  the conversation manually.

- **Multi-client attachment.** Only one client can attach to a conversation at a
  time. Concurrent attachment would require coordination (which client receives
  streaming chunks?) and is out of scope.

- **Non-query commands.** Only `jp query` creates running processes. Other
  subcommands are instant and do not interact with the process registry.

- **Sub-agent support.** The process model is compatible with future sub-agents
  but this RFD does not propose agent infrastructure.

- **Cross-machine attachment.** Processes are local to the machine. No network
  IPC.

## Risks and Open Questions

### Collision when targeting a running conversation

A user runs `jp query --id=<cid> "new question"` but a background process is
already running for that conversation. The conversation lock ([RFD 020]) blocks
the new query with a lock contention error. The error message should mention
`--attach` as an option in addition to the standard suggestions from [RFD 020]
(fork, new, kill):

```txt
Error: Conversation jp-c17528832001 is locked by pid 12345 (detached).

Suggestions:
    jp conversation attach jp-c17528832001   connect to the running process.
    --fork     branch from this conversation.
    --new      start a new conversation.
    jp conversation kill jp-c17528832001     terminate the running process.
```

### Attach during piped execution

When `echo foo | jp query | script` is running and a user runs `jp query
--id=<cid> --attach`, the conversation gains an attached client. But the
original piped process still owns stdout (piped to `script`). The attach client
gets a separate rendering channel via the socket.

Two "views" of the same conversation exist briefly. The piped process writes
final output to stdout. The attached client sees interactive rendering via the
socket. This is consistent with the model but may be surprising.

Blocking attachment to conversations with an existing client would prevent a
useful workflow (piped execution hits a prompt, user attaches to answer it).

### Daemonization strategy

`--detach` needs to detach the process from the terminal. Options:

1. **Double-fork** (classic Unix daemon pattern).
2. **`nohup` + background** (simpler but less control over signals).
3. **Re-exec** (`jp query --detach` re-executes itself with an internal flag and
   exits).

The third option is cleanest. `jp query --detach` spawns a child process that
outlives the parent, inherits conversation state, and runs independently. The
parent exits immediately after spawning.

### Output on Attach

When a client attaches to a running conversation, it needs enough context to
understand the current state — especially if the conversation is
`waiting-for-input` and the user needs to decide how to answer a prompt.

The attach flow has three steps:

1. **Replay from disk.** Read persisted events from the conversation's event
   stream on disk and render them using the same rendering logic as `jp
   conversation print`. The number of events shown is controlled by `--tail`
   (default: current in-progress turn). The conversation stream is persisted
   after every streaming phase and every tool execution phase, so by the time a
   conversation is `waiting-for-input`, all events up to and including the
   pending `InquiryRequest` are on disk.

2. **Flush the memory buffer.** While detached, the `Printer` writes to an
   in-memory buffer instead of the terminal (via [RFD 021]'s `swap_writers()`).
   This buffer captures rendered output from the current streaming cycle —
   content that has passed through `ChatResponseRenderer` and other renderers
   but hasn't been persisted as a complete event yet. On attach, this buffer is
   flushed to the client's terminal. The buffer has a hard-coded 1 MB capacity;
   if exceeded, a truncation notice is shown and the user can run `jp
   conversation print` for full history.

3. **Swap back to terminal.** The `Printer` is swapped from the memory buffer
   back to the terminal writer. From this point, all renderer output goes
   directly to the client's terminal via the normal streaming path.

This design reuses existing infrastructure:

- `EventBuilder` already accumulates partial chunks and flushes complete events.
- `ChatResponseRenderer` and other renderers already track their internal
  markdown/formatting state, so they continue correctly after the swap.
- `conversation print` already renders event streams from disk.
- [RFD 021]'s `Printer::swap_writers()` handles the output redirection.

The only new component is the in-memory buffer passed to `swap_writers()` on
detach. On detach, the caller does `flush_instant()` followed by
`swap_writers(memory_buffer)` to minimize the transition window.

### Default detached policy change

This RFD proposes changing the default detached policy from `deny` ([RFD 019])
to `queue`. Piped queries that hit prompts will pause and wait instead of
failing.

This is safer (nothing auto-approves) but changes behavior for users who rely on
prompts failing fast in non-interactive contexts. Users who want the old
behavior set `detached = "deny"`.

## Implementation Plan

### Phase 1: Process Registry (Read-Only)

Every `jp query` writes a process entry to the user-local data directory. The
entry contains PID, status, and conversation ID. `jp conversation ls` reads the
registry and displays process status alongside conversation metadata.

No IPC, no attach, no `--detach` yet. Just visibility.

Stale entry cleanup via PID liveness checks.

Can be merged independently.

### Phase 2: Prompt Queue

Implement the `queue` detached policy in the tool coordinator. When an inquiry
arrives without an attached client and the policy is `queue`, serialize the
inquiry to the process registry and suspend tool execution.

No way to answer yet (no IPC) — the conversation stays paused until the process
is killed or exits.

Depends on Phase 1 and [RFD 019] Phase 2 (routing integration).

### Phase 3: Attach IPC

Add Unix domain socket listener to each running conversation. Implement the
JSON-lines protocol. `jp query --id=<cid> --attach` connects, receives pending
inquiries, sends answers. The conversation resumes.

Streaming output is forwarded to the attached client.

Depends on Phase 2.

### Phase 4: Detach Mode

Implement `jp query --detach`. Daemonize the process, register it, exit the CLI
immediately. The conversation runs in the background.

Implement `jp conversation kill <cid>` for cleanup.

Depends on Phase 3. Unix-only initially.

## References

- [RFD 021: Printer Live Redirection][RFD 021] — runtime output redirection via
  `swap_writers()`; used for the detach/attach memory buffer.
- [RFD 020: Parallel Conversations][RFD 020] — per-session conversation tracking
  and conversation locks; prerequisite for this RFD.
- [RFD 019: Non-Interactive Mode][RFD 019] — defines the `auto`, `defaults`, and
  `deny` policies; this RFD adds `queue`.
- [RFD 018: Typed Inquiry System][RFD 018] — the `Inquiry` enum used for enum
  used for serializing pending prompts.
- [RFD 005: First-Class Inquiry Events][RFD 005] — persisting
  `InquiryRequest`/`InquiryResponse` events.
- [RFD 009: Stateful Tool Protocol][RFD 009] — long-running tool handles;
  `AwaitingInput` state supports prompt queuing.
- tmux session model — precedent for background sessions with attach/detach.
- `docker attach` — precedent for connecting a terminal to a running container.

[RFD 005]: 005-first-class-inquiry-events.md
[RFD 009]: 009-stateful-tool-protocol.md
[RFD 018]: 018-typed-prompt-routing-enum.md
[RFD 019]: 019-non-interactive-mode.md
[RFD 020]: 020-parallel-conversations.md
[RFD 021]: 021-printer-live-redirection.md
[RFD 023]: 023-resumable-conversation-turns.md
[RFD 027]: 027-client-server-query-architecture.md
