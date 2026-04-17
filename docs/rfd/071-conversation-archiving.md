# RFD 071: Conversation Archiving

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-17

## Summary

This RFD adds conversation archiving to JP. Archived conversations are
physically moved out of the active storage directory, excluded from the
workspace index, and hidden from listings and pickers. Two new subcommands — `jp
conversation archive` and `jp conversation unarchive` — manage the lifecycle.

## Motivation

Long-running workspaces accumulate conversations. Most are no longer relevant
but contain valuable history that shouldn't be deleted. Today, the only options
are to keep every conversation in the active listing (noise) or delete them
permanently (data loss).

Archiving provides a middle ground: conversations are hidden from day-to-day
operations but preserved on disk and recoverable at any time. The existing
`expires_at` / `--tmp` mechanism handles truly ephemeral conversations that
should be garbage-collected. Archiving is for conversations the user wants to
keep but not see.

Without this feature, users resort to manual filesystem operations (moving
directories) or ignore the clutter, which degrades the picker and listing
experience as the workspace grows.

## Design

### User-Facing Behavior

#### Archiving

```sh
# Show a picker of conversations to archive
jp conversation archive
jp c a

# Archive specific conversations by ID
jp c archive jp-c123 jp-c456

# Archive multiple via picker
jp c archive ?

# Archive all pinned conversations
jp c archive +pinned
```

When archiving a pinned or active session conversation, JP prompts for
confirmation:

```
Archive the active conversation jp-c123? [y/n/?]
```

The prompt defaults to "no" and the conversation is skipped if declined.

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

The `--archived` flag switches `ls` to scan the archive partition. All existing
`ls` flags (`--sort`, `--limit`, `--full`, `--local`) work as expected.

#### Targeting Keywords

The conversation targeting system supports archive-related keywords:

| Keyword     | Alias | Description                                  |
|-------------|-------|----------------------------------------------|
| `archived`  | `a`   | Most recently archived conversation          |
| `+archived` | `+a`  | All archived conversations                   |
| `?archived` | `?a`  | Interactive picker of archived conversations |

The `archived` keyword resolves to the most recently archived conversation

The `archived` and `?archived` keywords work with `jp c use` to
unarchive-and-activate in one step:

```sh
# Pick an archived conversation, unarchive it, and activate it
jp c use ?archived

# Unarchive the most recently archived conversation and activate it
jp c use archived
```

### Storage Layout

Archived conversations are moved into a `.archive/` subdirectory within the
`conversations/` directory:

```
.jp/conversations/
├── 17729599457-active-conversation/
├── 17729621655/
└── .archive/
    ├── 17729596932-old-conversation/
    └── 17729598000-another-old-one/
```

The `.archive/` directory uses the existing dot-prefix convention that the
storage layer already skips during index scans (same pattern as `.trash/`,
`.old-*`, `.staging-*`). This means archived conversations are invisible to the
active index with zero filtering overhead.

Both workspace and user storage roots have independent `.archive/` directories.
The archive operation preserves which root a conversation belongs to.

### Backend Traits

The storage backend traits are extended to support archiving:

**`PersistBackend`** gains `archive(id)` and `unarchive(id)` methods. The
filesystem backend moves directories between the active and archive partitions.
The in-memory backend moves entries between two `HashMap`s. The null backend
discards both operations.

**`LoadBackend`** gains a `ConversationFilter` parameter on
`load_conversation_ids`:

```rust
#[derive(Debug, Default, Clone, Copy)]
pub struct ConversationFilter {
    pub archived: bool,
}
```

The default filter returns active conversations. Setting `archived: true` scans
the `.archive/` directories instead. This is a storage-level partition filter,
not a metadata filter — the cost is the same as the existing directory scan (no
per-conversation file I/O).

`load_conversation_metadata` searches both regular and archive partitions
transparently, so commands like `jp c show <archived-id>` work without special
handling.

The archived partition is not cached in the workspace's in-memory index.
`archived_conversations()` scans the filesystem on each call. This is the right
trade-off: archive operations are infrequent, the directory scan is cheap, and
caching would consume memory for conversations the user explicitly wanted out of
their working set. If repeated scans become a bottleneck, adding a cached
archived index to `State` is straightforward.

### Workspace Layer

`Workspace` exposes three methods:

