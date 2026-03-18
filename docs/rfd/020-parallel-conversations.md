# RFD 020: Parallel Conversations

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-19

## Summary

This RFD replaces the workspace-wide "active conversation" with per-session
conversation tracking, adds conversation locks to prevent concurrent mutations,
and introduces `--id`, `--last`, and `--fork` flags on `jp query` for explicit
conversation targeting. Together, these changes allow users to run multiple
conversations in parallel across terminal sessions.

## Motivation

JP tracks a single "active conversation" per workspace. This is a global
singleton stored in `ConversationsMetadata.active_conversation_id`. Every `jp
query` without `--new` operates on this conversation, regardless of which
terminal the command runs in.

This creates three problems:

**Terminal interference.** If you have two terminal tabs open in the same
workspace and run `jp query` in both, they operate on the same conversation. The
second query appends to the first's history. Tool calls from one session can
interleave with events from another. The conversation becomes incoherent and
potentially corrupted.

**No parallelism.** You cannot work on two independent queries at the same time
within one workspace. Starting a new conversation in one terminal (`jp query
--new`) changes the active conversation for all terminals. The other terminal's
next `jp query` silently switches to the new conversation.

**Fragile state.** The `active_conversation_id` is a single point of contention.
If two processes write it simultaneously (e.g., both running `jp query --new`),
the last writer wins and the other session loses track of its conversation.

These problems block any future work on background execution or multi-agent
workflows. Before conversations can run in parallel, each session needs its own
conversation identity and conversations need protection against concurrent
writes.

## Design

### Session Identity

A **session** is a terminal context - a tab, window, tmux pane, or scripting
environment. JP identifies sessions using three layers, checked in order:

1. **`$JP_SESSION` environment variable.** If set, this value is the session
   identity. It takes priority over everything else. The value is opaque to JP —
   any non-empty string works.

2. **Platform-specific automatic detection.** On Unix, `getsid(0)` returns the
   session leader PID. On Windows, `GetConsoleWindow()` returns a per-tab window
   handle (HWND).

3. **Terminal-specific environment variables.** JP checks a list of known
   per-tab or per-pane env vars set by popular terminal emulators and
   multiplexers.

