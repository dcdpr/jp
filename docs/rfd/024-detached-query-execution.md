# RFD 024: Detached Query Execution

- **Status**: Abandoned
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-19
- **Summary**: Runs queries in the background via `jp query --detach`, tracks
  active processes, and pauses on user inquiries.

> [!IMPORTANT]
> This RFD is **Abandoned**.
>
> Superseded by [RFD 027: Client-Server Query Architecture][RFD 027], which
> unifies detached execution, live attachment, and foreground queries under a
> single client-server model.
> The process registry and `queue` policy from this RFD are carried forward into
> RFD 027 (the `queue` policy is renamed to `defer`).
>
> The original text below is preserved for historical context.

## Summary

This RFD introduces `jp query --detach` for running queries in the background, a
process registry for tracking active queries, and the `queue` detached policy.
When a detached query hits an inquiry that requires user input, `queue` persists
the incomplete turn and exits so the user can resume it later with `--continue`.

This design builds on [RFD 023] (Resumable Conversation Turns) for incomplete
turn persistence and `--continue`, [RFD 020] (Parallel Conversations) for
conversation locks and `--id`, and [RFD 019] (Non-Interactive Mode) for detached
prompt policies.

## Motivation

Every `jp query` invocation occupies a terminal until the turn finishes.
A long-running query with multiple tool call cycles ties up the terminal tab,
and closing the terminal kills the query.

Per-session conversation tracking ([RFD 020]) and resumable turns ([RFD 023])
provide the foundations to run queries independently of a terminal session:
conversations can be targeted by ID, locks prevent concurrent mutations, and
incomplete turns can be persisted and resumed.
What is missing is a flag to run a query in the background and a policy for what
happens when that query needs user input.

## Design

### `--detach`

`jp query --detach` spawns a background process for the conversation and exits
immediately.
The background process streams the LLM response, executes tool calls, and cycles
through follow-up rounds without a terminal.

```sh
jp query --detach "Refactor the auth module"
jp query --detach --id=jp-c17528832001 "Continue in the background"
jp query --detach --new "Start something new in the background"
```

All conversation targeting flags from [RFD 020] work with `--detach`, and the
same lock checks apply: if the conversation is locked, `--detach` fails just
like a foreground query.

Detachment requires `--detach` explicitly; the absence of a TTY does not imply
it.
A piped command such as `echo foo | jp query | cat` still runs in the foreground
as part of the pipeline.

### Stopping Points

A detached process runs until it reaches a **stopping point**:

| Stopping point           | What happens                           |
| ------------------------ | -------------------------------------- |
| Turn completes           | Process persists the conversation and  |
|                          | exits cleanly.                         |
| Inquiry needs user input | Process persists the incomplete turn   |
|                          | ([RFD 023]) and exits.             |
| Unrecoverable error      | Process persists what it can and exits |
|                          | with an error.                         |

A detached process never idles.
It either does active work (streaming responses, executing tools) or exits.
The "waiting for input" state lives on disk as a persisted `IncompleteTurn`, not
in a paused process.

### The `queue` Detached Policy

This RFD adds a fourth detached policy mode to the three defined in [RFD 019]:

| Mode        | Behavior                                 |
| ----------- | ---------------------------------------- |
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

The user can then resume the incomplete turn with `jp query --continue
--id=<cid>`, which prompts for the inquiry answer and continues execution ([RFD
023]).

`queue` is the **default detached policy** for processes started with
`--detach`, replacing `deny` from [RFD 019].
Nothing runs unattended unless the user explicitly sets `detached = "auto"` in
their config.
Non-interactive commands without `--detach` (such as piped execution) keep
`deny` as their default.

#### Prompt Routing

The `route_prompt` function from [RFD 019] maps `DetachedMode::Queue` to
`PromptAction::PersistAndExit`:

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

The workspace ID scopes the registry to avoid collisions between projects.
The conversation ID is the key, since the conversation lock ([RFD 020]) allows
at most one running process per conversation.

#### Process Entry Format

```json
{
  "conversation_id": "jp-c17528832001",
  "pid": 12345,
  "started_at": "2025-07-19T14:30:00.000Z"
}
```

The detached process writes its registry entry on start and deletes it on exit.
The entry records only the conversation ID, PID, and start time; it has no
`status` or `pending_inquiry` field because the process exits as soon as an
inquiry is persisted to `IncompleteTurn` on disk.

#### Stale Entry Cleanup

`jp conversation ls` checks each entry's PID liveness via `kill(pid, 0)` on Unix
and removes entries for dead processes, recovering from crashes, `SIGKILL`, and
machine reboots.
Cleanup runs as a background task, matching the lock and session file cleanup in
[RFD 020].

### CLI Interface

#### `jp query --detach`

Spawns a background process, prints the conversation ID for later targeting, and
exits immediately:

```sh
$ jp query --detach "Refactor the auth module"
Detached: jp-c17528832001
```

