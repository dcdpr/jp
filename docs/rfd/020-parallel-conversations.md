# RFD 020: Parallel Conversations

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-19
- **Extended by**: [RFD 069], [RFD 087]
- **Summary**: Replaces the workspace-wide active conversation with per-session
  tracking and cross-process locks so multiple terminals can run in parallel.

## Summary

This RFD replaces the workspace-wide "active conversation" with per-session
conversation tracking, adds conversation locks to prevent concurrent mutations,
and introduces `--id` and `--fork` flags on `jp query` for explicit conversation
targeting.
Together, these changes allow users to run multiple conversations in parallel
across terminal sessions.

## Motivation

JP tracks a single "active conversation" per workspace in
`ConversationsMetadata.active_conversation_id`.
Every `jp query` without `--new` operates on this conversation, regardless of
which terminal runs the command.

This creates three problems:

**Terminal interference.** Running `jp query` in two terminal tabs within the
same workspace targets the same conversation.
The second query appends to the first query's history, and tool calls from one
session interleave with events from the other, leaving the conversation
incoherent or corrupted.

**No parallelism.** Users cannot run two independent queries at the same time in
one workspace.
Starting a new conversation in one terminal (`jp query --new`) changes the
active conversation for every terminal, so the other terminal's next `jp query`
silently switches to the new conversation.

**Fragile state.** `active_conversation_id` is a single point of contention.
If two processes write it simultaneously (for example, both running `jp query
--new`), the last writer wins and the other session loses track of its
conversation.

These problems block background execution and multi-agent workflows.
To run conversations in parallel, each session needs its own conversation
identity and each conversation needs protection against concurrent writes.

## Design

### Session Identity

A **session** is a terminal context, such as a tab, window, tmux pane, or
scripting environment.
JP identifies sessions using three layers, checked in order:

1. **`$JP_SESSION` environment variable.** If set, this value takes priority
   over all other layers.
   The value is opaque to JP; any non-empty string works.

2. **Platform-specific automatic detection.** On Unix, `getsid(0)` returns the
   session leader PID.
   On Windows, `GetConsoleWindow()` returns a per-tab window handle (HWND).

3. **Terminal-specific environment variables.** JP checks known per-tab or
   per-pane environment variables set by terminal emulators and multiplexers.

