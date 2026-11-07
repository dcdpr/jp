# RFD 071: Conversation Archiving

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-17
- **Summary**: Adds `jp conversation archive` and `unarchive` to move inactive
  conversations out of active listings while preserving them on disk.

## Summary

This RFD adds `jp conversation archive` and `jp conversation unarchive`.
Archiving moves a conversation out of the active storage directory, excluding it
from the workspace index and hiding it from default listings and pickers;
unarchiving restores it to the active directory.

## Motivation

Long-running workspaces accumulate finished conversations whose history is still
worth keeping.
Without archiving, users must either leave every conversation in the active
listing (cluttering pickers and listings as the workspace grows), delete them
permanently (losing data), or move directories by hand.

The `expires_at` / `--tmp` mechanism handles ephemeral conversations that should
be garbage-collected.
Archiving covers conversations the user wants to keep on disk and recover at any
time while hiding them from day-to-day operations.

## Design

### User-Facing Behavior

#### Archiving

```sh
# Archive the session's active conversation (same fallback chain as
# `jp c show`: session active → conversation.default_id → picker)
jp conversation archive
jp c a

# Archive specific conversations by ID
jp c archive jp-c123 jp-c456

# Archive multiple via picker
jp c archive ?

# Archive all pinned conversations
jp c archive +pinned

# Archive a range of conversations by creation date. Both bounds accept a
# conversation ID (uses its creation timestamp), a relative duration
# (3w, 30d, 6h), or an absolute date (2026-01-01). The range is half-open:
# --created-since is inclusive, --created-before is exclusive.
jp c archive --created-since 3w --created-before 1d
jp c archive --created-since jp-c123

# Archive every conversation unused since a given time (last_activated_at,
# not creation date). Accepts the same syntax as --created-since.
jp c archive --inactive-since 30d

# Filters AND-compose; this archives conversations created in the last
# month that have been idle for at least a week.
jp c archive --created-since 30d --inactive-since 7d

# Skip the per-conversation confirmation prompt for pinned/active entries.
jp c archive --inactive-since 6mo --yes
```

Archiving a pinned conversation or the session's active conversation prompts for
confirmation first, printing the conversation's details so bulk selections can
be reviewed one entry at a time:

```
Archiving conversation jp-c123

             ID  jp-c123
          Title  Rework the config pipeline
         Events  12
          Turns  6
 Latest Message  2 hours ago (2026-09-07 18:12:04)
 Last Activated  Currently Active
         Pinned  No
          Local  Yes

Archive this conversation? [y,n,?]
```

The prompt defaults to "no", and declining skips the conversation.
`--yes` (`-y`) suppresses the prompt for batch use.

`jp c archive`, `jp c rm`, and `jp c show` (which omits the heading) render this
detail block via `DetailsFmt` in `jp_cli::format::conversation`.
Both `archive` and `rm` run the prompt through
`jp_cli::shared::confirm::confirm_conversation_action`.

#### Unarchiving

```sh
# Show a picker of archived conversations to restore
jp conversation unarchive
jp c ua

# Unarchive specific conversations by ID
jp c unarchive jp-c123 jp-c456

# Unarchive multiple via picker
jp c unarchive ?

# Unarchive all archived conversations
jp c unarchive +archived
```

Non-archived IDs passed to `unarchive` are skipped with a warning.

#### Listing

```sh
# List archived conversations
jp c ls --archived
```

`--archived` switches `ls` to scan the archive partition and composes with the
other `ls` flags (`--sort`, `--limit`, `--full`, `--local`).

#### Targeting Keywords

The conversation targeting system supports three archive keywords:

| Keyword     | Alias | Description                                  |
| ----------- | ----- | -------------------------------------------- |
| `archived`  | `a`   | Most recently archived conversation          |
| `+archived` | `+a`  | All archived conversations                   |
| `?archived` | `?a`  | Interactive picker of archived conversations |

Passing `archived` or `?archived` to `jp c use` unarchives and activates the
conversation in one step:

