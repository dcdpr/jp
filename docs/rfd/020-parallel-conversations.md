# RFD 020: Parallel Conversations

- **Status**: Draft
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
singleton stored in `ConversationsMetadata.active_conversation_id`. Every
`jp query` without `--new` operates on this conversation, regardless of which
terminal the command runs in.

This creates three problems:

**Terminal interference.** If you have two terminal tabs open in the same
workspace and run `jp query` in both, they operate on the same conversation.
The second query appends to the first's history. Tool calls from one session
can interleave with events from another. The conversation becomes incoherent.

**No parallelism.** You cannot work on two independent queries at the same
time within one workspace. Starting a new conversation in one terminal
(`jp query --new`) changes the active conversation for all terminals. The
other terminal's next `jp query` silently switches to the new conversation.

**Fragile state.** The `active_conversation_id` is a single point of
contention. If two processes write it simultaneously (e.g., both running
`jp query --new`), the last writer wins and the other session loses track of
its conversation.

These problems block any future work on background execution or multi-agent
workflows. Before conversations can run in parallel, each session needs its own
conversation identity and conversations need protection against concurrent
writes.

## Design

### Session Identity

A **session** is a terminal context — a tab, window, tmux pane, or scripting
environment. JP identifies sessions using three layers, checked in order:

1. **`$JP_SESSION` environment variable.** If set, this value is the session
   identity. It takes priority over everything else. The value is opaque to
   JP — any non-empty string works.

2. **Terminal-specific environment variables.** JP checks a list of known
   per-tab or per-pane env vars set by popular terminal emulators and
   multiplexers.

3. **Platform-specific automatic detection.** On Unix, `ttyname(stdin)`
   returns the TTY device path. On Windows, `GetConsoleWindow()` returns a
   per-tab window handle (HWND).

If none of these produce a session identity, JP has no session and commands
that need one fail with an error directing the user to `--id`, `--new`, or
`$JP_SESSION`.

#### `$JP_SESSION`

The environment variable is the explicit override for cases where automatic
detection is unreliable or unavailable:

- **CI/scripts**: Non-interactive environments without a terminal can set
  `JP_SESSION` to any stable identifier.
- **SSH**: SSH allocates a new PTY per connection. If you want session
  persistence across SSH reconnects, set `JP_SESSION` to a stable value.
- **Unusual terminal setups**: Any environment where the automatic detection
  produces wrong or missing results.

#### Terminal Environment Variables

Many terminal emulators and multiplexers set environment variables that
uniquely identify the current tab or pane. JP checks these before falling back
to platform-specific detection:

| Variable              | Terminal          | Granularity | Platform      |
|-----------------------|-------------------|-------------|---------------|
| `$TMUX_PANE`          | tmux              | Per-pane    | Cross-platform|
| `$WEZTERM_PANE`       | WezTerm           | Per-pane    | Cross-platform|
| `$TERM_SESSION_ID`    | macOS Terminal.app | Per-tab     | macOS         |
| `$ITERM_SESSION_ID`   | iTerm2            | Per-session | macOS         |

Only variables with **per-tab or per-pane** granularity are used. Per-window
variables like `$WT_SESSION` (Windows Terminal), `$KITTY_WINDOW_ID` (Kitty),
and `$ALACRITTY_WINDOW_ID` (Alacritty) are deliberately excluded because
multiple tabs in the same window share the value, which would cause sessions
to collide.

The `$TMUX_PANE` check is particularly valuable: it provides stable session
identity across tmux detach/reattach (where the TTY device path changes) with
no user configuration needed.

This list is extensible. New terminals can be added without an RFD.

#### Platform-Specific Automatic Detection

If no environment variable matches, JP uses a platform-specific mechanism:

**Unix: TTY device path.** `ttyname(stdin)` returns the PTY device path (e.g.,
`/dev/pts/3` on Linux, `/dev/ttys003` on macOS). Each terminal tab, window,
and tmux pane gets its own PTY, so the path is unique per tab. It is inherited
by subshells, so `bash -c "jp query ..."` sees the same identity as the parent
shell.

