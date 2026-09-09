# Unlinking a conversation lock file on release can leave two holders

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-09-10
- **Label**: domain=storage
- **Label**: package=jp_storage
- **Label**: package=jp_workspace
- **Label**: type=bug

Conversation lock files are unlinked while other processes may be acquiring
them, so two processes can hold write exclusion on the same conversation at the
same time.
The exclusion is advisory on an *inode*, and the pathname is what every
acquisition resolves; unlinking separates the two.

## The interleaving

`FsResourceGuard::drop` (`crates/jp_storage/src/resource_lock.rs`) closes the
file handle, releasing the `flock`, and then unlinks the path:

1. A closes its handle.
   The OS lock on inode I is released; the path still resolves to I.
2. B opens the path, gets I, and takes the lock.
   B is a legitimate holder.
3. A unlinks the path.
   B now holds a lock on an unlinked inode, and the presence marker for the
   conversation is gone.
4. C opens the path, creating inode J, and takes the lock.

B and C both report success and can write the same conversation with no
exclusion between them.
`is_conversation_locked` also reports the conversation unlocked while B holds
it, because `lock_file_path` decides on `exists()`.

The window in step 2 is the few instructions between the close and the unlink,
so `try_lock` narrows it but does not close it.
A *blocking* acquisition turns it into a certainty, which is why
`FsResourceLocker::lock` refuses to run with `remove_on_drop` set.

## Two unlink sites, not one

- Guard drop, above.
  `try_lock_conversation` opts into it with `with_remove_on_drop`, because the
  lock file doubles as the presence marker that `is_conversation_locked` and the
  orphan scan read.
- `Workspace::cleanup_stale_files`
  (`crates/jp_workspace/src/session_mapping.rs`) unlinks every path returned by
  `Storage::list_orphaned_lock_files`, and runs at the end of every invocation.
  The orphan test is itself an open-then-`flock`, so it can unlink a file
  another process locked between the two.

## Plausible input

Two `jp` invocations against the same conversation, one releasing as the other
acquires — `jp query` finishing while a second `jp query` or `jp c rm` waits on
the same id.
Contention is ordinary; landing inside the close/unlink window is rare.
The damage neither stays put nor shows itself: two unserialized writers against
a conversation's durable state, reported as success by both.

## Direction

Make conversation lock files stable inodes, never unlinked while acquisition can
run, and find another representation for the presence marker that
`is_conversation_locked` and the orphan scan depend on.
That means:

- Drop `with_remove_on_drop` from `try_lock_conversation`, and with it the
  option on `FsResourceLocker` if nothing else wants it.
- Decide what "no lock file exists" means once files persist —
  `Storage::lock_file_path` currently signals the write location through
  `Err(path)` on the strength of `exists()`.
- Stop `cleanup_stale_files` from unlinking lock files, or bound when it may.

RFD 106 designs the ID allocator lock around exactly this property ("The lock
file is never unlinked, so it is a stable inode and the release-then-unlink
hazard does not apply to it") and scopes the conversation-lock case out ("that
is a pre-existing bug, out of scope here").
This ticket is that case.

## Testing

A regression test has to control the interleaving rather than run the steps in
sequence — the existing `test_fs_remove_on_drop` acquires and drops in order
and never overlaps.
Something that holds the guard's drop between the close and the unlink while a
second acquisition runs, or three coordinated processes.
