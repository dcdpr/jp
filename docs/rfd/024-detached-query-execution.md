# RFD 024: Detached Query Execution

- **Status**: Abandoned
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-19

> [!IMPORTANT]
> This RFD is **Abandoned**.
>
> Superseded by [RFD 027: Client-Server Query Architecture][RFD 027], which
> unifies detached execution, live attachment, and foreground queries under a
> single client-server model. The process registry and `queue` policy from this
> RFD are carried forward into RFD 027 (the `queue` policy is renamed to
> `defer`).
>
> The original text below is preserved for historical context.

## Summary

This RFD introduces `jp query --detach` for running conversations in the
background, a process registry for visibility into running queries, and the
`queue` detached policy that persists an incomplete turn and exits when an
inquiry requires user input. Resumption after exit uses `--continue` from [RFD
023].

This RFD depends on [RFD 023] (Resumable Conversation Turns) for incomplete turn
persistence and the `--continue` flag, on [RFD 020] (Parallel Conversations) for
conversation locks and the `--id` flag, and on [RFD 019] (Non-Interactive Mode)
for detached prompt policies.

## Motivation

Each `jp query` invocation occupies a terminal for its entire duration. A
long-running query with multiple tool call cycles keeps a terminal tab busy
until it completes. If the user closes the terminal, the query dies.

With per-session conversation tracking ([RFD 020]) and resumable turns ([RFD
023]), the foundations exist for a query to run independently of a terminal
session: conversations can be targeted by ID, locks prevent concurrent
mutations, and incomplete turns can be persisted and resumed.

What's missing is the ability to say "run this in the background" and the policy
for what happens when a backgrounded query needs user input.

## Design

### `--detach`

`jp query --detach` spawns a background process for the conversation and exits
immediately. The background process runs the query to completion — streaming the
LLM response, executing tool calls, and cycling through follow-up rounds —
without a terminal.

```sh
jp query --detach "Refactor the auth module"
jp query --detach --id=jp-c17528832001 "Continue in the background"
jp query --detach --new "Start something new in the background"
```

All conversation targeting flags from [RFD 020] combine with `--detach`. All
lock errors apply — if the conversation is locked, `--detach` fails the same way
a foreground query would.

`--detach` is an explicit opt-in. Piped execution (`echo foo | jp query | cat`)
is **not** detached — the process runs in the foreground, owned by the pipeline.
`--detach` means "spawn a background process and exit." The absence of a TTY
does not imply detachment.

### Stopping Points

A detached process runs until it reaches a **stopping point**:

| Stopping point           | What happens                             |
|--------------------------|------------------------------------------|
| Turn completes           | Process persists the conversation and    |
|                          | exits cleanly.                           |
| Inquiry needs user input | Process persists the incomplete turn     |
|                          | ([RFD 023]) and exits.                   |
| Unrecoverable error      | Process persists what it can and exits   |
|                          | with an error.                           |

The key insight: a detached process never idles. It either does useful work
(streaming, executing tools) or it exits. There is no "paused waiting for input"
state — that state is represented by the persisted `IncompleteTurn` on disk, not
by a running process.

### The `queue` Detached Policy

This RFD adds a fourth detached policy mode to the three defined in [RFD 019]:

| Mode        | Behavior                                 |
|-------------|------------------------------------------|
| `auto`      | Auto-approve or route to LLM.            |
| `defaults`  | Use default values.                      |
| `deny`      | Fail the tool call.                      |
| **`queue`** | **Persist the incomplete turn and exit** |
|             | **the process.**                         |

When `queue` is active and an inquiry arrives:

1. The tool coordinator lets all other running tools in the batch complete.
2. Each completed tool's `ToolCallResponse` is persisted incrementally ([RFD
   023]).
3. The `InquiryRequest` for the tool that needs input is persisted.
4. The process exits cleanly.

The conversation is now in an incomplete turn state. The user resumes it later
with `jp query --continue --id=<cid>`, which prompts for the inquiry answer and
continues the turn ([RFD 023]).

With `queue` available, it becomes the **default detached policy** — replacing
`deny` from [RFD 019] as the default for detached processes. Nothing runs
unattended unless explicitly configured. Users who want automation set `detached
= "auto"` in their config.