The main limitation is that PTY numbers recycle when terminals close. If you
close a terminal with `/dev/pts/3` and open a new one that happens to get
`/dev/pts/3`, the new terminal could see the old session mapping. This is
handled by **stale session detection**: JP compares the TTY device's creation
time (`stat` ctime) against the session mapping's last update time. If the TTY
was created after the mapping was written, the mapping is stale and is
discarded.

**Windows: Console window handle.** `GetConsoleWindow()` returns the HWND of
the console host (`conhost.exe` or `openconsole.exe`) backing the current tab.
Each tab in Windows Terminal, CMD, or PowerShell gets its own console host
process with a unique HWND. The handle remains stable across commands within
the same tab because the shell process keeps the console host alive.

Stale detection on Windows uses the same `updated_at` comparison, checking
whether the console host process is still alive.

### Session-to-Conversation Mapping

JP stores a mapping from session identity to conversation ID in the user-local
data directory:

```
~/.local/share/jp/workspace/<workspace-id>/sessions/
├── <session-key>.json
```

Where `<session-key>` is the `$JP_SESSION` value or a sanitized TTY device path
(e.g., `dev-pts-3`).

The mapping file contains:

```json
{
  "conversation_id": "jp-c17528832001",
  "updated_at": "2025-07-19T14:30:00.000Z"
}
```

The `updated_at` field is used for stale session detection when the session
identity comes from a TTY device path. When a stale mapping is detected (TTY
creation time is newer than `updated_at`), the mapping file is deleted
immediately.

When `jp query` successfully operates on a conversation, the mapping is updated.
When `jp query --new` creates a new conversation, the mapping is set to the new
conversation. The mapping is the session's "default conversation."

### Conversation Locks

Conversations are protected by **exclusive file locks** during write operations.
When `jp query` starts, it acquires an `flock`-based advisory lock on the
conversation. The lock is held for the entire query execution (including tool
calls and multi-cycle turns) and released when the command exits.

Lock files live in the user-local data directory:

```
~/.local/share/jp/workspace/<workspace-id>/locks/<conversation-id>.lock
```

The lock file contains the PID and session identity for diagnostic purposes:

```json
{
  "pid": 12345,
  "session": "/dev/pts/3",
  "acquired_at": "2025-07-19T14:30:00.000Z"
}
```

This content is informational only — the actual locking is done by the OS via
`flock`. If the process exits, crashes, or is killed (including SIGKILL), the
OS releases the lock automatically.