```sh
# Pick an archived conversation, unarchive it, and activate it
jp c use ?archived

# Unarchive the most recently archived conversation and activate it
jp c use archived
```

### Storage Layout

Archived conversations live in a `.archive/` subdirectory under
`conversations/`:

```
.jp/conversations/
├── 17729599457-active-conversation/
├── 17729621655/
└── .archive/
    ├── 17729596932-old-conversation/
    └── 17729598000-another-old-one/
```

Because the storage layer skips dot-prefixed entries during index scans
(alongside `.trash/`, `.old-*`, and `.staging-*`), `.archive/` is invisible to
the active index without extra filtering.

Workspace and user storage roots each have their own `.archive/` directory, and
archiving keeps a conversation in its original root.

### Backend Traits

**`PersistBackend`** gains `archive(id)` and `unarchive(id)` methods.
The filesystem backend moves directories between the active and archive
partitions, the in-memory backend moves entries between two `HashMap`s, and the
null backend discards both operations.

**`LoadBackend`** gains a `ConversationFilter` parameter on
`load_conversation_ids`:

```rust
#[derive(Debug, Default, Clone, Copy)]
pub struct ConversationFilter {
    pub archived: bool,
}
```

The default filter returns active conversations; setting `archived: true` scans
the `.archive/` directories instead.
Because this filters by storage partition rather than reading metadata, it costs
a single directory scan with no per-conversation file I/O.

`load_conversation_metadata` searches both active and archive partitions
transparently, so commands like `jp c show <archived-id>` work without special
handling.

### Workspace Layer

`Workspace` exposes three methods:

- `archive_conversation(conv: ConversationMut)`: moves the conversation to the
  archive partition and removes it from the in-memory index.
- `unarchive_conversation(id: &ConversationId) -> Result<ConversationHandle>`:
  restores the conversation from the archive partition and inserts it into the
  in-memory index.
- `archived_conversations()`: returns an iterator over archived conversation
  metadata, loaded on demand from the archive partition.

`archived_conversations()` scans the filesystem on each call rather than caching
the archive partition in the workspace's in-memory index.
Archive operations are infrequent, the directory scan is cheap, and caching
would keep conversations in memory after the user removed them from the working
set.
If repeated scans become a bottleneck, `State` can cache an archived index.

### CLI Integration

`jp c archive` participates in the standard conversation resolution pipeline.
Its `conversation_load_request` operates in two modes:

- **Filter mode** (any of
  `--created-since`/`--created-before`/`--inactive-since` set): returns
  `ConversationLoadRequest::none()`.
  The subcommand iterates the workspace and selects matching conversations
  internally.
- **Direct mode** (no filter flags): returns `explicit_or_session(target)`,
  which resolves explicit IDs when provided and otherwise follows the same
  fallback chain as `jp c show` (session active → `conversation.default_id` →
  picker).
  The startup pipeline resolves the targets and passes pre-resolved handles to
  the subcommand.

`jp c unarchive` always returns `ConversationLoadRequest::none()` and resolves
targets internally, because archived conversations live outside the active
index.

`jp c archive` and `jp c rm` share the `--created-since`/`--created-before`
range filter via a flattened `CreationRange` args struct in
`crates/jp_cli/src/cmd/time.rs`, keeping their creation-range semantics in
lockstep.

> [!NOTE]
> The creation-range flags shipped as `--from`/`--until` and were renamed to
> `--created-since`/`--created-before` when `jp c print`, `jp c compact`, and
> `jp c fork` introduced `--from`/`--to` as a turn-range selector.
> Because both flag families accept durations and absolute dates, distinct names
> separate conversation selection from turn selection.
> The range remains half-open on conversation creation time.
> See [Indexing and Counting Conventions] for turn ranges.