#### `jp conversation ls`

Displays process and turn status alongside conversation metadata:

| Status                          | Meaning                                  |
| ------------------------------- | ---------------------------------------- |
| `running (pid NNN)`             | Detached process is active               |
| `waiting-for-input (tool_name)` | No process; incomplete turn with pending |
|                                 | inquiry                                  |
| `interrupted (...)`             | No process; incomplete turn without      |
|                                 | pending inquiry                          |
| (no status)                     | Idle, last turn complete                 |

The `running` status comes from the process registry after a PID liveness check.
The `waiting-for-input` and `interrupted` statuses come from the conversation's
last event ([RFD 023]).

```sh
$ jp conversation ls
ID                TITLE            STATUS
jp-c17528832001   Refactor auth    waiting-for-input (fs_modify_file)
jp-c17528831500   Fix tests        running (pid 12345)
jp-c17528831000   Debug service    idle
```

#### `jp conversation kill <cid>`

Sends `SIGTERM` to the detached process and removes its registry entry.
Terminating the process releases the conversation lock ([RFD 020]) while leaving
the persisted conversation data intact.

```sh
$ jp conversation kill jp-c17528831500
Killed process 12345 for conversation jp-c17528831500.
```

If the process has already exited, `kill` removes the stale registry entry
silently.

### Daemonization

`--detach` re-executes the `jp` binary so the child process outlives the parent
terminal:

1. `jp query --detach "message"` validates arguments, resolves the conversation,
   and acquires the conversation lock.
2. It re-executes itself with an internal flag (`--_detached`) and the resolved
   conversation ID, redirecting stdout and stderr to `/dev/null` (or a log file
   if `-v` is set).
3. The child process starts in a new process group (`setsid`), writes its PID to
   the process registry, and runs the query.
4. The parent confirms the child started, prints the conversation ID, and exits.

The internal `--_detached` flag is hidden from `--help` and tells the child
process not to daemonize again.

#### Output Handling

Because a detached process has no terminal, its output channels are routed as
follows:

| Channel    | Destination                          |
| ---------- | ------------------------------------ |
| stdout     | `/dev/null` (assistant output has no |
|            | consumer)                            |
| stderr     | `/dev/null` or log file              |
| `/dev/tty` | Not available (`has_client = false`) |
| Tracing    | Log file (if `-v` specified on the   |
|            | original command)                    |

The `Printer` is initialized with sink writers.
Renderers still run because they maintain state for event building, but their
rendered output is discarded.

### Conversation Locks

A detached process holds the conversation lock ([RFD 020]) for its entire
execution, preventing other sessions from mutating the conversation.

Targeting a conversation locked by a detached process reports the PID and
recovery options:

```sh
$ jp query --id=jp-c17528832001 "follow-up"
Error: Conversation jp-c17528832001 is locked by pid 12345 (detached).

    jp conversation kill jp-c17528832001
                            Terminate the detached process.
    --fork                  Branch from this conversation.
    --new                   Start a new conversation.
```

When the detached process exits, whether the turn completed or paused at an
inquiry, it releases the lock so `--continue` or a new query can acquire it.

### End-to-End Workflow

In a typical detached workflow, the user starts a query in the background,
checks its status, and resumes it when an inquiry needs input:

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

**Process lifecycle management.** Daemonization on Unix (re-exec, `setsid`,
signal handling, and output redirection) has subtle edge cases around process
groups, controlling terminals, and signal inheritance.

**Platform constraints.** `setsid`, `/dev/null`, and `kill(pid, 0)` are
Unix-specific.
Supporting Windows requires separate daemonization and PID liveness mechanisms,
so the initial implementation targets macOS and Linux only.

**No live output.** A detached process discards all rendered output.
To inspect the response, the user must wait for the process to stop and run
`conversation print`.
Live re-attachment to a running process is deferred to a separate RFD.

**Two-step inquiry handling.** Answering an inquiry from a detached query
requires running `jp query --continue` as a separate step.
This adds friction compared to an inline terminal prompt, in exchange for
freeing the terminal while the query runs.

## Alternatives

### Keep the Process Alive at Inquiries (Original RFD 022 Design)

When a detached process hits an inquiry, keep it running and use IPC (Unix
domain sockets) to deliver the answer when the user attaches.

Rejected because keeping an idle process alive to wait for input is unnecessary
once `IncompleteTurn` ([RFD 023]) persists everything needed to resume.
Exiting and resuming via `--continue` avoids IPC and socket infrastructure and
works across machine reboots.

### `nohup` Wrapper Instead of Re-Exec

Tell users to run `nohup jp query "..." &` instead of building daemonization
into JP.