The lock file itself is eagerly deleted when the lock guard is dropped (normal
exit, Ctrl+C, panics). If the process is killed with SIGKILL, the lock file
orphans on disk but the actual `flock` is released by the OS. Orphaned lock
files are harmless and are cleaned up by a background task (see
[Stale File Cleanup](#stale-file-cleanup)).

#### What the lock protects

| Operation | Lock required? | Rationale |
|---|---|---|
| `jp query` (any variant) | Yes (exclusive) | Writes events to conversation |
| `jp conversation rm` | Yes (exclusive) | Deletes conversation data |
| `jp conversation edit` | Yes (exclusive) | Modifies conversation metadata |
| `jp conversation show` | No | Read-only |
| `jp conversation ls` | No | Read-only |
| `jp conversation fork` | No | Reads source, writes to a new conversation |
| `jp conversation grep` | No | Read-only |
| `jp conversation print` | No | Read-only |

When a lock cannot be acquired, JP reports which session holds it:

```
Error: Conversation jp-c17528832001 is locked by pid 12345 (session /dev/pts/3).

Suggestions:
    --id    target a specific conversation.
    --new   start a new conversation.
    --fork  branch from this one.
```

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
`ConversationLock` is held for the entire `jp query` run, so any
`&mut ConversationStream` obtained through it is guaranteed to be protected by
the lock for its entire lifetime.

The lock file is deleted in the `Drop` implementation of `ConversationLock`.

#### Non-persisting queries skip the lock

`jp --no-persist query` skips lock acquisition entirely. Since no events are
written back to disk, there is no write-write conflict. The query reads
conversation state as a snapshot.

On Unix, reading a JSONL file while another process appends is safe — the reader
sees a consistent prefix of complete lines. The snapshot may be missing events
written after the read, but for a non-persisting query this is acceptable.

This means `jp --no-persist query --id=<id>` works on locked conversations,
which is useful for inspecting a conversation's state while another session is
actively using it.

### CLI Changes

#### `jp query --id=<id>`

Operate on a specific conversation by ID. Fails if the conversation is locked.

```bash
$ jp query --id=jp-c17528832001 "Follow-up question"
```

This is the explicit targeting mechanism. It does not depend on session identity
and works in all environments.

#### `jp query --last`

Operate on the most recently modified conversation in the workspace. Fails if
the conversation is locked. Sets the conversation as the session's default.

```bash
$ jp query --last "Continue where I left off"
```

This is intended for fresh sessions where no session-to-conversation mapping
exists yet. It uses the `last_activated_at` timestamp on conversations to find
the most recent one.

#### `jp query --fork[=N]`

Fork the session's active conversation and start a new turn on the fork. The
forked conversation becomes the session's default.

```bash
$ jp query --fork "Try a different approach"
$ jp query --fork=3 "Redo the last 3 turns differently"
```

`N` is optional. If specified, the fork keeps the last `N` turns of the source
conversation. If omitted, the fork keeps the entire history.

This is shorthand for `jp conversation fork --activate` followed by `jp query`.
The fork reads the source conversation (no lock needed) and creates a new one.

`--fork` can combine with `--last` or `--id` to fork a specific conversation:

```bash
$ jp query --fork --last "Branch from the most recent conversation"
$ jp query --fork --id=jp-c17528832001 "Branch from this one"
```

Without `--last` or `--id`, `--fork` operates on the session's active
conversation. Fails if the session has no active conversation.

#### `jp query` (no flags)

Continue the session's active conversation. This is the common case for
ongoing work within a single terminal.

```bash
$ jp query "Next question"
```

Fails if:
- No session identity (no TTY, no `$JP_SESSION`)
- Session has no active conversation mapping
- Conversation is locked by another session

The error message directs the user to `--new`, `--last`, `--id`, or `--fork`.

#### `jp query --new`

Unchanged. Creates a new conversation and starts a turn. The new conversation
becomes the session's default. Always succeeds (no lock contention — new
conversations have no other users).

#### `jp conversation use <id>`

Sets the session's default conversation without starting a query. The
conversation does not need to be unlocked (this is a session mapping change,
not a write to the conversation).

```bash
$ jp conversation use jp-c17528832001
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

**Requires session identity.** Users without a TTY and without `$JP_SESSION`
cannot use bare `jp query` — they must pass `--id` or `--new` every time. This
is a worse experience than the current "just works with the active conversation"
behavior.

**Lock contention UX.** When a conversation is locked, the error message is
clear, but the user still has to choose an alternative (fork, new, wait). This
is friction that didn't exist before.

**Session mapping is a new storage location.** User-local state in
`~/.local/share/jp/workspace/<workspace-id>/sessions/` adds new files alongside
the existing local conversation storage. This is another place where state lives
that users need to be aware of for debugging.

**Breaking change.** Removing `active_conversation_id` changes the default
behavior of `jp query` without flags. Existing users who rely on the implicit
active conversation will see errors in fresh terminals until they adopt
`--last`, `--id`, or `$JP_SESSION`.

## Alternatives

### Keep workspace-wide active conversation as fallback

When no session mapping exists and no `--last` is passed, fall back to the
workspace-wide `active_conversation_id`. This preserves backward compatibility.

Rejected because it reintroduces the problem this RFD solves. If `jp query`
silently falls back to a global conversation, two sessions without mappings end
up on the same conversation again. The fallback makes the new behavior
unpredictable — sometimes you get per-session isolation, sometimes you don't.

### PPID-based session identity

Use the parent process ID instead of TTY device paths.

Rejected because PPID is unreliable. All tmux panes share the tmux-server PPID.
VS Code terminal tabs may share a PPID. Subshells have different PPIDs than
their parent shell. TTY device paths are more stable across these cases.

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

- **Automatic session detection beyond TTY.** Shell integration, terminal
  emulator APIs, or other mechanisms for automatic session detection are future
  work. The initial implementation uses `$JP_SESSION` and TTY device paths.

- **Conversation access control.** Locks prevent concurrent writes. They do not
  implement permissions or multi-user access control. Any local user with
  filesystem access can read conversations.

## Risks and Open Questions

### Conversation discovery for `--last`

`--last` finds the most recently modified conversation. Today, conversations
are sorted by `last_activated_at`. With per-session tracking, this timestamp
is updated whenever a session operates on a conversation. This means `--last`
returns the conversation most recently used by *any* session, not the most
recently *created* conversation. This seems correct — "last" means "last
worked on" — but should be documented.

## Implementation Plan

### Phase 1: Session Identity

Add session identity resolution to `jp_cli`. Check `$JP_SESSION`, then
terminal env vars (`$TMUX_PANE`, `$WEZTERM_PANE`, `$TERM_SESSION_ID`,
`$ITERM_SESSION_ID`), then platform-specific detection (`ttyname` on Unix,
`GetConsoleWindow` on Windows). Store the result in `Ctx` for use by commands.

No behavioral changes yet. The session identity is computed but not used.

Can be merged independently.

### Phase 2: Session-to-Conversation Mapping and Lock Infrastructure

Add the session mapping storage in
`~/.local/share/jp/workspace/<workspace-id>/sessions/`. When
`jp query` operates on a conversation, write the mapping. When `jp query --new`
creates a conversation, write the mapping.

`jp query` without `--new` reads the mapping to find the session's default
conversation. If no mapping exists, error with guidance to use `--new`,
`--last`, or `--id`.

Remove `active_conversation_id` from `ConversationsMetadata`.

Add `flock`-based locking via `ConversationGuard`. Introduce the type-level
enforcement for conversation mutations.

### Phase 3: CLI Flags

Add `--id`, `--last`, and `--fork` to `jp query`. Implement the resolution
order:

1. `--id=<id>` — explicit conversation
2. `--last` — most recently modified conversation
3. `--fork[=N]` — fork source conversation, operate on fork
4. (none) — session's default conversation

Update `jp conversation use` to write the session mapping instead of the
workspace-wide field.

### Phase 4: Stale File Cleanup

Add a background task (using the existing `TaskHandler` system) that runs on
every `jp` invocation and removes orphaned files:

- **Lock files**: Orphaned if no `flock` is held on them (attempt a
  non-blocking `flock`; if it succeeds, the file is orphaned).
- **Session mappings**: Orphaned if they point to a conversation that no longer
  exists.

## References

- `flock(2)` — POSIX advisory file locking used for conversation locks.
- `fd-lock` — Cross-platform advisory file locks (uses `LockFileEx` on
  Windows).
- `ttyname(3)` — POSIX function for resolving TTY device paths.
- `nix::unistd::ttyname` — Rust binding via the `nix` crate.

### Platform Portability

| Concern          | Unix                | Windows                    | Rust crate    |
|------------------|---------------------|----------------------------|---------------|
| File locking     | `flock`             | `LockFileEx`               | `fd-lock`     |
| Session identity | `ttyname(stdin)`    | `GetConsoleWindow()` HWND  | platform code |
| Session env vars | `$TMUX_PANE`, etc.  | `$WEZTERM_PANE`, etc.      | `std::env`    |

The `ConversationLock` abstraction hides the platform-specific locking
mechanism. Session identity uses the three-layer resolution described above,
which works on both platforms. The terminal env var layer is fully
cross-platform. The automatic detection layer uses platform-specific APIs
behind a common interface.
