# RFD 020: Parallel Conversations

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-19
- **Extended by**: [RFD 069](069-guard-scoped-persistence-for-conversations.md)

## Summary

This RFD replaces the workspace-wide "active conversation" with per-session
conversation tracking, adds conversation locks to prevent concurrent mutations,
and introduces `--id` and `--fork` flags on `jp query` for explicit conversation
targeting. Together, these changes allow users to run multiple conversations in
parallel across terminal sessions.

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

If none of these produce a session identity, JP operates without a session. In
interactive terminals, commands that need a conversation show an interactive
picker (see [`jp query` (no flags)](#jp-query-no-flags)). In non-interactive
environments, they fail with an error directing the user to `--id`, `--new`, or
`$JP_SESSION`. Session mappings cannot be persisted without a session identity.

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

The `history` array tracks conversations activated in this session, ordered most
recent first. Each entry records the conversation ID and activation timestamp.
The list is deduplicated — reactivating a conversation moves it to the front
rather than creating a duplicate entry. The active conversation is always
`history[0]`. This history enables `--id=previous` (the session's previously
active conversation) and improves the interactive picker by showing
session-relevant conversations first.

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
they are only removed by the [Stale File Cleanup](#stale-file-cleanup) task when
the conversation they point to no longer exists.

When `jp query` successfully operates on a conversation, that conversation is
pushed to the front of the session's history. When `jp query --new` creates a
new conversation, it becomes `history[0]`. The session's active conversation is
always `history[0]`.

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
    --id        open conversation picker.
    --id=<id>   target a specific conversation.
    --id=last   continue the most recently active conversation.
    --new       start a new conversation.
    --fork      branch from this one.
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

All existing mutation methods stay on `ConversationStream` and `Conversation`.
The change is that `get_events_mut`, `try_get_events_mut`,
`get_conversation_mut`, and `try_get_conversation_mut` take a
`&ConversationLock` parameter. Call sites add one argument:

```rust
// before
workspace.try_get_events_mut(&cid)?.add_config_delta(delta);
workspace.try_get_conversation_mut(&cid)?.title = Some(title);

// after
workspace.try_get_events_mut(&cid, &lock)?.add_config_delta(delta);
workspace.try_get_conversation_mut(&cid, &lock)?.title = Some(title);
```

This enforces the lock-before-mutate invariant at the API boundary. You cannot
call `get_events_mut` or `get_conversation_mut` without proof that the process
holds the lock. `ConversationLock` is held for the entire `jp query` run, so any
mutable reference obtained through it is guaranteed to be protected by the lock
for its entire lifetime.

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

#### `jp query --id[=<target>]`

The `--id` flag accepts a conversation ID, a reserved keyword, or no value:

| Form                       | Behavior                                           |
|----------------------------|----------------------------------------------------|
| `--id=<conversation-id>`   | Operate on a specific conversation by ID           |
| `--id=last-activated`      | Most recently activated conversation (any session) |
| `--id=last-created`        | Most recently created conversation                 |
| `--id=previous`            | Session's previously active conversation           |
| `--id` (no value)          | Show an interactive conversation picker            |

`last` is shorthand for `last-activated`. `previous` can be shortened to `prev`.

All forms wait if the target conversation is locked (see [Lock acquisition
behavior](#lock-acquisition-behavior)) and set the resolved conversation as the
session's default.

```sh
jp query --id=jp-c17528832001 "Follow-up question"  # explicit ID
jp query --id=last "Continue where I left off"       # most recent
jp query --id=previous "Back to the other thing"     # session's previous
jp query --id "Which conversation?"                  # interactive picker
```

`last` uses the `last_activated_at` timestamp across all conversations.
`last-created` uses the conversation's creation timestamp — useful when you ran
`--new` in another tab and want to pick it up here. `previous` reads the
session's activation history to find the conversation that was active before the
current one, similar to `cd -` in a shell. Fails if the session has no previous
conversation.

The keyword list is extensible — new keywords can be added without structural
changes, since conversation IDs use the `jp-c<timestamp>` format which cannot
collide with short alphabetic keywords.

The interactive picker (`--id` with no value) displays recent conversations with
their title, last message preview, and timestamp. Conversations previously
activated in the current session appear first, followed by remaining
conversations sorted by last activation time. This follows the same pattern as
the bare `--cfg` interactive browser described in [RFD 061].

In clap, `--id` uses `num_args = 0..=1` with `default_missing_value = "". The
empty string triggers the interactive picker; recognized keywords trigger their
respective resolution; any other value is treated as a conversation ID.

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

`--fork` can combine with `--id` to fork a specific conversation:

```sh
jp query --fork --id=last "Branch from the most recent conversation"
jp query --fork --id=jp-c17528832001 "Branch from this one"
jp query --fork --id -- "Pick a conversation to fork"
```

Without `--id`, `--fork` operates on the session's active conversation. Fails if
the session has no active conversation.

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
2. **No session mapping + interactive terminal**: Show the interactive
   conversation picker (same as `--id` with no value). The selected conversation
   becomes the session's default.
3. **No session mapping + non-interactive**: Fail with an error directing the
   user to `--id=<id>`, `--id=last`, `--new`, or `$JP_SESSION`.
4. **No conversations exist**: Fail with guidance to use `--new`.

In the common single-session workflow, step 1 applies after the first query.
Step 2 only triggers when a session has never operated on a conversation — for
example, opening a new terminal tab or a split pane. The picker lets the user
explicitly choose which conversation to continue, avoiding silent misrouting
when multiple conversations are active.

Fails if:
- No conversations exist in the workspace
- Conversation is locked by another session and the lock wait times out
- Non-interactive and no session mapping exists

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

### Terminal Title Updates

When `jp query` finishes a turn, JP writes the conversation's ID + title (or ID
if untitled) to the terminal title via the OSC 2 escape sequence:

```
\x1b]2;jp-c1234: Refactoring the config layer\x07
```

This makes the active conversation visible in the terminal's tab or title bar,
which helps users identify which conversation is running in which terminal
session. In split-pane workflows, the title is visible in each pane's header (in
tmux) or the tab bar (in terminals that update per-pane).

The title is updated at the end of each assistant turn, not continuously during
streaming. If the conversation has no title yet (e.g., the first turn before
title generation runs), JP uses a truncated form of the conversation ID.

This is a progressive enhancement — terminals that don't support OSC 2 ignore
the sequence. The `jp_term::osc` module already provides escape sequence
utilities.

## Drawbacks

**No sticky session without session identity.** Users without a controlling
terminal and without `$JP_SESSION` cannot persist a session mapping. In
interactive terminals, they see the conversation picker on every bare `jp query`
invocation. In non-interactive environments, they get an error and must use
`--id=<id>`, `--id=last`, or `--new` explicitly.

**Lock contention UX.** When a conversation is locked, JP waits up to 30 seconds
for the lock to be released (with an interactive prompt in terminals). For brief
contention (another query finishing its final write) this resolves transparently.
For long-running sessions, the wait times out and the user must choose an
alternative. This is friction that didn't exist before.

**Session mapping is a new storage location.** User-local state in
`~/.local/share/jp/workspace/<workspace-id>/sessions/` adds new files alongside
the existing local conversation storage. This is another place where state lives
that users need to be aware of for debugging.

**Interactive picker adds a step for fresh sessions.** Users who open a new
terminal and run bare `jp query` see a conversation picker instead of
automatically continuing their last conversation. This is one extra interaction
compared to an implicit fallback, but it prevents silent misrouting when
multiple conversations are active across sessions.

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

### Implicit `--last` fallback for fresh sessions

When no session mapping exists, silently resolve to the most recently modified
conversation. This avoids the picker step and preserves "just works" behavior
for users who only ever use one terminal.

Rejected because it silently picks the wrong conversation when multiple sessions
are active. For example: a user working in a split pane sees conversation A on
the left, but an agentic session in another tab recently touched conversation B.
The right pane's bare `jp query` silently continues conversation B. The user
doesn't notice until the response is wrong. The interactive picker makes this
choice explicit at the cost of one additional interaction.

### Separate `--last` flag

Add a dedicated `--last` flag instead of overloading `--id=last`.

Rejected in favor of consolidating conversation targeting into a single `--id`
flag with three modes (explicit ID, `last` keyword, interactive picker). This
reduces the flag surface area and follows the same pattern as `--cfg` in [RFD
061], where a bare flag triggers interactive mode and a valued flag provides
explicit input.

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

### Conversation discovery for `--id=last`

`--id=last` finds the most recently modified conversation. Today, conversations
are sorted by `last_activated_at`. With per-session tracking, this timestamp is
updated whenever a session operates on a conversation. This means `--id=last`
returns the conversation most recently used by *any* session, not the most
recently *created* conversation. This seems correct — "last" means "last worked
on" — but should be documented.

## Implementation Plan

### Phase 1: Session Identity

Add session identity resolution to `jp_cli`. Check `$JP_SESSION`, then
platform-specific detection (`getsid` on Unix, `GetConsoleWindow` on Windows),
then terminal env vars (`$TMUX_PANE`, `$WEZTERM_PANE`, `$TERM_SESSION_ID`,
`$ITERM_SESSION_ID`). Store the result in `Ctx` for use by commands.

### Phase 2: Handle-Based Workspace API

`ConversationHandle` and `ConversationGuard` types provide proof of conversation
existence at the type level. `acquire_conversation`, `events`/`metadata`/
`events_mut`/`metadata_mut` form the handle-based API. All production code
migrated. Old `get_events`/`get_conversation` methods remain for internal use.

### Phase 3a: Remove Global Active Conversation

`ConversationsMetadata` and `conversations/metadata.json` removed. The
workspace-wide singleton is replaced by per-session mappings (Phase 3b).

### Phase 3b: Session-to-Conversation Mapping and `ConversationNeed`

Session mapping storage in `~/.local/share/jp/workspace/<id>/sessions/`. `jp
query` and `jp conversation use` write the mapping. The startup pipeline reads
it to resolve the session's target conversation.

`ConversationNeed` enum (`None` / `Session`) and startup dispatch implemented.
Commands declare their need; `None`-need commands skip conversation loading
entirely.

### Phase 3c: Conversation Locks

`ConversationFileLock` in `jp_storage::lock` wraps OS-level advisory locks
(`flock` on Unix, `LockFileEx` on Windows) via `libc`/`windows-sys` directly
(not `fd-lock`, to avoid the `RwLockWriteGuard` lifetime issue and a
`windows-sys` version conflict).

`ConversationGuard` is a real struct: `ConversationHandle` + `Option<
ConversationFileLock>`. The lock is `Option` to support `guard_conversation`
(unlocked, for `--no-persist` and commands not yet migrated). `jp query`
acquires the lock with polling/timeout (`acquire_conversation_lock` in
`query.rs`). `$JP_LOCK_DURATION` overrides the default 30s timeout.

### Phase 4a: CLI Flags

Add `--id` and `--fork` to `jp query`. The `--id` flag supports three modes:

1. `--id=<id>` — explicit conversation
2. `--id=last` — most recently modified conversation
3. `--id` (no value) — triggers interactive picker

`--fork[=N]` composes with `--id` for forking specific conversations.

This phase also adds `ConversationNeed::New` and
`ConversationNeed::Explicit(id)` variants and the corresponding startup dispatch
paths.

### Phase 4b: Interactive Pickers

All interactive selection prompts that require design and UX work:

- **Conversation picker** (`--id` with no value, or bare `jp query` with no
  session mapping in an interactive terminal). Shows recent conversations with
  title, last message preview, timestamp. Session-relevant conversations first.
- **Lock contention prompt** (conversation locked by another session). Options:
  continue waiting, start new conversation, fork, cancel.

#### Non-interactive fallback

Implemented as part of Phase 4b. In non-interactive environments (stdin is not a
TTY):

- Bare `--id` (picker mode) fails with `NoConversationTarget` error and guidance
  directing to `--id=<id>`, `--id=last`, `--new`, or `$JP_SESSION`.
- `Session` path with no session mapping fails with the same error.

In interactive terminals, both cases currently fall through to `Session` (which
picks the most recent conversation). The interactive picker will replace this
fallback.

#### Conversation picker

MVP using `inquire::Select`. Two call sites:

1. **`conversation_need()` Picker path** (`--id` with no value in interactive
   terminal): `pick_conversation()` in `query.rs` shows the picker and
   returns `Explicit(id)`. Conversations sorted by `last_activated_at`
   (most recent first), session's active conversation pinned to top.
2. **`run_inner` Session path** (no session mapping in interactive terminal):
   `pick_session_conversation()` in `lib.rs` shows the same picker. Returns
   `Option<ConversationId>` — `None` on cancel falls through to the
   most-recent-conversation default. Only shown when >1 conversation exists.

Both pickers display `{id}  {title}` per line (or just `{id}` if untitled).

#### Lock contention prompt

When `acquire_conversation_lock` times out in an interactive terminal, it
shows an `inquire::Select` prompt with four options:

- **Continue waiting** — re-enters the polling loop with a fresh timeout.
  Recurses back to the prompt if the second timeout also expires.
- **Start a new conversation** — returns `LockOutcome::NewConversation`;
  `Query::run` creates a new conversation and acquires the lock on it.
- **Fork this conversation** — returns `LockOutcome::ForkConversation`;
  `Query::run` forks the locked conversation (read-only, no lock needed)
  and acquires the lock on the fork.
- **Cancel** — returns `Error::LockTimeout`, terminating the command.

In non-interactive environments (no TTY), `acquire_conversation_lock` still
fails with `Error::LockTimeout` immediately on timeout.

### Phase 5: Terminal Title Updates

Write the conversation title to the terminal title via OSC 2 at two points:

1. **At startup** when the target conversation is resolved — the user
   immediately sees which conversation they're in.
2. **Inside the title generation task** when a title is first generated — the
   update appears as soon as the title is ready, not at the end of the turn.

This is a progressive enhancement — terminals that don't support OSC 2 ignore
the sequence.

#### Implementation notes

- `jp_term::osc::set_title()` writes the OSC 2 sequence to stderr.
- `Query::run()` calls `set_terminal_title(id, title)` after acquiring the
  conversation metadata, gated on `ctx.term.is_tty`.
- `TitleGeneratorTask::sync()` calls `jp_term::osc::set_title()` after writing
  the title to the workspace.
- `jp_term` added as a dependency of `jp_task`.

### Phase 6: Stale File Cleanup {#stale-file-cleanup}

Runs synchronously at the end of every `jp` invocation (alongside ephemeral
conversation cleanup) via `Workspace::cleanup_stale_files()`:

- **Lock files**: `Storage::list_orphaned_lock_files()` attempts a non-blocking
  `flock` on each `.lock` file; if it succeeds, the file is orphaned and
  deleted. Uses `lock::is_orphaned_lock()` internally.
- **Session mappings**: `Storage::list_session_files()` lists all session
  mapping files. For each, the history is checked against the set of existing
  conversation IDs. Mappings where no conversation in the history still exists
  are deleted.

## Implementation Details

### Conversation Targeting {#conversation-targeting}

#### Problem

The current startup pipeline assumes every command operates on a conversation:

```txt
sanitize → load_conversation_index → load_config (incl. conversation config) → command.run()
```

`load_conversation_index` both scans conversation IDs and sets the active
conversation. `load_partial_config` reads the active conversation's events to
merge per-conversation config overrides. This coupling creates problems:

- **Commands that don't need conversations** (e.g., `jp config show`) are forced
  through conversation resolution anyway.
- **Conversation resolution requires user interaction** (picker) in some cases,
  but the startup pipeline runs before any command logic.
- **Config loading depends on knowing the active conversation**, but resolving
  the active conversation may need config (e.g., picker backend).

#### Resolution

Each command declares its conversation targeting needs via a trait method. The
startup pipeline uses this declaration to determine whether and how to load a
conversation before config is built.

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

The `IntoPartialAppConfig` trait (or a new companion trait) gains a method:

```rust
fn conversation_need(
    &self,
    conversation_ids: &[ConversationId],
    session: &Option<Session>,
) -> Result<ConversationNeed>;
```

This is called early in the startup pipeline, after filesystem scanning but
before config loading.

#### Startup Flow

The startup pipeline becomes:

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

The `AppConfig` in `Ctx` is always complete and read-only. The double-build only
occurs in the `Session` path with a picker fallback — a
first-invocation-per-terminal event that also requires `/dev/tty`.

#### Workspace API and Conversation Handles

`Workspace` becomes a pure storage manager with no opinion on which conversation
is "active." Conversation targeting is expressed through handle types that
provide proof of existence and access rights.

##### Type Hierarchy

```txt
ConversationGuard (owns both, for write commands like `jp query`)
├── ConversationHandle (proof of existence, for read access)
└── ConversationLock (proof of exclusive file lock, Phase 3c)
```

##### `ConversationHandle`

A move-only (non-`Copy`, non-`Clone`) type that proves a conversation exists in
the workspace index. Obtained exclusively through `Workspace` methods. Only one
handle should exist per conversation ID at a time — acquiring a second handle to
the same ID is a logic error (debug-asserted).

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

Combines a `ConversationHandle` with a `ConversationLock` (Phase 3c). Required
for mutable access to conversation state. Derefs to `ConversationHandle` so read
methods work transparently.

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

Methods that take `&ConversationHandle` are infallible — the handle is proof the
conversation exists. The implementation uses an internal `expect` that can only
fire if the workspace implementation has a bug (the handle invariant is
violated).

`events_mut` and `metadata_mut` require `&ConversationGuard` (which derefs to
`&ConversationHandle`), enforcing that write access requires both existence
proof and the file lock. Read-only methods accept `&ConversationHandle`
directly, so they work with either a bare handle or a guard.

`remove_conversation` consumes the `ConversationGuard` by value. After the call,
the handle is moved and the borrow checker prevents any further use. This
provides compile-time protection against use-after-remove.

> [!NOTE]
> Before Phase 3c adds `ConversationLock`, `ConversationGuard` can be a type
> alias for `ConversationHandle`, or the guard can hold a placeholder lock
> field. This allows the API shape to stabilize before locking is implemented.

#### Command Implementations

| Command                    | `conversation_need`     | Notes                                  |
|----------------------------|-------------------------|----------------------------------------|
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

- `flock(2)` — POSIX advisory file locking used for conversation locks.
- `LockFileEx` — Windows equivalent, used via `windows-sys`.
- `getsid(2)` — POSIX function for obtaining the session leader PID.

### Platform Portability

| Concern          | Unix               | Windows                   | Rust crate             |
|------------------|--------------------|---------------------------|------------------------|
| File locking     | `flock`            | `LockFileEx`              | `libc` / `windows-sys` |
| Session identity | `getsid(0)`        | `GetConsoleWindow()` HWND | `libc` / `windows-sys` |
| Session env vars | `$TMUX_PANE`, etc. | `$WEZTERM_PANE`, etc.     | `std::env`             |

The `ConversationFileLock` abstraction in `jp_storage::lock` hides the
platform-specific locking mechanism. We use `libc` and `windows-sys` directly
rather than `fd-lock` to avoid `RwLockWriteGuard` lifetime issues and a
`windows-sys` version conflict. Session identity uses the three-layer resolution
described above, which works on both platforms.

[RFD 039]: 039-conversation-trees.md
[RFD 052]: 052-workspace-data-store-sanitization.md
[RFD 061]: 061-interactive-config.md