If none of these layers produce a session identity, JP operates without a
session and cannot persist session mappings.
In interactive terminals, commands that need a conversation show an interactive
picker (see [`jp query` (no flags)](#jp-query-no-flags)).
In non-interactive environments, they fail with an error directing the user to
`--id`, `--new`, or `$JP_SESSION`.

#### `$JP_SESSION` Override

The `$JP_SESSION` environment variable overrides automatic detection when
detection is unavailable or unreliable:

- **CI and scripts**: Non-interactive environments without a terminal can set
  `JP_SESSION` to any stable identifier.
- **SSH**: SSH allocates a new PTY per connection.
  Setting `JP_SESSION` to a stable value preserves the session across
  reconnects.
- **Unusual terminal setups**: Environments where automatic detection returns
  missing or inaccurate results can set `JP_SESSION` explicitly.

#### Automatic Platform Detection

If `$JP_SESSION` is not set, JP uses a platform-specific mechanism:

**Unix: Session leader PID.** `getsid(0)` returns the PID of the session leader,
typically the login shell spawned by the terminal.
Each terminal tab, window, and tmux pane has its own session leader, making the
PID unique per tab.
Subshells and child processes inherit this PID, so `bash -c "jp query ..."` sees
the same session identity as the parent shell.
The PID also stays stable across tmux detach and reattach because the shell
process inside the pane remains alive.

The main limitation is PID recycling: if a terminal closes and a new process
receives the same PID as the old session leader, a new terminal could inherit
the old session mapping.
**Stale session detection** prevents this by checking whether the session leader
process is still alive and discarding the mapping if it is dead.

**Windows: Console window handle.** `GetConsoleWindow()` returns the HWND of the
console host (`conhost.exe` or `openconsole.exe`) backing the current tab.
Each tab in Windows Terminal, CMD, or PowerShell runs its own console host
process with a unique HWND, which stays stable across commands as long as the
shell process keeps the console host alive.
Stale detection on Windows likewise checks whether the console host process is
still alive.

#### Terminal Environment Variables

If platform-specific detection fails (for example, when there is no controlling
terminal), JP checks terminal-specific environment variables as a fallback:

| Variable            | Terminal           | Granularity | Platform       |
| ------------------- | ------------------ | ----------- | -------------- |
| `$TMUX_PANE`        | tmux               | Per-pane    | Cross-platform |
| `$WEZTERM_PANE`     | WezTerm            | Per-pane    | Cross-platform |
| `$TERM_SESSION_ID`  | macOS Terminal.app | Per-tab     | macOS          |
| `$ITERM_SESSION_ID` | iTerm2             | Per-session | macOS          |

JP uses only variables with **per-tab or per-pane** granularity.
Per-window variables such as `$WT_SESSION` (Windows Terminal),
`$KITTY_WINDOW_ID` (Kitty), and `$ALACRITTY_WINDOW_ID` (Alacritty) are excluded
because multiple tabs in the same window share one value and would collide.

Because these variables are opaque strings, JP cannot check process liveness for
them.
Their session mappings are cleaned up only when none of the referenced
conversations exist on disk (see [Stale File Cleanup](#stale-file-cleanup)).
New terminal variables can be added to this list without an RFD.

### Session-to-Conversation Mapping

JP stores a mapping from session identity to conversation ID in the user-local
data directory:

```
~/.local/share/jp/workspace/<workspace-id>/sessions/
├── <session-key>.json
```

`<session-key>` is the `$JP_SESSION` value or the session leader PID (for
example, `12057`).

> [!TIP]
> Keying the filename on the bare value alone can collide when two env-sourced
> sessions share a value across different variables (such as `$JP_SESSION=1234`
> and `$TMUX_PANE=1234`), or when an `Env` value matches a session-leader PID,
> because both resolve to `1234.json`.
> Encoding the full session *source* into the filename (`getsid-<pid>`,
> `hwnd-<handle>`, `env-<KEY>-<hash(value)>`) and storing the session value
> inside the mapping avoids parsing the filename during cleanup.
> Legacy bare-value files are read via a fallback and migrated to the new
> filename during stale-file cleanup.

The mapping file contains:

```json
{
  "history": [
    {
      "id": "jp-c17528832001",
      "activated_at": "2025-07-19T14:30:00Z"
    },
    {
      "id": "jp-c17528832002",
      "activated_at": "2025-07-19T14:00:00Z"
    },
    {
      "id": "jp-c17528832003",
      "activated_at": "2025-07-19T13:00:00Z"
    }
  ],
  "source": {
    "type": "env",
    "key": "JP_SESSION"
  }
}
```

The `history` array tracks conversations activated in the session, ordered from
most recent to oldest.
Each entry records the conversation ID and activation timestamp.
When `jp query` operates on a conversation or `jp query --new` creates one, that
conversation is pushed to `history[0]`, which is always the session's active
conversation.
Reactivating an earlier conversation moves it to `history[0]` rather than
creating a duplicate entry.
This history enables `--id=previous` (the session's previously active
conversation) and lets the interactive picker list the session's own
conversations first.

The `source` field records how the session identity was derived, which
determines how stale detection works:

| Source        | `source` value                           | Stale detection                          |
| ------------- | ---------------------------------------- | ---------------------------------------- |
| `getsid`      | `"getsid"`                               | Delete only when session leader PID is   |
|               |                                          | confirmed dead. A live process keeps its |
|               |                                          | session unconditionally.                 |
| Windows HWND  | `"hwnd"`                                 | Delete only when the console window      |
|               |                                          | handle is no longer valid.               |
| `$JP_SESSION` | `{ "type": "env", "key": "JP_SESSION" }` | Liveness unknown; removed only when no   |
|               |                                          | referenced conversations exist on disk.  |
| `$TMUX_PANE`  | `{ "type": "env", "key": "TMUX_PANE" }`  | Same as `$JP_SESSION`.                   |

For `Getsid` and `Hwnd` sources, process liveness is authoritative.
While the session leader or console window is alive, the session mapping stays
valid even if its conversations are not yet visible on disk (for example, when
another process is mid-persist or created the conversation after the current
process loaded its index).
The mapping is deleted only when the process is confirmed dead.

Environment-variable sources are opaque strings without a liveness check, so
[Stale File Cleanup](#stale-file-cleanup) removes the mapping only when none of
the conversations in its history exist on disk.

### Conversation Locks

Conversations are protected by **exclusive file locks** during write operations.
When `jp query` starts, it acquires an advisory lock on the conversation via
`fd-lock` (`flock` on Unix, `LockFileEx` on Windows).
The lock is held for the entire query execution (including tool calls and
multi-cycle turns) and released when the command exits.

Lock files live in the user-local data directory:

```
~/.local/share/jp/workspace/<workspace-id>/locks/<conversation-id>.lock
```

The lock file contains the PID and session identity for diagnostic purposes:

```json
{
  "pid": 12345,
  "session": "12057",
  "acquired_at": "2025-07-19T14:30:00.000Z"
}
```

This JSON payload is informational only; the OS enforces the lock via `flock`
(Unix) or `LockFileEx` (Windows).
Dropping the lock guard (on normal exit, Ctrl+C, or panic) releases the OS lock
and deletes the `.lock` file.
If the process is killed with SIGKILL, the OS still releases the lock
automatically, and the orphaned `.lock` file is cleaned up later (see [Stale
File Cleanup](#stale-file-cleanup)).

#### Protected Operations

| Operation                | Lock required?  | Rationale                      |
| ------------------------ | --------------- | ------------------------------ |
| `jp query` (any variant) | Yes (exclusive) | Writes events to conversation  |
| `jp conversation rm`     | Yes (exclusive) | Deletes conversation data      |
| `jp conversation edit`   | Yes (exclusive) | Modifies conversation metadata |
| `jp conversation show`   | No              | Read-only                      |
| `jp conversation ls`     | No              | Read-only                      |
| `jp conversation fork`   | No              | Reads source, writes to a new  |
|                          |                 | conversation                   |
| `jp conversation grep`   | No              | Read-only                      |
| `jp conversation print`  | No              | Read-only                      |

> [!TIP]
> [RFD 052] adds workspace sanitization, which moves corrupt conversation
> directories to `.trash/`.
> Sanitization should acquire an exclusive lock on each conversation before
> trashing it, to avoid moving a directory that another session is actively
> writing to.

#### Lock Acquisition Behavior

When a lock cannot be acquired immediately, JP blocks and waits for it to be
released.
In interactive terminals, JP presents a selection prompt:

```txt
Conversation jp-c17528832001 is locked (held by pid 12345, session 12057).

> Continue waiting
  Start a new conversation
  Fork this conversation
  Cancel
```

Selecting "Continue waiting" resumes the wait, "Cancel" exits the command, and
the remaining options let the user proceed immediately without passing CLI
flags.

In non-interactive environments (no TTY on stdin), JP prints an informational
message and waits:

```txt
Waiting for lock on conversation jp-c17528832001 (held by pid 12345, session 12057)...
```

Because `jp query` holds the lock for the entire session (including multi-cycle
tool call loops that can run for minutes), JP enforces a default timeout of 30
seconds.
If the lock is not released within that window, the command fails:

```txt
Error: Timed out waiting for lock on conversation jp-c17528832001
       (held by pid 12345, session 12057).

Suggestions:
    --id        open conversation picker.
    --id=<id>   target a specific conversation.
    --id=last   continue the most recently active conversation.
    --new       start a new conversation.
    --fork      branch from this one.
```

Pressing Ctrl+C during the wait cancels immediately.

The `$JP_LOCK_DURATION` environment variable overrides the default timeout and
accepts `humantime`-compatible duration strings (such as `10s`, `2m`, or `1h`).
Setting it to `0` disables waiting, so lock acquisition fails immediately if the
lock is held, which is useful for scripts and CI pipelines that must not block.

The implementation polls with non-blocking `try_lock()` at ~500ms intervals
rather than issuing a blocking lock call, keeping timeout enforcement and signal
handling uniform across platforms.

#### Type-Level Enforcement

A `ConversationLock` type proves at compile time that the process holds the
lock:

```rust
/// Process-level lock. Acquired once at the start of `jp query`,
/// released when the query ends. Holds the flock.
pub struct ConversationLock {
    _file: File,  // holds the flock; released on drop
    conversation_id: ConversationId,
}
```

`Workspace::lock_conversation(id) -> Result<ConversationLock>` acquires the
`flock` and returns the lock.
The constructor is private, so callers can obtain a `ConversationLock` only
through this method.
`jp query` acquires the lock once at startup and holds it in a long-lived scope
(such as `Ctx`).

Mutable access to a conversation's event stream or metadata requires a reference
to the lock:

```rust
impl Workspace {
    pub fn get_events_mut(
        &mut self,
        id: &ConversationId,
        _lock: &ConversationLock,
    ) -> Option<&mut ConversationStream> { /* ... */ }

    pub fn get_conversation_mut(
        &mut self,
        id: &ConversationId,
        _lock: &ConversationLock,
    ) -> Option<Mut<'_, ConversationId, Conversation>> { /* ... */ }
}
```

All existing mutation methods stay on `ConversationStream` and `Conversation`,
while `get_events_mut`, `try_get_events_mut`, `get_conversation_mut`, and
`try_get_conversation_mut` take an additional `&ConversationLock` parameter:

```rust
// before
workspace.try_get_events_mut(&cid)?.add_config_delta(delta);
workspace.try_get_conversation_mut(&cid)?.title = Some(title);

// after
workspace.try_get_events_mut(&cid, &lock)?.add_config_delta(delta);
workspace.try_get_conversation_mut(&cid, &lock)?.title = Some(title);
```

This enforces the lock-before-mutate invariant at the API boundary: callers
cannot invoke `get_events_mut` or `get_conversation_mut` without a
`ConversationLock`.
Because `ConversationLock` is held for the entire `jp query` run, any mutable
reference obtained through it remains locked for its entire lifetime, and
dropping `ConversationLock` deletes the lock file.

#### Non-Persisting Queries Skip the Lock

`jp --no-persist query` skips lock acquisition because it writes no events back
to disk, avoiding write-write conflicts.
The query reads conversation state as a point-in-time snapshot.
If the lock holder writes `events.json` concurrently, the reader may see a
partially written file and fail to parse it.
Because writes occur only at discrete turn boundaries, this race window is small
and the user can retry.
Skipping the lock lets `jp --no-persist query --id=<id>` inspect a locked
conversation without waiting.

### CLI Changes

#### `jp query --id[=<target>]`

The `--id` flag accepts a conversation ID, a reserved keyword, or no value:

| Form                     | Behavior                                           |
| ------------------------ | -------------------------------------------------- |
| `--id=<conversation-id>` | Operate on a specific conversation by ID           |
| `--id=last-activated`    | Most recently activated conversation (any session) |
| `--id=last-created`      | Most recently created conversation                 |
| `--id=previous`          | Session's previously active conversation           |
| `--id` (no value)        | Show an interactive conversation picker            |

`last` is shorthand for `last-activated`, and `prev` is shorthand for
`previous`.
All forms wait if the target conversation is locked (see [Lock Acquisition
Behavior](#lock-acquisition-behavior)) and set the resolved conversation as the
session's default.

```sh
jp query --id=jp-c17528832001 "Follow-up question"  # explicit ID
jp query --id=last "Continue where I left off"       # most recent
jp query --id=previous "Back to the other thing"     # session's previous
jp query --id "Which conversation?"                  # interactive picker
```

`last` uses the `last_activated_at` timestamp across all conversations.
`last-created` uses the conversation's creation timestamp, which is useful when
picking up a conversation just created with `--new` in another tab.
`previous` reads the session's activation history to select the conversation
active before the current one (like `cd -` in a shell), and fails if the session
has no previous conversation.

New keywords can be added without structural changes because conversation IDs
use the `jp-c<timestamp>` format, which cannot collide with short alphabetic
keywords.

The interactive picker (`--id` with no value) displays recent conversations with
their title, last message preview, and timestamp.
Conversations previously activated in the current session appear first, followed
by the remaining conversations sorted by last activation time, matching the bare
`--cfg` interactive browser in [RFD 061].

In clap, `--id` uses `num_args = 0..=1` with `default_missing_value = ""`.
The empty string opens the interactive picker, recognized keywords run their
respective resolution, and any other value is parsed as a conversation ID.

#### `jp query --fork[=N]`

Forks the target conversation, sets the fork as the session's default, and
starts a new turn on it:

```sh
jp query --fork "Try a different approach"
jp query --fork=3 "Redo the last 3 turns differently"
```

If `N` is specified, the fork keeps the last `N` turns of the source
conversation; if omitted, it keeps the entire history.
This is shorthand for `jp conversation fork --activate` followed by `jp query`,
reading the source conversation without acquiring a lock and creating a new
conversation.

`--fork` combines with `--id` to fork a specific conversation:

```sh
jp query --fork --id=last "Branch from the most recent conversation"
jp query --fork --id=jp-c17528832001 "Branch from this one"
jp query --fork --id -- "Pick a conversation to fork"
```

Without `--id`, `--fork` operates on the session's active conversation and fails
if the session has none.

> [!TIP]
> [RFD 039] introduces conversation trees, where `--fork=0` becomes the standard
> mechanism for creating blank child conversations nested under their parent.

#### `jp query` (no flags)

Continues the session's active conversation:

```sh
jp query "Next question"
```

Resolution order:

1. **Session mapping exists**: Use the session's default conversation.
2. **No session mapping + interactive terminal**: Show the interactive
   conversation picker (same as `--id` with no value).
   The selected conversation becomes the session's default.
3. **No session mapping + non-interactive**: Fail with an error directing the
   user to `--id=<id>`, `--id=last`, `--new`, or `$JP_SESSION`.
4. **No conversations exist**: Fail with guidance to use `--new`.

In a single-session workflow, step 1 applies after the first query.
Step 2 runs only when a session has never operated on a conversation, such as in
a new terminal tab or split pane.
Showing the picker lets the user choose which conversation to continue rather
than silently routing to the wrong one.

The command fails if no conversations exist in the workspace, if the session is
non-interactive and has no session mapping, or if the target conversation is
locked by another session and the lock wait times out.

#### `jp query --new`

Creates a new conversation, sets it as the session's default, and starts a turn.
Because a new conversation has no other users, `--new` never contends for a
lock.

#### `jp conversation use <id>`

Sets the session's default conversation without starting a query or acquiring a
lock:

```sh
jp conversation use jp-c17528832001
```

Instead of changing a workspace-wide active conversation, `use` updates only the
current session's mapping.

### Removal of `active_conversation_id`

`ConversationsMetadata.active_conversation_id` is removed, eliminating the
workspace-wide active conversation.
Each session tracks its own default conversation through its session mapping.
Because Serde ignores unknown fields during deserialization, existing workspace
files that still contain `active_conversation_id` continue to load while JP
stops reading or writing the field.

### Terminal Title Updates

When `jp query` finishes a turn, JP writes the conversation's ID and title (or a
truncated ID if untitled) to the terminal title via the OSC 2 escape sequence:

```
\x1b]2;jp-c1234: Refactoring the config layer\x07
```

This makes the active conversation visible in the terminal's tab or title bar
(or pane header in tmux), helping users see which conversation is running in
each session.
The title updates at the end of each assistant turn rather than continuously
during streaming.
Terminals that do not support OSC 2 ignore the sequence.
The `jp_term::osc` module provides the escape sequence helpers.

## Drawbacks

**No sticky session without session identity.** Users without a controlling
terminal or `$JP_SESSION` cannot persist a session mapping.
Interactive terminals show the conversation picker on every bare `jp query`
invocation, while non-interactive environments fail with an error and require
`--id=<id>`, `--id=last`, or `--new`.

**Lock contention UX.** When a conversation is locked, JP waits up to 30 seconds
for the lock to be released (or shows an interactive prompt in terminals).
Brief contention while another query finishes its final write resolves
transparently, but long-running sessions time out and require the user to choose
another action.

**Additional storage location.** Session mappings in
`~/.local/share/jp/workspace/<workspace-id>/sessions/` add another user-local
state directory alongside conversation storage that users may need to inspect
when debugging.

**Interactive picker step for fresh sessions.** Opening a new terminal and
running bare `jp query` shows the conversation picker instead of automatically
continuing the last conversation.
This extra prompt prevents silent misrouting when multiple conversations are
active across sessions.

## Alternatives

### Keep Workspace-Wide Active Conversation as Fallback

Fall back to the workspace-wide `active_conversation_id` when no session mapping
exists and `--last` is not passed, preserving backward compatibility.

Rejected because it ties the fallback to mutable global state.
An implicit `--last` fallback achieves the same convenience without a shared
`active_conversation_id` singleton by reading the most recently modified
conversation from read-only metadata.

### PPID-Based Session Identity

Use the parent process ID instead of the session leader PID.

Rejected because PPID is not unique per terminal tab: all tmux panes share the
tmux-server PPID, VS Code terminal tabs can share a PPID, and subshells have a
different PPID than their parent shell.
The session leader PID (`getsid`) stays stable across all of these cases because
it resolves through the process hierarchy to the original session leader.

### No Conversation Locks

Rely on users not to run `jp query` on the same conversation from two terminals.

Rejected because concurrent appends to `events.json` corrupt conversation data.
Locks are required for data integrity.

### Lock per Workspace Instead of per Conversation

Acquire a single lock for the entire workspace so only one `jp query` can run at
a time.

Rejected because it prevents running independent queries simultaneously on
different conversations.
Per-conversation locks allow parallel work.

### Implicit `--last` Fallback for Fresh Sessions

Silently resolve to the most recently modified conversation when no session
mapping exists, avoiding the picker for single-terminal users.

Rejected because it silently picks the wrong conversation when multiple sessions
are active.
For example, if a user in a split pane sees conversation A on the left while an
agentic session in another tab recently updated conversation B, a bare `jp
query` in the right pane would silently continue conversation B. The interactive
picker makes this choice explicit at the cost of one prompt.

### Separate `--last` Flag

Add a dedicated `--last` flag instead of `--id=last`.

Rejected to keep conversation targeting under a single `--id` flag with three
modes (explicit ID, `last` keyword, and interactive picker).
This matches `--cfg` in [RFD 061], where a bare flag opens interactive mode and
a valued flag provides explicit input.

## Non-Goals

- **Background execution.** Running conversations as detached background
  processes, attaching to running conversations from other terminals, and
  deferred prompt answering are future work built on conversation targeting and
  locking.

- **Cross-machine sessions.** Session mappings are local to the machine.
  `$JP_SESSION` can be set to the same value on different machines, but the
  session-to-conversation mapping is not synchronized.

- **Automatic session detection beyond `getsid`.** Shell integration, terminal
  emulator APIs, and other session detection mechanisms are future work; the
  initial implementation uses `$JP_SESSION`, terminal environment variables, and
  `getsid`.

- **Conversation access control.** Locks prevent concurrent writes, not
  unauthorized access; any local user with filesystem access can read
  conversations.

## Risks and Open Questions

### Conversation Discovery for `--id=last`

`--id=last` selects the most recently modified conversation using
`last_activated_at`.
With per-session tracking, this timestamp updates whenever any session operates
on a conversation, so `--id=last` returns the conversation most recently used by
*any* session ("last worked on") rather than the most recently *created*
conversation (`--id=last-created`).

## Implementation Plan

### Phase 1: Session Identity

Add session identity resolution to `jp_cli`.
Check `$JP_SESSION`, then platform-specific detection (`getsid` on Unix,
`GetConsoleWindow` on Windows), then terminal environment variables
(`$TMUX_PANE`, `$WEZTERM_PANE`, `$TERM_SESSION_ID`, `$ITERM_SESSION_ID`).
Store the result in `Ctx` for use by commands.

### Phase 2: Handle-Based Workspace API

Introduce `ConversationHandle` and `ConversationGuard` to prove conversation
existence at the type level, along with the handle-based API
(`acquire_conversation`, `events`, `metadata`, `events_mut`, and
`metadata_mut`).
Migrate production callers while keeping `get_events` and `get_conversation` for
internal use.

### Phase 3a: Remove Global Active Conversation

Remove `ConversationsMetadata` and `conversations/metadata.json`, replacing the
workspace-wide singleton with per-session mappings (Phase 3b).

### Phase 3b: Session-to-Conversation Mapping and `ConversationNeed`

Store session mappings in `~/.local/share/jp/workspace/<id>/sessions/`.
`jp query` and `jp conversation use` write the mapping, and the startup pipeline
reads it to resolve the session's target conversation.

Implement the `ConversationNeed` enum (`None` and `Session`) and startup
dispatch so commands declare their conversation requirements and `None` commands
skip conversation loading.

### Phase 3c: Conversation Locks

`ConversationFileLock` in `jp_storage::lock` wraps OS-level advisory locks
(`flock` on Unix, `LockFileEx` on Windows) via `libc` and `windows-sys` directly
rather than `fd-lock` (avoiding the `RwLockWriteGuard` lifetime issue and a
`windows-sys` version conflict).

`ConversationGuard` holds `ConversationHandle` and
`Option<ConversationFileLock>`, where `None` supports `guard_conversation`
(unlocked access for `--no-persist` and unmigrated commands).
`jp query` acquires the lock with polling and timeout
(`acquire_conversation_lock` in `query.rs`), and `$JP_LOCK_DURATION` overrides
the default 30-second timeout.

### Phase 4a: CLI Flags

Add `--id` and `--fork` to `jp query`.
The `--id` flag supports three modes:

1. `--id=<id>`: target an explicit conversation.
2. `--id=last`: target the most recently modified conversation.
3. `--id` (no value): open the interactive picker.

`--fork[=N]` composes with `--id` for forking specific conversations.

This phase also adds `ConversationNeed::New` and
`ConversationNeed::Explicit(id)` variants and their startup dispatch paths.

### Phase 4b: Interactive Pickers

Implement the interactive selection prompts:

- **Conversation picker** (`--id` with no value, or bare `jp query` with no
  session mapping in an interactive terminal): lists recent conversations with
  their title, last message preview, and timestamp, showing session-relevant
  conversations first.
- **Lock contention prompt** (conversation locked by another session): offers
  continue waiting, start new conversation, fork, and cancel.

#### Non-Interactive Fallback

In non-interactive environments (stdin is not a TTY):

- Bare `--id` (picker mode) fails with a `NoConversationTarget` error directing
  the user to `--id=<id>`, `--id=last`, `--new`, or `$JP_SESSION`.
- The `Session` path with no session mapping fails with the same error.

In interactive terminals, both cases fall through to the interactive picker.

#### Conversation Picker

Built with `inquire::Select` at two call sites:

1. **`conversation_need()` Picker path** (`--id` with no value in an interactive
   terminal): `pick_conversation()` in `query.rs` shows the picker and returns
   `Explicit(id)`.
   Conversations are sorted by `last_activated_at` (most recent first) with the
   session's active conversation pinned to the top.
2. **`run_inner` Session path** (no session mapping in an interactive terminal):
   `pick_session_conversation()` in `lib.rs` shows the same picker when more
   than one conversation exists, returning `Option<ConversationId>` (`None` on
   cancel falls through to the most-recent-conversation default).

Both pickers display `{id} {title}` per line (or `{id}` if untitled).

#### Lock Contention Prompt

When `acquire_conversation_lock` times out in an interactive terminal, it shows
an `inquire::Select` prompt with four options:

- **Continue waiting**: re-enters the polling loop with a fresh timeout and
  returns to the prompt if the next timeout also expires.
- **Start a new conversation**: returns `LockOutcome::NewConversation`;
  `Query::run` creates a new conversation and acquires the lock on it.
- **Fork this conversation**: returns `LockOutcome::ForkConversation`;
  `Query::run` forks the locked conversation (read-only, no lock needed) and
  acquires the lock on the fork.
- **Cancel**: returns `Error::LockTimeout`, terminating the command.

In non-interactive environments (no TTY), `acquire_conversation_lock` fails with
`Error::LockTimeout` immediately on timeout.

### Phase 5: Terminal Title Updates

Write the conversation title to the terminal title via OSC 2 at two points:

1. **At startup** when the target conversation is resolved, so the user
   immediately sees which conversation is active.
2. **Inside the title generation task** when a title is first generated, so the
   update appears as soon as the title is ready rather than at the end of the
   turn.

Terminals that do not support OSC 2 ignore the sequence.

#### Implementation Notes

- `jp_term::osc::set_title()` writes the OSC 2 sequence to stderr.
- `Query::run()` calls `set_terminal_title(id, title)` after acquiring the
  conversation metadata, gated on `ctx.term.is_tty`.
- `TitleGeneratorTask::sync()` calls `jp_term::osc::set_title()` after writing
  the title to the workspace.
- Add `jp_term` as a dependency of `jp_task`.

### Phase 6: Stale File Cleanup {#stale-file-cleanup}

Runs synchronously at the end of every `jp` invocation (alongside ephemeral
conversation cleanup) via `Workspace::cleanup_stale_files()`:

- **Lock files**: `Storage::list_orphaned_lock_files()` attempts a non-blocking
  `flock` on each `.lock` file via `lock::is_orphaned_lock()`; if it succeeds,
  the file is orphaned and deleted.
- **Session mappings**: `Storage::list_session_files()` lists all session
  mapping files and evaluates each based on its `SessionSource`:
  - **Getsid/Hwnd** (process liveness checkable): deleted only when the
    originating process is confirmed dead.
    A live process keeps its session unconditionally without checking
    conversation existence, because another process may be mid-persist or may
    have created the conversation after the current process loaded its index.
  - **Env** (liveness unknown): falls back to checking conversation existence by
    re-scanning conversation IDs from disk (rather than the in-memory index) to
    account for conversations created by other processes.
    Mappings where no conversation in the history exists on disk are deleted.

## Implementation Details

### Conversation Targeting {#conversation-targeting}

#### Problem

The startup pipeline assumes every command operates on a conversation:

```txt
sanitize → load_conversation_index → load_config (incl. conversation config) → command.run()
```

`load_conversation_index` scans conversation IDs and sets the active
conversation, and `load_partial_config` reads the active conversation's events
to merge per-conversation config overrides.
This coupling creates three problems:

- **Commands that do not use conversations** (such as `jp config show`) still
  run through conversation resolution.
- **Conversation resolution can require user interaction** (the picker), even
  though the startup pipeline runs before command logic.
- **Config loading depends on knowing the active conversation**, while resolving
  the active conversation can require config (such as the picker backend).

#### Resolution

Each command declares its conversation targeting requirements via a trait
method, and the startup pipeline uses that declaration to decide whether and how
to load a conversation before building config:

```rust
/// Declared by each command to indicate its conversation needs.
enum ConversationNeed {
    /// Command does not operate on a conversation.
    /// Config is built without the conversation config layer.
    None,

    /// Command creates a new conversation.
    /// Config is built without the conversation config layer (new
    /// conversations have no stored config).
    New,

    /// Command targets a specific conversation by ID.
    /// The conversation is loaded and its config is included.
    Explicit(ConversationId),

    /// Command continues the session's active conversation.
    /// Resolved via session mapping, falling back to an interactive
    /// picker if no mapping exists or error for non-interactive sessions.
    Session,
}
```

The `IntoPartialAppConfig` trait (or a companion trait) adds:

```rust
fn conversation_need(
    &self,
    conversation_ids: &[ConversationId],
    session: &Option<Session>,
) -> Result<ConversationNeed>;
```

The startup pipeline calls `conversation_need` after scanning the filesystem and
before loading config.

#### Startup Flow

The startup pipeline runs as follows:

```txt
sanitize()
ids = workspace.conversation_ids()              // scan only
need = command.conversation_need(ids, session)

match need:
  None →
    config = build(layers 1 + 2 + 4 + 5)        // no conversation layer
  New →
    config = build(layers 1 + 2 + 4 + 5)        // no conversation layer
    create new conversation
  Explicit(id) →
    workspace.load_conversation(id)             // eager load target
    config = build(layers 1 + 2 + 3 + 4 + 5)    // full config
  Session →
    target = read_session_mapping()             // file read
    if target exists and valid:
      workspace.load_conversation(target)
      config = build(layers 1 + 2 + 3 + 4 + 5)
    else if interactive:
      pre_config = build(layers 1 + 2 + 4 + 5)  // for picker UI
      target = show_picker(ids, pre_config)
      workspace.load_conversation(target)
      config = build(layers 1 + 2 + 3 + 4 + 5)  // rebuild with conversation
    else:
      error with guidance

Ctx::new(workspace, config, ...)
command.run(ctx)
```

The `AppConfig` in `Ctx` is always complete and read-only.
Config is built twice only when the `Session` path falls back to the interactive
picker on the first invocation in a terminal (`/dev/tty`).

#### Workspace API and Conversation Handles

`Workspace` acts as a storage manager with no concept of an active conversation.
Commands express conversation targeting through handle types that prove
existence and access rights.

##### Type Hierarchy

```txt
ConversationGuard (owns both, for write commands like `jp query`)
├── ConversationHandle (proof of existence, for read access)
└── ConversationLock (proof of exclusive file lock, Phase 3c)
```

##### `ConversationHandle`

A move-only (non-`Copy`, non-`Clone`) type obtained exclusively through
`Workspace` methods that proves a conversation exists in the workspace index.
Only one handle can exist per conversation ID at a time; acquiring a second
handle to the same ID is a debug-asserted logic error.

```rust
/// Proof that a conversation exists and is loaded.
///
/// Move-only: consuming this handle (e.g., via `remove_conversation`)
/// invalidates all access. The borrow checker prevents use-after-move.
struct ConversationHandle {
    id: ConversationId,
    // Private constructor — only obtainable through Workspace methods.
}
```

##### `ConversationGuard`

Combines a `ConversationHandle` with a `ConversationLock` (Phase 3c) and is
required for mutable access to conversation state.
`ConversationGuard` derefs to `ConversationHandle` so read-only methods accept
it directly.

```rust
/// Proof of existence AND exclusive write access.
struct ConversationGuard {
    handle: ConversationHandle,
    lock: ConversationLock, // holds the flock; added in Phase 3c
}

impl Deref for ConversationGuard {
    type Target = ConversationHandle;
    fn deref(&self) -> &ConversationHandle { &self.handle }
}
```

##### Workspace API

```rust
impl Workspace {
    /// Scan conversation IDs from disk. No loading.
    fn conversation_ids(&self) -> Vec<ConversationId>;

    /// Acquire a handle to a conversation, proving it exists in the index.
    ///
    /// Does not load metadata or events from disk — those are loaded
    /// lazily when accessed via `events()`, `metadata()`, etc.
    /// Returns an error if the ID doesn't exist. Only one handle should
    /// exist per ID at a time (debug-asserted).
    fn acquire_conversation(
        &self,
        id: &ConversationId,
    ) -> Result<ConversationHandle>;

    /// Create a new conversation and return a handle to it.
    fn create_conversation(
        &mut self,
        conversation: Conversation,
        config: Arc<AppConfig>,
    ) -> ConversationHandle;

    // Read access (handle required, infallible)

    fn events(&self, h: &ConversationHandle) -> &ConversationStream;
    fn metadata(&self, h: &ConversationHandle) -> &Conversation;

    // Write access (guard required, infallible)

    fn events_mut(&mut self, g: &ConversationGuard) -> &mut ConversationStream;
    fn metadata_mut(&mut self, g: &ConversationGuard) -> &mut Conversation;

    // Lifecycle operations

    /// Remove a conversation. Consumes the guard, releasing the handle
    /// and lock. The borrow checker prevents use-after-remove.
    fn remove_conversation(&mut self, g: ConversationGuard) -> Conversation;

    /// Persist a specific conversation to disk.
    fn persist_conversation(&mut self, h: &ConversationHandle) -> Result<()>;

    /// Persist all modified conversations.
    fn persist(&mut self) -> Result<()>;
}
```

Methods that take `&ConversationHandle` are infallible because the handle proves
the conversation exists; an internal `expect` fires only if a `Workspace` bug
violates the handle invariant.

`events_mut` and `metadata_mut` require `&ConversationGuard` (which derefs to
`&ConversationHandle`), ensuring that write access requires both existence proof
and the file lock.
Read-only methods accept `&ConversationHandle` directly, so they work with
either a bare handle or a guard.

`remove_conversation` consumes the `ConversationGuard` by value, moving the
handle so the borrow checker prevents any use after removal.

> [!NOTE]
> Before Phase 3c adds `ConversationLock`, `ConversationGuard` can be a type
> alias for `ConversationHandle` or hold a placeholder lock field, allowing the
> API shape to stabilize before locking is implemented.

#### Command Implementations

| Command                    | `conversation_need`     | Notes                                  |
| -------------------------- | ----------------------- | -------------------------------------- |
| `jp query` (no flags)      | `Session`               | Reads session mapping, picker fallback |
| `jp query --new`           | `New`                   | Always creates fresh                   |
| `jp query --id=<id>`       | `Explicit(id)`          | Direct targeting                       |
| `jp config show`           | `None`                  | No conversation needed                 |
| `jp conversation rm <id>`  | `Explicit(id)`          | Targets specific conversation          |
| `jp conversation ls`       | `None`                  | Lists all, no active needed            |
| `jp conversation use <id>` | `None`                  | Writes session mapping only            |
| `jp conversation fork`     | `Session` or `Explicit` | Reads source conversation              |
| `jp attachment ls`         | `None`                  | No conversation needed                 |

## References

- `flock(2)`: POSIX advisory file locking used for conversation locks.
- `LockFileEx`: Windows equivalent, used via `windows-sys`.
- `getsid(2)`: POSIX function for obtaining the session leader PID.

### Platform Portability

| Concern          | Unix               | Windows                   | Rust crate             |
| ---------------- | ------------------ | ------------------------- | ---------------------- |
| File locking     | `flock`            | `LockFileEx`              | `libc` / `windows-sys` |
| Session identity | `getsid(0)`        | `GetConsoleWindow()` HWND | `libc` / `windows-sys` |
| Session env vars | `$TMUX_PANE`, etc. | `$WEZTERM_PANE`, etc.     | `std::env`             |

`ConversationFileLock` in `jp_storage::lock` abstracts the platform-specific
locking mechanism, calling `libc` and `windows-sys` directly rather than
`fd-lock` to avoid `RwLockWriteGuard` lifetime issues and a `windows-sys`
version conflict.
Session identity uses the three-layer resolution described in [Session
Identity](#session-identity) on both platforms.

[RFD 039]: 039-conversation-trees.md
[RFD 052]: 052-workspace-data-store-sanitization.md
[RFD 061]: 061-interactive-config.md
[RFD 069]: 069-guard-scoped-persistence-for-conversations.md
[RFD 087]: 087-session-scoped-active-workspace.md