If none of these produce a session identity, JP operates without a session.
Commands that need a conversation fall back to implicit `--last` behavior (see
[`jp query` (no flags)](#jp-query-no-flags)) but cannot persist a session
mapping. Each invocation re-resolves the conversation independently.

#### `$JP_SESSION`

The environment variable is the explicit override for cases where automatic
detection is unreliable or unavailable:

- **CI/scripts**: Non-interactive environments without a terminal can set
  `JP_SESSION` to any stable identifier.
- **SSH**: SSH allocates a new PTY per connection. If you want session
  persistence across SSH reconnects, set `JP_SESSION` to a stable value.
- **Unusual terminal setups**: Any environment where the automatic detection
  produces wrong or missing results.

#### Platform-Specific Automatic Detection

If `$JP_SESSION` is not set, JP uses a platform-specific mechanism:

**Unix: Session leader PID.** `getsid(0)` returns the PID of the session leader
— typically the login shell that the terminal spawned. Each terminal tab,
window, and tmux pane gets its own session leader, so the PID is unique per tab.
It is inherited by subshells and child processes, so `bash -c "jp query ..."`
sees the same session identity as the parent shell.

The session leader PID is also stable across tmux detach/reattach: the shell
process inside the pane stays alive, so its PID (and therefore the session
leader PID) does not change.

The main limitation is PID recycling. If a terminal is closed and a new process
happens to get the same PID as the old session leader, the new terminal could
see the old session mapping. This is handled by **stale session detection**: JP
checks whether the session leader process is still alive. If it is not, the
mapping is stale and is discarded.

**Windows: Console window handle.** `GetConsoleWindow()` returns the HWND of the
console host (`conhost.exe` or `openconsole.exe`) backing the current tab. Each
tab in Windows Terminal, CMD, or PowerShell gets its own console host process
with a unique HWND. The handle remains stable across commands within the same
tab because the shell process keeps the console host alive.

Stale detection on Windows uses the same approach, checking whether the console
host process is still alive.

#### Terminal Environment Variables

If platform-specific detection fails (e.g., no controlling terminal), JP checks
terminal-specific environment variables as a final fallback:

| Variable              | Terminal          | Granularity | Platform      |
|-----------------------|-------------------|-------------|---------------|
| `$TMUX_PANE`          | tmux              | Per-pane    | Cross-platform|
| `$WEZTERM_PANE`       | WezTerm           | Per-pane    | Cross-platform|
| `$TERM_SESSION_ID`    | macOS Terminal.app | Per-tab     | macOS         |
| `$ITERM_SESSION_ID`   | iTerm2            | Per-session | macOS         |

Only variables with **per-tab or per-pane** granularity are used. Per-window
variables like `$WT_SESSION` (Windows Terminal), `$KITTY_WINDOW_ID` (Kitty), and
`$ALACRITTY_WINDOW_ID` (Alacritty) are deliberately excluded because multiple
tabs in the same window share the value, which would cause sessions to collide.

Since these variables are opaque strings, session mappings sourced from them
cannot be stale-detected via process liveness. They are only cleaned up when
the conversation they point to no longer exists (see [Stale File
Cleanup](#stale-file-cleanup)).

This list is extensible. New terminals can be added without an RFD.

### Session-to-Conversation Mapping

JP stores a mapping from session identity to conversation ID in the user-local
data directory:

```
~/.local/share/jp/workspace/<workspace-id>/sessions/
├── <session-key>.json
```

Where `<session-key>` is the `$JP_SESSION` value or the session leader PID
(e.g., `12057`).

The mapping file contains:

```json
{
  "conversation_id": "jp-c17528832001",
  "updated_at": "2025-07-19T14:30:00.000Z",
  "source": {
    "type": "env",
    "key": "JP_SESSION"
  }
}
```

The `source` field records how the session identity was produced. This
determines whether and how stale detection can be performed:

| Source        | `source` value                           | Stale detection                          |
|---------------|------------------------------------------|------------------------------------------|
| `getsid`      | `"getsid"`                               | Check if session leader PID is alive     |
| `$JP_SESSION` | `{ "type": "env", "key": "JP_SESSION" }` | Not possible — never automatically       |
|               |                                          | removed                                  |
| `$TMUX_PANE`  | `{ "type": "env", "key": "TMUX_PANE" }`  | Not possible — never automatically       |
|               |                                          | removed                                  |
| Windows HWND  | `"hwnd"`                                 | Check if console host process is alive   |

When a stale mapping is detected (e.g., session leader process is no longer
alive), the mapping file is deleted immediately. Mappings sourced from
environment variables are opaque strings with no way to verify liveness, so
they are only removed by the [Stale File Cleanup](#stale-file-cleanup) task
when the conversation they point to no longer exists.

When `jp query` successfully operates on a conversation, the mapping is updated.
When `jp query --new` creates a new conversation, the mapping is set to the new
conversation. The mapping is the session's "default conversation."

### Conversation Locks

Conversations are protected by **exclusive file locks** during write operations.
When `jp query` starts, it acquires an advisory lock on the conversation via
`fd-lock` (`flock` on Unix, `LockFileEx` on Windows). The lock is held for the
entire query execution (including tool calls and multi-cycle turns) and released
when the command exits.

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

This content is informational only — the actual locking is done by the OS via
`flock` (Unix) or `LockFileEx` (Windows). If the process exits, crashes, or is
killed (including SIGKILL), the OS releases the lock automatically.

The lock file itself is eagerly deleted when the lock guard is dropped (normal
exit, Ctrl+C, panics). If the process is killed with SIGKILL, the lock file
orphans on disk but the OS releases the underlying lock automatically. Orphaned
files are harmless and are cleaned up by a background task (see [Stale File
Cleanup](#stale-file-cleanup)).

#### What the lock protects

| Operation                | Lock required?  | Rationale                      |
|--------------------------|-----------------|--------------------------------|
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
> directories to `.trash/`. Sanitization should acquire an exclusive lock on
> each conversation before trashing it, to avoid moving a directory that another
> session is actively writing to.

#### Lock acquisition behavior

When a lock cannot be acquired immediately, JP blocks and waits for it to be
released. In interactive terminals, JP presents a selection prompt:

```txt
Conversation jp-c17528832001 is locked (held by pid 12345, session 12057).

> Continue waiting
  Start a new conversation
  Fork this conversation
  Cancel
```

Selecting "Continue waiting" resumes the wait. The other options provide
immediate alternatives without the user having to remember CLI flags. "Cancel"
exits the command.

In non-interactive environments (no TTY on stdin), JP prints an informational
message and waits silently:

```txt
Waiting for lock on conversation jp-c17528832001 (held by pid 12345, session 12057)...
```

Since `jp query` holds the lock for the entire session (including multi-cycle
tool call loops that can run for minutes), JP enforces a default timeout of 30
seconds. If the lock is not released within this window, the command fails:

```txt
Error: Timed out waiting for lock on conversation jp-c17528832001
       (held by pid 12345, session 12057).

Suggestions:
    --id    target a specific conversation.
    --new   start a new conversation.
    --fork  branch from this one.
```

Ctrl+C during the wait cancels immediately.

The `$JP_LOCK_DURATION` environment variable overrides the default timeout.
It accepts `humantime`-compatible duration strings (e.g., `10s`, `2m`, `1h`).
Setting it to `0` disables waiting entirely — lock acquisition fails
immediately if the lock is held. This is useful for scripts and CI pipelines
that should not block.

The implementation polls with non-blocking `try_lock()` at ~500ms intervals
rather than issuing a blocking lock call. This makes timeout enforcement and
signal handling straightforward on both platforms.

#### Type-level enforcement

A `ConversationLock` type represents proof that the process holds the lock:

```rust
/// Process-level lock. Acquired once at the start of `jp query`,
/// released when the query ends. Holds the flock.
pub struct ConversationLock {
    _file: File,  // holds the flock; released on drop
    conversation_id: ConversationId,
}
```

`Workspace::lock_conversation(id) -> Result<ConversationLock>` acquires the
`flock` and returns the lock. The constructor is private — the only way to
obtain a `ConversationLock` is through this method. The lock is acquired once at
the start of `jp query` and lives in a long-lived scope (e.g., `Ctx`).

Mutable access to a conversation's event stream requires a reference to the
lock:

```rust
impl Workspace {
    pub fn get_events_mut(
        &mut self,
        id: &ConversationId,
        _lock: &ConversationLock,
    ) -> Option<&mut ConversationStream> { /* ... */ }
}
```

All existing mutation methods stay on `ConversationStream`. The only change is
that `get_events_mut` (and `try_get_events_mut`) take a `&ConversationLock`
parameter. Call sites add one argument:

```rust
// before
workspace.try_get_events_mut(&cid)?.add_config_delta(delta);

// after
workspace.try_get_events_mut(&cid, &lock)?.add_config_delta(delta);
```

This enforces the lock-before-mutate invariant at the API boundary. You cannot
call `get_events_mut` without proof that the process holds the lock.
`ConversationLock` is held for the entire `jp query` run, so any `&mut
ConversationStream` obtained through it is guaranteed to be protected by the
lock for its entire lifetime.

The lock file is deleted in the `Drop` implementation of `ConversationLock`.

#### Non-persisting queries skip the lock

`jp --no-persist query` skips lock acquisition entirely. Since no events are
written back to disk, there is no write-write conflict. The query reads
conversation state as a snapshot at the time the file is read. If the lock
holder writes `events.json` concurrently, the reader could see a partially
written file and fail to parse it. This is an acceptable trade-off: the user can
retry, and the window is small since writes happen at discrete turn boundaries.

This means `jp --no-persist query --id=<id>` works on locked conversations,
which is useful for running a non-persistent query without waiting for the lock.

### CLI Changes

#### `jp query --id=<id>`

Operate on a specific conversation by ID. Waits if the conversation is locked
(see [Lock acquisition behavior](#lock-acquisition-behavior)).

```sh
jp query --id=jp-c17528832001 "Follow-up question"
```

This is the explicit targeting mechanism. It does not depend on session identity
and works in all environments.

#### `jp query --last`

Operate on the most recently modified conversation in the workspace. Waits if
the conversation is locked (see [Lock acquisition
behavior](#lock-acquisition-behavior)). Sets the conversation as the session's
default.

```sh
jp query --last "Continue where I left off"
```

This uses the `last_activated_at` timestamp on conversations to find the most
recent one. It is useful for explicitly switching the session to whatever
conversation was most recently active, even if the session already has a mapping
to a different conversation.

#### `jp query --fork[=N]`

Fork the session's active conversation and start a new turn on the fork. The
forked conversation becomes the session's default.

```sh
jp query --fork "Try a different approach"
jp query --fork=3 "Redo the last 3 turns differently"
```

`N` is optional. If specified, the fork keeps the last `N` turns of the source
conversation. If omitted, the fork keeps the entire history.

This is shorthand for `jp conversation fork --activate` followed by `jp query`.
The fork reads the source conversation (no lock needed) and creates a new one.

`--fork` can combine with `--last` or `--id` to fork a specific conversation:

```sh
jp query --fork --last "Branch from the most recent conversation"
jp query --fork --id=jp-c17528832001 "Branch from this one"
```

Without `--last` or `--id`, `--fork` operates on the session's active
conversation. Fails if the session has no active conversation.

> [!TIP]
> [RFD 039] introduces conversation trees, where `--fork=0` becomes the standard
> mechanism for creating blank child conversations nested under their parent.

#### `jp query` (no flags)

Continue the session's active conversation. This is the common case for ongoing
work within a single terminal.

```sh
jp query "Next question"
```

Resolution order:

1. **Session mapping exists**: Use the session's default conversation.
2. **No session mapping**: Fall back to the most recently modified conversation
   in the workspace (implicit `--last` behavior). The resolved conversation
   becomes the session's default.
3. **No session identity**: Same as (2), but the mapping is not persisted. Each
   invocation re-resolves via `--last`.
4. **No conversations exist**: Fail with guidance to use `--new`.

The implicit `--last` fallback means opening a new terminal tab and running `jp
query` continues the conversation you were most recently working on. Once the
session has a mapping, subsequent `jp query` calls use that mapping regardless
of activity in other sessions.

Without a session identity (e.g., a CI pipeline without `$JP_SESSION`), step (3)
preserves the current "just works" behavior: `jp query` operates on the most
recently used conversation. The only downside is that every invocation
re-resolves, so there is no sticky session.

Fails if:
- No conversations exist in the workspace
- Conversation is locked by another session and the lock wait times out

The error message directs the user to `--new`, `--last`, `--id`, or `--fork`.

#### `jp query --new`

Unchanged. Creates a new conversation and starts a turn. The new conversation
becomes the session's default. Always succeeds (no lock contention — new
conversations have no other users).

#### `jp conversation use <id>`

Sets the session's default conversation without starting a query. The
conversation does not need to be unlocked (this is a session mapping change, not
a write to the conversation).

```sh
jp conversation use jp-c17528832001
```

This replaces the current behavior where `use` changes the workspace-wide active
conversation. It now only affects the current session.

### Removal of `active_conversation_id`

The `active_conversation_id` field in `ConversationsMetadata` is removed. There
is no workspace-wide "active" conversation. Each session independently tracks
its own default conversation via the session mapping.

The field is removed from the `ConversationsMetadata` struct. Serde ignores
unknown fields during deserialization, so old workspace files that contain the
field will continue to load. JP simply stops reading or writing it.

## Drawbacks

**No sticky session without session identity.** Users without a controlling
terminal and without `$JP_SESSION` get implicit `--last` behavior on every
invocation. This works, but the session is not sticky — if another session
modifies a different conversation between invocations, the next bare `jp query`
may resolve to a different conversation than the previous one.

**Lock contention UX.** When a conversation is locked, JP waits up to 30 seconds
for the lock to be released (with an interactive prompt in terminals). For brief
contention (another query finishing its final write) this resolves transparently.
For long-running sessions, the wait times out and the user must choose an
alternative. This is friction that didn't exist before.

**Session mapping is a new storage location.** User-local state in
`~/.local/share/jp/workspace/<workspace-id>/sessions/` adds new files alongside
the existing local conversation storage. This is another place where state lives
that users need to be aware of for debugging.

**Implicit `--last` in fresh sessions.** When a session has no mapping, `jp
query` implicitly resolves to the most recently modified conversation. This is
convenient for the common single-tab workflow, but it means two fresh sessions
opened simultaneously will target the same conversation. The lock wait behavior
prevents data corruption in this case, but the user experience (second session
waits, then times out) may be confusing. Once either session gets a mapping,
isolation is established.

## Alternatives

### Keep workspace-wide active conversation as fallback

When no session mapping exists and no `--last` is passed, fall back to the
workspace-wide `active_conversation_id`. This preserves backward compatibility.

Rejected because it ties the fallback to mutable global state. The implicit
`--last` fallback achieves similar convenience without a global singleton: it
reads the most recently modified conversation from conversation metadata
(read-only) rather than a shared `active_conversation_id` field that multiple
processes contend over.

### PPID-based session identity

Use the parent process ID instead of the session leader PID.

Rejected because PPID is unreliable. All tmux panes share the tmux-server PPID.
VS Code terminal tabs may share a PPID. Subshells have different PPIDs than
their parent shell. The session leader PID (`getsid`) is stable across all of
these cases because it looks through the process hierarchy to the original
session leader.

### No conversation locks

Rely on user discipline to avoid concurrent writes. Trust that users won't run
`jp query` on the same conversation from two terminals.

Rejected because it's a data integrity issue, not a UX preference. Two processes
appending to `events.json` concurrently produces corrupt data. Locks are
necessary for correctness.

### Lock per workspace instead of per conversation

A single lock for the entire workspace. Only one `jp query` can run at a time.

Rejected because it defeats the purpose of parallel conversations. Users want
independent queries running simultaneously on different conversations. Per-
conversation locks allow this.

## Non-Goals

- **Background execution.** Running conversations as detached background
  processes, attaching to running conversations from other terminals, and
  deferred prompt answering are future work that builds on the foundations
  established here (conversation targeting and locking).

- **Cross-machine sessions.** Session mappings are local to the machine.
  `$JP_SESSION` can be set to the same value on different machines, but the
  session-to-conversation mapping is not synchronized.

- **Automatic session detection beyond `getsid`.** Shell integration, terminal
  emulator APIs, or other mechanisms for automatic session detection are future
  work. The initial implementation uses `$JP_SESSION`, terminal env vars, and
  `getsid`.

- **Conversation access control.** Locks prevent concurrent writes. They do not
  implement permissions or multi-user access control. Any local user with
  filesystem access can read conversations.

## Risks and Open Questions

### Conversation discovery for `--last`

`--last` (and the implicit `--last` fallback for fresh sessions) finds the most
recently modified conversation. Today, conversations are sorted by
`last_activated_at`. With per-session tracking, this timestamp is updated
whenever a session operates on a conversation. This means `--last` returns the
conversation most recently used by *any* session, not the most recently
*created* conversation. This seems correct — "last" means "last worked on" — but
should be documented.

## Implementation Plan

### Phase 1: Session Identity

Add session identity resolution to `jp_cli`. Check `$JP_SESSION`, then
platform-specific detection (`getsid` on Unix, `GetConsoleWindow` on Windows),
then terminal env vars (`$TMUX_PANE`, `$WEZTERM_PANE`, `$TERM_SESSION_ID`,
`$ITERM_SESSION_ID`). Store the result in `Ctx` for use by commands.

No behavioral changes yet. The session identity is computed but not used.

Can be merged independently.

### Phase 2: Session-to-Conversation Mapping and Lock Infrastructure

Add the session mapping storage in
`~/.local/share/jp/workspace/<workspace-id>/sessions/`. When `jp query` operates
on a conversation, write the mapping. When `jp query --new` creates a
conversation, write the mapping.

`jp query` without `--new` reads the mapping to find the session's default
conversation. If no mapping exists, fall back to the most recently modified
conversation (implicit `--last`) and write the mapping.

Remove `active_conversation_id` from `ConversationsMetadata`.

Add `flock`-based locking via `ConversationLock`. Introduce the type-level
enforcement for conversation mutations.

### Phase 3: CLI Flags

Add `--id`, `--last`, and `--fork` to `jp query`. Implement the resolution
order:

1. `--id=<id>` — explicit conversation
2. `--last` — most recently modified conversation
3. `--fork[=N]` — fork source conversation, operate on fork
4. (none) — session's default conversation, falling back to most recently
   modified if no mapping exists

Update `jp conversation use` to write the session mapping instead of the
workspace-wide field.

### Phase 4: Stale File Cleanup

Add a background task (using the existing `TaskHandler` system) that runs on
every `jp` invocation and removes orphaned files:

- **Lock files**: Orphaned if no `flock` is held on them (attempt a non-blocking
  `flock`; if it succeeds, the file is orphaned).
- **Session mappings**: Orphaned if they point to a conversation that no longer
  exists.

## References

- `flock(2)` — POSIX advisory file locking used for conversation locks.
- `fd-lock` — Cross-platform advisory file locks (uses `LockFileEx` on Windows).
- `getsid(2)` — POSIX function for obtaining the session leader PID.
- `nix::unistd::getsid` — Rust binding via the `nix` crate.

### Platform Portability

| Concern          | Unix                | Windows                    | Rust crate    |
|------------------|---------------------|----------------------------|---------------|
| File locking     | `flock`             | `LockFileEx`               | `fd-lock`     |
| Session identity | `getsid(0)`         | `GetConsoleWindow()` HWND  | `nix`         |
| Session env vars | `$TMUX_PANE`, etc.  | `$WEZTERM_PANE`, etc.      | `std::env`    |

The `ConversationLock` abstraction hides the platform-specific locking
mechanism. Session identity uses the three-layer resolution described above,
which works on both platforms. The terminal env var layer is fully
cross-platform. The automatic detection layer uses platform-specific APIs behind
a common interface.

[RFD 039]: 039-conversation-trees.md
[RFD 052]: 052-workspace-data-store-sanitization.md