The default for non-detached non-interactive contexts (piped execution without
`--detach`) remains `deny` as defined in [RFD 019]. `queue` only applies when
the process was started with `--detach`.

#### Integration with prompt routing

The `route_prompt` function from [RFD 019] is extended:

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
        DetachedMode::Queue => PromptAction::PersistAndExit,
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

`PromptAction::PersistAndExit` signals the tool coordinator to let other tools
finish, persist the incomplete turn, and initiate a clean shutdown.

### Process Registry

Running detached processes register in the user-local data directory:

```
~/.local/share/jp/workspace/<workspace-id>/processes/
└── <conversation-id>.json
```

The workspace ID scopes the registry to avoid collisions between projects. The
conversation ID is the key — at most one running process per conversation,
enforced by the conversation lock ([RFD 020]).

#### Process Entry Format

```json
{
  "conversation_id": "jp-c17528832001",
  "pid": 12345,
  "started_at": "2025-07-19T14:30:00.000Z"
}
```

The entry is minimal: it records that a process is running and which
conversation it operates on. There is no `status` or `pending_inquiry` field —
the "waiting for input" state lives in the conversation's `IncompleteTurn` on
disk, not in the process registry. By the time an inquiry is pending, the
process has already exited.

The registry entry is written when the detached process starts and deleted when
it exits.

#### Stale Entry Cleanup

`jp conversation ls` checks PID liveness for each entry. If the process is dead,
the entry is removed. This handles crashes, SIGKILL, and machine reboots. PID
liveness is checked via `kill(pid, 0)` on Unix.

This uses the same background task cleanup approach as [RFD 020]'s lock and
session file cleanup.

### CLI Interface

#### `jp query --detach`

Spawns a background process and exits immediately. Prints the conversation ID so
the user can target it later:

```sh
$ jp query --detach "Refactor the auth module"
Detached: jp-c17528832001
```

#### `jp conversation ls` (extended)

Shows process status alongside conversation metadata:

| Status                          | Meaning                                  |
|---------------------------------|------------------------------------------|
| `running (pid NNN)`             | Detached process is active               |
| `waiting-for-input (tool_name)` | No process; incomplete turn with pending |
|                                 | inquiry                                  |
| `interrupted (...)`             | No process; incomplete turn without      |
|                                 | pending inquiry                          |
| (no status)                     | Idle, last turn complete                 |

The `running` status comes from the process registry (PID liveness check). The
`waiting-for-input` and `interrupted` statuses come from the conversation's last
event ([RFD 023]).

```sh
$ jp conversation ls
ID                TITLE            STATUS
jp-c17528832001   Refactor auth    waiting-for-input (fs_modify_file)
jp-c17528831500   Fix tests        running (pid 12345)
jp-c17528831000   Debug service    idle
```

#### `jp conversation kill <cid>`

Sends SIGTERM to the detached process, cleans up the registry entry. The
conversation data remains intact — this only terminates the process, which
releases the conversation lock ([RFD 020]).

```sh
$ jp conversation kill jp-c17528831500
Killed process 12345 for conversation jp-c17528831500.
```

If the process has already exited (stale entry), the entry is cleaned up
silently.

### Daemonization

`--detach` needs to create a process that outlives the parent terminal. The
re-exec strategy is cleanest:

1. `jp query --detach "message"` validates arguments, resolves the conversation,
   acquires the lock.
2. It re-executes itself with an internal flag (`--_detached`) and the resolved
   conversation ID, redirecting stdout/stderr to `/dev/null` (or a log file if
   `-v` is set).
3. The child process starts in a new process group (`setsid`), writes its PID to
   the process registry, and runs the query.
4. The parent confirms the child started, prints the conversation ID, and exits.

The internal `--_detached` flag is hidden from `--help`. It signals that the
process is already detached and should not attempt to daemonize again.

#### Output handling

A detached process has no terminal. Output channels:

| Channel    | Destination                              |
|------------|------------------------------------------|
| stdout     | `/dev/null` (assistant output has no     |
|            | consumer)                                |
| stderr     | `/dev/null` or log file                  |
| `/dev/tty` | Not available (`has_client = false`)     |
| Tracing    | Log file (if `-v` specified on the       |
|            | original command)                        |

