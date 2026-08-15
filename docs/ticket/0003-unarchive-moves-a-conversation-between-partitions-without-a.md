# T0003: `unarchive` moves a conversation between partitions without a lock

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-08-15

`Workspace::unarchive_conversation` moves a conversation directory out of the
archive partition into the live one without holding its conversation lock. All
three call sites go in bare: `unarchive.rs:41`, `use_.rs:163`, `use_.rs:248`.

Anything that reads the two partitions concurrently can therefore observe the
conversation in neither. `Workspace::cleanup_stale_files` is the case that
prompted this: it treats "absent from both partitions" as grounds for pruning a
session's history entry, and falls back to `lock_info` to spot a conversation
that is mid-write. That fallback finds nothing during an unarchive, because no
lock is taken.

## Scope

Cleanup's own exposure is already narrowed in `f6a3c957`: it re-reads the live
partition after the archive scan, so a single move in either direction is caught
by one of the scans. That does not cover multiple concurrent transitions, and it
does nothing for any other reader of the two partitions.

The underlying issue is that unarchive is the one partition-moving operation
that does not participate in the conversation-locking protocol. `archive` takes
a `ConversationMut` and therefore holds the lock; `unarchive` takes a bare
`&ConversationId`.

## Notes

Fixing this means changing the signature of `unarchive_conversation` to require
proof of the lock, in the shape `archive_conversation` already uses, and
updating the three call sites to acquire it. Worth checking whether the picker
paths in `use_.rs` can acquire a lock at the point they currently call through,
or whether they need restructuring first.