- `archive_conversation(conv: ConversationMut)` — moves the conversation to the
  archive partition, removes it from the in-memory index.
- `unarchive_conversation(id: &ConversationId) -> Result<ConversationHandle>` —
  restores from the archive, inserts into the in-memory index.
- `archived_conversations()` — returns an iterator over archived conversation
  metadata, loaded on demand from the archive partition. Not cached in the
  workspace index.

### CLI Integration

`jp c archive` participates in the standard conversation resolution pipeline.
Its `conversation_load_request` returns real targets: a `Picker` when no IDs are
given, or the explicit targets when IDs are provided. The startup pipeline
resolves them, and the subcommand receives pre-resolved handles.

`jp c unarchive` returns `ConversationLoadRequest::none()` because its targets
are in the archive partition and cannot be resolved through the active index.
It handles resolution internally.

## Drawbacks

- **No toggle.** Archiving and unarchiving are separate subcommands. There is no
  single command that flips the state. This is a deliberate trade-off: the two
  operations have different user intents ("clean up" vs "I need that back"),
  different discovery paths, and different resolution needs (active index vs
  archive partition).

- **Directory rename under lock.** Archiving requires holding the conversation
  lock and then renaming the directory. If the process crashes between clearing
  dirty state and completing the rename, the conversation could be in an
  inconsistent state. The startup validation pass does not currently scan
  `.archive/` for recovery. A crash during archiving could leave the
  conversation missing from both partitions. The risk is low (the rename is a
  single syscall) but should be addressed by extending the validation pass to
  check `.archive/` in a follow-up.

## Alternatives

### Metadata Flag

Add an `archived: bool` field to `Conversation` metadata instead of moving
directories. Filter at the `conversations()` iterator level.

Rejected because: every consumer of `workspace.conversations()` would need to
filter, the index still pays the cost of tracking archived conversations in
memory, and session mappings would need awareness of the archived state. The
physical separation approach has zero cost for normal operations.

### `--archive` Flag on `jp c edit`

Implement archive as a property flag on the `edit` subcommand, alongside
`--pin`, `--local`, etc.

Rejected because: archiving is not a property mutation — it changes whether a
conversation exists in the working set, not how it appears. `--pin` and
`--local` modify metadata fields; `--archive` moves the entire conversation to a
different storage partition. Separate subcommands make this distinction clear to
the user and give each operation its own help text, argument handling, and
aliases.

### Dot-Prefixed Directory Names

Prefix archived conversations with `.archived-` in the main `conversations/`
directory instead of using a subdirectory.

Rejected because: it clutters the `conversations/` directory with dot-prefixed
entries, collides conceptually with `.old-*`/`.staging-*` which are transient
states, and the `.trash/` precedent already uses a subdirectory.

## Non-Goals

- **Automatic archiving.** Rules like "archive after N days of inactivity" that
  run without user intervention are out of scope.

- **Archive-specific metadata.** Archived conversations retain their original
  metadata. There is no `archived_at` timestamp or archive-specific fields. This
  means the `archived` keyword resolves by `last_activated_at` rather than by
  when the conversation was archived.

## Implementation Plan

### Phase 1: Storage and Workspace

Add `ConversationFilter`, `archive`/`unarchive` to the backend traits. Implement
for `FsStorageBackend` (directory moves), `InMemoryStorageBackend` (separate
`HashMap`), and `NullPersistBackend` (no-op). Add `archive_conversation`,
`unarchive_conversation`, and `archived_conversations` to `Workspace`. Add
parity tests.

Can be merged independently.

### Phase 2: CLI Subcommands

Add `jp c archive` and `jp c unarchive` subcommands with `--older-than` support.
Add `--archived` flag to `jp c ls`. Add `archived`/`+archived`/`?archived`
targeting keywords. Update `jp c use` to support unarchive-and-activate via the
`archived` keyword.

Depends on Phase 1.

## References

- [RFD 052: Workspace Data Store Sanitization][RFD 052] — `.trash/` pattern for
  invalid conversations
- [RFD 073: Layered Storage Backend for Workspaces][RFD 073] — backend trait
  architecture

[RFD 052]: 052-workspace-data-store-sanitization.md
[RFD 073]: 073-layered-storage-backend-for-workspaces.md