`jp c archive` (like `jp c rm`) acquires the conversation lock before rendering
the confirmation prompt and holds it until the user responds.
This guarantees that the displayed details match the state being archived;
otherwise, another terminal could append turns while the prompt is open, and
confirming would archive work the user never saw.
As a trade-off, bulk archiving acquires the lock for each conversation in turn,
including ones the user declines.

## Drawbacks

- **No toggle.** Archiving and unarchiving are separate subcommands rather than
  a single command that flips the state.
  The two operations have different user intents ("clean up" vs "I need that
  back"), discovery paths, and resolution targets (active index vs archive
  partition).

- **Directory rename under lock.** Archiving clears dirty state and renames the
  conversation directory while holding the conversation lock.
  Because the startup validation pass does not scan `.archive/` for recovery, a
  crash before the rename completes could leave the conversation in an
  inconsistent state or missing from both partitions.
  The rename is a single syscall, so the window is small, and a follow-up can
  extend the validation pass to check `.archive/`.

## Alternatives

### Metadata Flag

Add an `archived: bool` field to `Conversation` metadata instead of moving
directories, and filter at the `conversations()` iterator level.

Rejected because every consumer of `workspace.conversations()` would have to
filter archived entries, the in-memory index would still track them, and session
mappings would have to check the archived state.
Separating partitions on disk adds zero overhead to active operations.

### `--archive` Flag on `jp c edit`

Implement archiving as a property flag on the `edit` subcommand, alongside
`--pin` and `--local`.

Rejected because `--pin` and `--local` modify metadata fields on a conversation
in the working set, whereas `--archive` moves the conversation to a different
storage partition and removes it from the working set.
Separate subcommands make this distinction clear and give each operation its own
help text, argument handling, and aliases.

### Dot-Prefixed Directory Names

Prefix archived conversation directories with `.archived-` inside
`conversations/` instead of moving them into a subdirectory.

Rejected because this clutters `conversations/` with dot-prefixed entries,
conflates persistent archives with transient `.old-*` and `.staging-*`
directories, and departs from the `.trash/` subdirectory precedent.

## Non-Goals

- **Automatic archiving.** Rules like "archive after N days of inactivity" that
  run without user intervention are out of scope.

- **Archive-specific metadata.** Archived conversations retain their original
  metadata without an `archived_at` timestamp or other archive-specific fields.
  Consequently, the `archived` keyword resolves by `last_activated_at` rather
  than by when the conversation was archived.

## Implementation Plan

### Phase 1: Storage and Workspace

Add `ConversationFilter` and `archive`/`unarchive` to the backend traits,
implemented for `FsStorageBackend` (directory moves), `InMemoryStorageBackend`
(separate `HashMap`), and `NullPersistBackend` (no-op).
Add `archive_conversation`, `unarchive_conversation`, and
`archived_conversations` to `Workspace`, along with parity tests.

Can be merged independently.

### Phase 2: CLI Subcommands

Add the `jp c archive` and `jp c unarchive` subcommands, the `--archived` flag
on `jp c ls`, and the `archived`, `+archived`, and `?archived` targeting
keywords.
Update `jp c use` to unarchive and activate conversations via the `archived`
keyword.

`jp c archive`'s selection flags ship as `--created-since`/`--created-before`
(creation-date range, extracted into a shared `CreationRange` args struct reused
by `jp c rm`) and `--inactive-since` (last-activity threshold).
All three accept the `TimeThreshold` syntax (conversation ID, relative duration,
or absolute date) and AND-compose.

Depends on Phase 1.

## References

- [RFD 052: Workspace Data Store Sanitization][RFD 052]: `.trash/` pattern for
  invalid conversations
- [RFD 073: Layered Storage Backend for Workspaces][RFD 073]: backend trait
  architecture
- [Indexing and Counting Conventions]: turn-range selector conventions that
  prompted renaming `--from`/`--until`

[Indexing and Counting Conventions]: ../wiki/00dxyba-indexing-and-counting-conventions.md
[RFD 052]: 052-workspace-data-store-sanitization.md
[RFD 073]: 073-layered-storage-backend-for-workspaces.md