The `Printer` is initialized with sink writers. Renderers still run (they
maintain state for event building) but their output is discarded.

### Relationship to Conversation Locks

A detached process holds the conversation lock ([RFD 020]) for its entire
execution. This prevents other sessions from writing to the same conversation.

Lock contention from a detached process produces a clear error:

```sh
$ jp query --id=jp-c17528832001 "follow-up"
Error: Conversation jp-c17528832001 is locked by pid 12345 (detached).

    jp conversation kill jp-c17528832001
                            Terminate the detached process.
    --fork                  Branch from this conversation.
    --new                   Start a new conversation.
```

When the detached process exits (turn complete or inquiry hit), it releases the
lock. The conversation is then available for `--continue` or a new query.

### End-to-End Workflow

A typical detached workflow:

```sh
# 1. Start a detached query
$ jp query --detach "Refactor auth to use the new token format"
Detached: jp-c17528832001

# 2. Check on it later
$ jp conversation ls
ID                TITLE            STATUS
jp-c17528832001   Refactor auth    waiting-for-input (fs_modify_file)

# 3. Resume and answer the inquiry
$ jp query --continue --id=jp-c17528832001
Resuming incomplete turn for jp-c17528832001...

⏳ Incomplete turn (waiting for input)
  ✓ cargo_check — completed
  ✓ fs_read_file — completed
  ⏸ fs_modify_file — "Overwrite existing file?"

> Overwrite existing file? [y/n]: y

[continues tool execution, sends follow-up to LLM, completes turn]
```

If the detached process completes without hitting an inquiry:

```sh
$ jp query --detach "Run cargo check on all crates"
Detached: jp-c17528832001

$ jp conversation ls
ID                TITLE            STATUS
jp-c17528832001   Run cargo check  idle

$ jp conversation print --id=jp-c17528832001 --last
[shows the completed turn]
```

## Drawbacks

**Process lifecycle management.** Daemonization (re-exec, setsid, signal
handling, output redirection) is non-trivial to implement correctly on Unix.
Edge cases around process groups, controlling terminals, and signal inheritance
require careful handling.

**Platform constraints.** `setsid`, `/dev/null`, and `kill(pid, 0)` are
Unix-specific. Windows support requires different daemonization and PID liveness
mechanisms. The initial implementation targets macOS and Linux only.

**No live output.** A detached process discards all rendered output. If the user
wants to see what's happening, they must wait for completion and use
`conversation print`. Live re-attachment to a running process is deferred to a
separate RFD.

**Two-step for inquiries.** When a detached query hits an inquiry, the user must
run a separate command (`--continue`) to answer it. This is more friction than
an interactive terminal where the prompt appears immediately. The trade-off is
intentional: the terminal is freed for other work.

## Alternatives

### Keep the process alive at inquiries (original RFD 022 design)

When a detached process hits an inquiry, keep the process running and use IPC
(Unix domain sockets) to deliver the answer when the user attaches.

Rejected because keeping a process alive to wait for input is wasteful. The
conversation state is already persisted. The `IncompleteTurn` from [RFD 023]
captures everything needed to resume. Exiting and resuming via `--continue` is
simpler, uses no IPC, requires no socket infrastructure, and works across
machine reboots.

### `nohup` wrapper instead of re-exec

Tell users to run `nohup jp query "..." &` instead of building daemonization
into JP.

Rejected because it's fragile (output handling, signal masking, lock cleanup
depend on the user's shell configuration), undiscoverable, and doesn't integrate
with the process registry or `conversation ls`.

### Tmux/screen session instead of daemonization

Spawn the query inside a tmux session for detachment.

Rejected because it adds a hard dependency on an external tool, doesn't
integrate with `conversation ls`, and requires the user to know tmux.

### Store process registry in `.jp/`

Put process entries in `.jp/processes/` alongside conversations.

Rejected because `.jp/` is typically committed to version control. Process state
is ephemeral, machine-local, and user-local. The user data directory
(`~/.local/share/jp/workspace/`) is the correct location.

## Non-Goals

- **Live re-attachment.** Connecting to a still-running detached process to see
  live output and answer inquiries interactively is a separate concern. This RFD
  covers detaching and running to a stopping point; re-attachment is deferred to
  a future RFD.