Rejected because shell backgrounding is fragile (output handling, signal
masking, and lock cleanup depend on the user's shell configuration),
undiscoverable, and invisible to the process registry and `conversation ls`.

### `tmux` or `screen` Session Instead of Daemonization

Spawn the query inside a `tmux` session for detachment.

Rejected because it adds a hard dependency on an external multiplexer, does not
integrate with `conversation ls`, and requires the user to know `tmux`.

### Store Process Registry in `.jp/`

Put process entries in `.jp/processes/` alongside conversations.

Rejected because `.jp/` is typically committed to version control, whereas
process state is ephemeral and local to the user and machine.
The user data directory (`~/.local/share/jp/workspace/`) is the appropriate
location.

## Non-Goals

- **Live re-attachment.** Connecting to a running detached process to view live
  output and answer inquiries interactively is deferred to a future RFD.
  This RFD covers detaching and running until a stopping point.

- **Sub-agent support.** The process model is compatible with future sub-agents,
  but this RFD does not propose agent infrastructure.

- **Cross-machine visibility.** The process registry is local to the machine and
  defines no network protocol.

- **Non-query commands.** Only `jp query` creates detached processes.

## Risks and Open Questions

### Lock Handoff Timing

Re-exec daemonization raises the question of how the conversation lock transfers
from parent to child:

- **Option A**: The parent acquires the lock and passes the lock file descriptor
  to the child via fd inheritance.
  The parent exits after confirming the child started, so the lock is never
  released between parent and child.
- **Option B**: The parent does not acquire the lock; the child acquires it
  after starting.
  This leaves a window where neither process holds the lock and another session
  could acquire it.

Option A is safer because `flock` file descriptors survive `exec`.

### Multiple Inquiries in One Batch

If two tools in the same batch both need user input under the `queue` policy,
both `InquiryRequest` events are persisted.
On `--continue`, turn state reconstruction ([RFD 023]) finds all pending
inquiries and prompts the user for each one sequentially before re-executing any
tool.

### Interaction with `--non-interactive`

Because a detached process has no TTY and cannot prompt interactively,
`--detach` implies `--non-interactive` ([RFD 019]) and uses the detached policy
(`queue` by default) to handle inquiries.
Passing `--detach --non-interactive` explicitly is redundant but not an error.

### Process Registry Race Conditions

Two terminals running `jp query --detach --new` simultaneously create separate
conversations and write separate registry entries keyed by conversation ID, so
they do not race.
For an existing conversation, the conversation lock prevents two detached
processes from running concurrently.

### Detached Process Logging

When a detached process encounters an error (such as an unavailable LLM provider
or a tool failure), the error is lost if no log file is configured.
`--detach` could default to writing a per-conversation log file, even without
`-v`:

```txt
~/.local/share/jp/workspace/<workspace-id>/processes/<conversation-id>.log
```

This log file would be cleaned up alongside the registry entry on process exit.

## Implementation Plan

### Phase 1: Process Registry

Every `jp query` invocation writes a process entry on start and removes it on
exit.
`jp conversation ls` reads the registry, displays `running (pid NNN)` alongside
conversation metadata, and cleans up stale entries via PID liveness checks.

This phase provides visibility into running foreground queries before `--detach`
exists and can be merged independently.

### Phase 2: `queue` Detached Policy

Add `DetachedMode::Queue` and `PromptAction::PersistAndExit` to prompt routing
([RFD 019]).
When the tool coordinator receives `PersistAndExit`, it waits for other tools in
the batch to finish, persists the incomplete turn, and initiates shutdown.

`queue` does not become the default until Phase 3 adds `--detach`, but it can be
enabled via config (`detached = "queue"`) for testing with `--non-interactive`.

Depends on [RFD 023] Phase 2 (incremental persistence) and [RFD 019] Phase 2
(routing integration).

### Phase 3: `--detach` Flag and Daemonization

Implement `jp query --detach` using re-exec daemonization with `setsid`, lock
handoff via fd inheritance, and output redirection to sink writers or a log
file.
Implement `jp conversation kill <cid>`, and set `queue` as the default detached
policy for `--detach`.

Depends on Phase 1 and Phase 2.
Targets Unix only initially.

## References

- [RFD 023: Resumable Conversation Turns][RFD 023]: Incomplete turn persistence
  and the `--continue` flag, required for the exit-and-resume model.
- [RFD 020: Parallel Conversations][RFD 020]: Conversation locks and per-session
  targeting, required for detached execution.
- [RFD 019: Non-Interactive Mode][RFD 019]: Detached prompt policies (`auto`,
  `defaults`, `deny`), extended here with `queue`.
- [RFD 018: Typed Inquiry System][RFD 018]: The `Inquiry` enum used in prompt
  routing.
- [RFD 005: First-Class Inquiry Events][RFD 005]: Persisted inquiry events
  recorded in incomplete turns.

[RFD 005]: 005-first-class-inquiry-events.md
[RFD 018]: 018-typed-prompt-routing-enum.md
[RFD 019]: 019-non-interactive-mode.md
[RFD 020]: 020-parallel-conversations.md
[RFD 023]: 023-resumable-conversation-turns.md
[RFD 027]: 027-client-server-query-architecture.md