- **Sub-agent support.** The process model is compatible with future sub-agents
  but this RFD does not propose agent infrastructure.

- **Cross-machine visibility.** Process registry is local. No network protocol.

- **Non-query commands.** Only `jp query` creates detached processes.

## Risks and Open Questions

### Lock handoff timing

The re-exec daemonization strategy has a timing question: who holds the
conversation lock?

Option A: The parent acquires the lock, passes the lock file descriptor to the
child via fd inheritance, and the child holds it for the duration. The parent
exits after confirming the child started. The lock is never released between
parent and child.

Option B: The parent does not acquire the lock. The child acquires it after
starting. There is a window where no process holds the lock — another session
could sneak in. This is unlikely but possible.

Option A is safer. `flock` file descriptors survive `exec`, so the child
inherits the lock.

### Multiple inquiries in one batch

If two tools in the same batch both need user input while in `queue` mode, both
`InquiryRequest` events are persisted. On `--continue`, the user answers both
sequentially before any tool re-executes. The turn state reconstruction in [RFD
023] handles this — it finds all pending inquiries and prompts for each.

### Interaction with `--non-interactive`

`--detach` implies `--non-interactive` ([RFD 019]). A detached process has no
TTY and cannot prompt. The detached policy (`queue` by default) governs what
happens at inquiries.

`--detach --non-interactive` is redundant but not an error.

### Process registry race conditions

Two terminals running `jp query --detach --new` simultaneously create two
conversations with two processes. Each writes its own registry entry (keyed by
conversation ID). No race — different conversations, different entries.

The conversation lock prevents two detached processes on the *same*
conversation.

### Detached process logging

When a detached process encounters errors (LLM provider down, tool failure), the
errors are lost if no log file is configured. Consider defaulting to a log file
when `--detach` is used, even without `-v`:

```txt
~/.local/share/jp/workspace/<workspace-id>/processes/<conversation-id>.log
```

This log would be cleaned up alongside the registry entry on process exit.

## Implementation Plan

### Phase 1: Process Registry

Every `jp query` writes a process entry on start and removes it on exit. `jp
conversation ls` reads the registry and displays `running (pid NNN)` alongside
conversation metadata. Stale entries are cleaned up via PID liveness checks.

No `--detach` yet. Just visibility into running foreground queries.

Can be merged independently.

### Phase 2: `queue` Detached Policy

Add `DetachedMode::Queue` and `PromptAction::PersistAndExit` to the prompt
routing ([RFD 019]). When the tool coordinator receives `PersistAndExit`, it
waits for other tools to finish, persists the incomplete turn, and initiates
shutdown.

`queue` is not yet the default — it requires `--detach` (Phase 3) to be useful.
For now it can be activated via config (`detached = "queue"`) for testing with
`--non-interactive`.

Depends on [RFD 023] Phase 2 (incremental persistence) and [RFD 019] Phase 2
(routing integration).

### Phase 3: `--detach` Flag and Daemonization

Implement `jp query --detach`. Re-exec daemonization with `setsid`, lock handoff
via fd inheritance, output redirection to sink/log.

Implement `jp conversation kill <cid>`.

Set `queue` as the default detached policy for `--detach`.

Depends on Phase 1 and Phase 2. Unix-only initially.

## References

- [RFD 023: Resumable Conversation Turns][RFD 023] — incomplete turn persistence
  and `--continue` flag; prerequisite for the exit-and-resume model.
- [RFD 020: Parallel Conversations][RFD 020] — conversation locks and
  per-session targeting; prerequisite for detached execution.
- [RFD 019: Non-Interactive Mode][RFD 019] — detached prompt policies (`auto`,
  `defaults`, `deny`); this RFD adds `queue`.
- [RFD 018: Typed Inquiry System][RFD 018] — the `Inquiry` enum used in prompt
  routing.
- [RFD 005: First-Class Inquiry Events][RFD 005] — persisted inquiry events that
  appear in the incomplete turn.

[RFD 005]: 005-first-class-inquiry-events.md
[RFD 018]: 018-typed-prompt-routing-enum.md
[RFD 019]: 019-non-interactive-mode.md
[RFD 020]: 020-parallel-conversations.md
[RFD 023]: 023-resumable-conversation-turns.md
[RFD 027]: 027-client-server-query-architecture.md
