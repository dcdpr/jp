# RFD 106: Collision-Free Conversation ID Allocation

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-08-05
- **Extends**: [RFD 031], [RFD 054], [RFD 073]
- **Summary**: Allocates collision-free conversation IDs by claiming the first
  unoccupied decisecond slot under a storage lock.

## Summary

Conversation IDs encode a decisecond timestamp, so two conversations created
within the same 100 ms window receive the same ID and one silently overwrites
the other on disk.
This RFD keeps the ID format, replaces `Utc::now()` with an allocator that
claims the first unoccupied decisecond slot at or after the current time, and
records an exact `created_at` timestamp in conversation metadata.

## Motivation

`ConversationId` is a `DateTime<Utc>` truncated to deciseconds and rendered as
`jp-c17457886043`.
`ConversationId::default()` returns `Utc::now()` truncated to deciseconds, and
every conversation creation path calls it.

`jp conversation fork A B` loops over its sources in a single process
(`fork_each` in `fork.rs`), calling
`create_and_lock_conversation_with_projection` on each iteration.
Because persisting a conversation is three small JSON writes, both iterations
finish within the same decisecond on local storage:

1. The second iteration calls `entry(id).insert_entry(OnceLock::new())`
   (`create_conversation_with_projection` in `jp_workspace/src/lib.rs`),
   replacing the first fork's in-memory entry and discarding it with `let _err
   =`.
2. The first fork dropped its `ConversationLock` at the end of its iteration, so
   `try_lock` succeeds for the second fork without error.
3. On persist, `reconcile_conversation_dir` (`jp_storage/src/lib.rs`) renames
   the first fork's directory to the second fork's name and removes any other
   matching directories with `remove_dir_all`.

The command exits with status 0, prints no error or warning, and loses the first
fork.

This collision requires neither concurrency nor unusual timing.
`fork --compact` avoids it only because its per-iteration LLM call (`fork.rs`)
separates creations by seconds.
Cross-process collisions are probabilistic, and [RFD 050] makes them routine:
`conversation new` and `fork` are designed to be driven from scripts, and `fork`
prints one ID per source, so a script can receive the same ID twice.

The existing test (`test_conversation_fork` in `fork_tests.rs`) forks a single
source against a stubbed epoch clock, leaving the multi-source path untested.

## Design

### User-facing behavior

The ID format is unchanged: `jp-c17457886043` remains an eleven-digit decisecond
timestamp, directory names are unchanged, and existing conversations keep their
IDs.

The allocator guarantees:

> Conversations auto-allocated and persisted within one allocation domain (a
> single machine's pair of storage roots) receive distinct IDs.

This introduces three behavioral changes:

- **Every created conversation gets a distinct ID.** A burst of N creations
  yields N distinct IDs in ascending order, skipping occupied slots.
- **IDs allocated within one process are strictly increasing.** Across process
  restarts, clock adjustments, and conversations pulled from git, IDs stay
  unique on the local machine but do not guarantee creation order.
  `created_at` records the exact, undrifted creation timestamp, though
  cross-machine ordering still depends on clock agreement.
- **A burst that pushes the allocator too far ahead of wall-clock time fails
  with a clear error** rather than succeeding with an inaccurate timestamp.

Interfaces that sort or filter by creation time continue to read
`id.timestamp()`, including `--sort created`, `--created-since`,
`--created-before`, and the `newest` target (`cmd/target.rs`).
Because IDs are approximate, a backwards clock adjustment can leave `newest`
pointing at an earlier conversation.
(`latest` is unaffected because it sorts on `last_activated_at`.)

`metadata.json` gains an exact `created_at` field, and `jp conversation show`
adds a `Created` row backed by a top-level `created_at` key in its `-F json`
payload (`DetailsFmt::json` in `format/conversation.rs`).
`jp conversation ls -F json`, which previously derived `created_at` from the ID
(`payload` in `ls.rs`), switches to the stored value and falls back to
`id.timestamp()` when the field is absent, so both commands report the same
creation time.

### Preserving the ID format

Seven behaviors read the ID's timestamp: `--sort created` (`ls.rs`, `grep.rs`),
the `last_event_at` fallback for an empty conversation (`ls.rs`), the
`created_at` key in `conversation ls -F json` (`payload` in `ls.rs`), the
`--created-since` / `--created-before` thresholds (`CreationRange::matches` in
`time.rs`), `expires_at = id.timestamp() + ttl` (`create_new_conversation` in
`query.rs`), `newest` resolution (`target.rs`), and the seed for
`ConversationStream::created_at` (`jp_storage/load.rs`,
`jp_workspace/src/lib.rs`).
Changing the ID format (adding a random suffix, switching to ULIDs, or
increasing timestamp precision) would break some or all of these callers and
invalidate existing conversation directory names on disk.
Allocating distinct decisecond slots within the existing encoding fixes the
collision without breaking this contract.

### The allocator

The requirement is uniqueness, not global ordering: IDs cannot increase
monotonically across machines because a teammate's conversation can arrive
through git carrying any timestamp.
Instead of advancing past the maximum stored ID, the allocator claims the first
unoccupied slot at or after the current decisecond, replacing
`ConversationId::default()` in the creation path:

```text
candidate = max(now_decisecond, process_high_water + 1)
while candidate is occupied:
    candidate += 1
claim candidate
```

A slot is **occupied** when a conversation directory exists for it in any
partition of any storage root, or when its conversation lock is currently held.
Neither check suffices on its own: only the conversation lock covers a slot
between claiming the ID and persisting the directory, and only the directory
covers the slot after the lock is released.
Because `ConversationLock` persists the conversation before releasing its lock
guard, the two states overlap without a gap, and the implementation must
preserve that ordering.
Observing both states without a race window is handled in
[Exclusion](#exclusion).
`process_high_water` advances on each allocation so a multi-source fork never
re-issues a slot that it has claimed but not yet persisted.

**Occupancy comes from storage, not from a cursor file.** Allocated IDs are
already durably recorded in directory names, and `scan_conversation_ids`
(`jp_storage/load.rs`) already reads them.
Each allocation scans the existing IDs into a `HashSet` while holding the
allocator lock.
The scan cannot be cached across allocations in a process: a snapshot taken
before the first allocation becomes stale as soon as another process persists a
conversation, which is the race covered in [Exclusion](#exclusion).
Probing candidates individually is no cheaper: because directories are named
`{id}-{title}`, checking a single candidate still requires a `read_dir` call
(`conversation_dirs_for_id` in `jp_storage/src/lib.rs`).

The scan covers both storage roots and both partitions (active and `.archive/`).
`load_conversation_index` reads only one partition at a time
(`jp_storage/load.rs`), so building the occupancy set from the workspace index
alone would miss archived IDs and allow `unarchive_conversation` to collide with
an active directory.

Because occupancy is a read operation, it belongs on `LoadBackend`, which
already owns reading conversations behind the storage abstraction from [RFD
073].
`FsStorageBackend` implements it with a union directory scan, and
`InMemoryStorageBackend` reads from its in-memory map.
Allocation therefore queries occupancy through `loader` and claims slots through
`locker` (`jp_workspace/src/lib.rs`), which requires both backends to describe
the same conversation set.
`--no-persist` relaxes this invariant intentionally: `loader` remains
filesystem-backed so allocation avoids existing on-disk IDs, while `locker` uses
`NullLockBackend` because the run never writes to disk.

Because the allocation floor is `now` rather than the highest stored ID, an
imported future-dated conversation causes the allocator to skip a single slot
when wall-clock time reaches that timestamp, rather than blocking creation until
local time catches up.
After a backwards clock jump, the next process allocates from the earlier
timestamp and skips any occupied slots, preserving uniqueness even though IDs no
longer increase across processes.
Within the process that experienced the clock jump, `process_high_water` keeps
allocations above the last ID issued in that run, and the drift budget governs
how far ahead of the new clock it can advance.

### Exclusion

A directory scan is a snapshot that does not reflect directories written by
other processes after the scan completes.
If two multi-source forks run concurrently, process P can claim `T` while
process Q claims, persists, and releases `T+1`; P's second allocation would then
find `T+1` unlocked and absent from its initial snapshot, overwriting Q's
conversation.

The per-conversation lock alone cannot close this window.
Beyond the stale snapshot, `ConversationFileLock::drop` releases the `flock`
before unlinking the lock file (`jp_storage/src/lock.rs`), allowing two
processes to hold exclusive locks on different inodes for the same path.

To serialize claims, each allocation holds a single stable lock file,
`locks/allocator.lock`, from the initial scan through the slot claim:

```text
acquire locks/allocator.lock
    scan occupancy into a set
    walk to the first candidate the set does not hold and no lock covers
    claim it (per-conversation lock, as today)
    re-read occupancy for that one ID
    still free: keep it. occupied: release the claim, advance, repeat
release locks/allocator.lock
```

**Re-reading occupancy after claiming the candidate closes the window between
the scan and `try_lock`.** An earlier process releases `locks/allocator.lock` as
soon as it claims a slot, before persisting its conversation directory and
releasing the per-conversation lock.
If that earlier process persists and drops its conversation lock after the
current process's initial scan but before its `try_lock` check, the slot appears
absent in the snapshot and unlocked when probed.
Checking directory occupancy a second time after acquiring the candidate's
conversation lock reverses the observation order: while this process holds both
`locks/allocator.lock` and the candidate's conversation lock, no new process can
claim the slot, and any previous holder has already persisted its directory
before releasing the conversation lock.
The initial scan serves as a skip-list so the re-read runs once per allocation
rather than once per candidate.
The re-read also neutralizes the release-before-unlink race in
`ConversationFileLock::drop`: even if two processes lock different inodes for
the same path, the later process sees the earlier process's persisted directory
during the re-read and advances to the next slot.

**Scan ordering handles concurrent partition moves.** `archive_conversation`
holds the conversation lock across the directory rename
(`jp_workspace/src/lib.rs`), so the lock check covers archiving.
`unarchive_conversation` takes no lock (ticket `T-02ynsca`), so a rename out of
`.archive/` can occur between the two halves of a partition scan.
The union scan therefore reads `.archive/` before the active partition, matching
the order `cleanup_stale_files` uses for the same reason, so whichever read runs
after the rename sees the directory.
Because an archived ID only becomes a candidate if the system clock moves
backwards past its creation time, this ordering is a low-cost safeguard.

`locks/allocator.lock` is never unlinked, so its inode remains stable and the
release-before-unlink hazard does not apply to it.
Nothing may delete it while allocation can run, including the stale-lock
reclamation pass (see Risks).
The critical section spans two directory scans, keeping lock hold times
negligible for a single-user CLI and leaving per-conversation lock semantics
unchanged.
Occupancy checking **fails closed**: an I/O error during a scan aborts the
allocation rather than proceeding with incomplete occupancy state.

**`LockBackend` owns the allocator lock** (`jp_storage/src/backend/lock.rs`).
It gains a method returning a guard that releases the lock on drop without
unlinking the file: `FsStorageBackend` uses `flock`, `InMemoryStorageBackend`
uses a process mutex, and `NullLockBackend` grants the lock unconditionally
(matching `try_lock`, since `--no-persist` writes nothing and promises no
uniqueness).
Reusing `try_lock("allocator")` is not viable because
`ConversationFileLock::drop` (`jp_storage/src/lock.rs`) unlinks the lock path on
drop.

**Contention retries briefly before failing.** `try_lock` is non-blocking, and
`lock_new_conversation` (`jp_workspace/src/lib.rs`) turns `Ok(None)` into an
immediate error; using non-blocking acquisition for the allocator lock would
cause one of two concurrent forks to fail while the other scans directories.
Instead, acquiring the allocator lock retries for up to `ALLOCATOR_LOCK_WAIT` (1
second) before returning a typed contention error.
It neither prompts the user nor reuses `style.lock_wait`, which governs a
10-second wait and interactive prompt when another session holds a conversation
lock (`acquire_lock` in `jp_cli/src/cmd/lock.rs`).
The allocator lock is held only across directory scans, leaving nothing for the
user to decide.

The release-before-unlink defect still affects write exclusion on existing
conversations, which is a pre-existing bug out of scope here.
`create_conversation`, the unlocked creation helper, does not acquire the
allocator lock and its doc comment notes that it provides no collision
guarantee; all production callers use the locking variants (`cmd/query.rs`,
`conversation/fork.rs`).

### Drift budget

Because downstream consumers treat the ID's timestamp as a creation time, an ID
allocated 100 seconds ahead of wall-clock time would be excluded by a
`--created-before` cutoff at the current time and would expire 100 seconds late.
Drift decays during idle periods, but it requires a ceiling.

`MAX_ID_DRIFT` is a constant set to 5 seconds (50 slots).
If the first unoccupied candidate exceeds `now + MAX_ID_DRIFT`, allocation
returns a typed error.
Only two conditions push a candidate past that limit: a sustained local burst,
or a backwards clock jump inside a process that has already allocated and
retains its earlier `process_high_water` floor.
An imported future-dated ID only occupies a single slot without moving the
floor, so the error message can name both possible causes directly:

```text
Error: cannot allocate a conversation ID: allocation is 5s ahead of wall-clock
time. Either conversations are being created faster than the ID format allows
(10 per second), or the system clock moved backwards during this run. Slow down,
or open an issue describing the workload.
```

Five seconds accommodates realistic bursts ([RFD 050] supports N positional fork
sources, where a dozen is plausible) while keeping timestamp skew small.
A backwards clock jump larger than `MAX_ID_DRIFT` blocks further creations in
the process that observed it until wall-clock time catches up; a process started
afterwards has no prior high-water mark and can allocate immediately.

### Exact `created_at`

`Conversation` gains `created_at: Option<DateTime<Utc>>` with `#[serde(default,
skip_serializing_if = "Option::is_none")]`, so conversations written before this
change load unchanged and `None` falls back to `id.timestamp()`.
Unlike the other timestamp fields on `Conversation`
(`jp_conversation/src/conversation.rs`), which use a custom `serialize_with` for
`"YYYY-MM-DD HH:MM:SS.f"`, `created_at` serializes as RFC 3339; the remaining
timestamp fields will migrate to RFC 3339 over time, and `parse_dt` already
parses both formats.

**The allocating creation path stamps `created_at`.**
`create_and_lock_conversation*` captures a single timestamp, derives the
decisecond floor from it, and writes the exact timestamp to `created_at` after
claiming the ID, overwriting any value passed by the caller.
Both production callers require this overwrite: `jp query` passes
`Conversation::default()` (`create_new_conversation` in `cmd/query.rs`), and
`fork_conversation` clones the source conversation's metadata while resetting
only `last_activated_at` and `expires_at` (`conversation/fork.rs`), so a fork
would otherwise inherit its source's `created_at`.
The same timestamp seeds `ConversationStream::created_at` (which
`jp_storage/load.rs` otherwise derives from `id.timestamp()`), keeping the
metadata and stream timestamps consistent.
The test-only explicit-ID creation variants leave an existing `created_at` value
intact so that restoring or importing a conversation preserves its original
creation time.

Sorting and filtering continue to read `id.timestamp()`.
Routing `--created-since` and `--created-before` through `metadata.json` would
replace an integer comparison on the ID with a file read per conversation, and
their inaccuracy is bounded by `MAX_ID_DRIFT`.
`conversation ls -F json` is the exception for the value it emits: it already
loads every conversation's metadata to build its output rows (`ls.rs`,
`Workspace::conversations` in `jp_workspace/src/lib.rs`), so reporting the
stored `created_at` adds no I/O.

## Drawbacks

- **The ID is an approximate timestamp.** Under burst creation, a conversation
  ID can drift up to `MAX_ID_DRIFT` ahead of the actual creation time, whereas
  previously it matched the creation decisecond whenever no collision occurred.
- **IDs do not guarantee creation order across machines or clock adjustments.**
  A backwards clock jump causes subsequent processes to allocate lower IDs than
  existing conversations, and conversations imported via git can carry
  timestamps ahead of local allocations.
- **Creation is serialized by `locks/allocator.lock`** and fails if that lock
  file cannot be opened or locked.
- **Each allocation performs up to two directory scans.** A twelve-source fork
  scans both roots and both partitions on each iteration.
  The cost scales with the number of conversations, matching the directory scan
  that listing commands already run.
- **Creation fails when drift exceeds `MAX_ID_DRIFT`.** Sustained bursts or a
  backwards clock jump larger than `MAX_ID_DRIFT` within an active process
  return an error until wall-clock time catches up, whereas creation previously
  succeeded with a colliding ID.
- **`created_at` has no internal consumer that requires sub-second accuracy.**
  It exists so `conversation show` and `conversation ls -F json` can report the
  exact creation time when the ID has drifted.

## Alternatives

**A persistent cursor file.** Storing a `next_id` counter on disk and
incrementing it under a lock introduces redundant state that can desynchronize
from the conversation directories.
In `.jp/`, the file would cause git merge conflicts; in user storage, wiping
user data would reset the counter and re-enable collisions; and in either
location, the counter would not reflect conversations pulled via `git pull`.
The conversation directories on disk already record which IDs are in use.

**Sleeping to bound drift.** Sleeping until wall-clock time catches up would
hold the allocator lock across the sleep, serializing concurrent creators, and a
multi-second pause in `jp conversation new` would be indistinguishable from a
hang.
Returning an error names the cause and invites the user to report the workload.

**Changing the ID format.** Finer timestamp precision (centiseconds or
milliseconds) reduces collision probability without guaranteeing uniqueness,
leaving `fork A B` racy; a ULID or random suffix breaks the timestamp consumers
outright.
Both options invalidate existing directory names on disk and lengthen a
deliberately compact, typeable ID.

**Process-local counter only.** Tracking a high-water mark in memory without
scanning storage fixes `fork A B` within one process, but does not prevent
collisions across concurrent or sequential processes.
It is retained as `process_high_water` within the full allocator rather than
serving as the standalone design.

**Allocating above the highest stored ID.** Setting the allocation floor to the
maximum ID on disk conflicts with the drift budget.
A conversation projected from a machine whose clock is a minute fast would push
the floor a minute into the future, causing every local allocation to fail the
drift check until wall-clock time caught up.

**A dedicated allocation backend trait.** `LoadBackend` and `LockBackend`
already own reading and exclusion, so a third trait would re-declare both
operations for every storage backend without encapsulating any new behavior.

## Non-Goals

- **Cross-machine collisions via git projection.** The allocator operates on a
  single machine's storage roots and cannot coordinate with other clones.
  Detecting and re-keying duplicate IDs across machines is separate work; [RFD
  097] solves the analogous problem for event IDs by combining insertion-time
  uniqueness with load-time repair.
- **Non-destructive handling of existing duplicate IDs.** When two directories
  share an ID, `reconcile_conversation_dir` (`jp_storage/src/lib.rs`) renames
  one match to the target name and deletes the others with `remove_dir_all`.
  Making storage operations non-destructive when duplicate directories exist is
  separate work; this RFD prevents local creation from producing duplicate IDs,
  while that effort makes duplicates survivable.
- **Reconciling same-ID copies across storage roots.** Deciding whether two
  same-ID directories in different roots are replicas or distinct conversations
  would revise the last-write-wins resolution defined in [RFD 031] and is out of
  scope.
- **User-supplied IDs.** [RFD 050] rejects `--id` for pre-generated IDs, and
  this RFD keeps that decision.
- **Changing how conversation directories are named or resolved.**
- **Raising the 10-per-second sustained ceiling.** Workloads that exceed 10
  creations per second for more than `MAX_ID_DRIFT` receive an error rather than
  a format change.

## Risks and Open Questions

- **Sequential uniqueness requires creation commands to persist before releasing
  the lock.** `ConversationMut::drop` (`jp_workspace/src/conversation_lock.rs`)
  writes a directory only when the conversation was marked dirty.
  If `jp conversation new` ([RFD 050]) exited without mutating the conversation,
  it would unlink its lock file without writing a directory, leaving nothing for
  the next process to skip.
  [RFD 050] owns that command and must uphold this invariant.
- **Scan cost on large workspaces.** In a workspace with 10,000 conversations
  across two roots and two partitions, each allocation scans those directories
  inside the critical section (twelve allocations for a twelve-source fork).
  This cost should be measured on large workspaces.
- **Stale-lock reclamation must skip `locks/allocator.lock`.**
  `Workspace::cleanup_stale_files` runs at the end of every CLI invocation
  (`jp_cli/src/lib.rs`) and deletes every unheld `*.lock` file in the user
  `locks/` directory (`jp_workspace/src/session_mapping.rs`,
  `list_orphaned_lock_files` in `jp_storage/src/lib.rs`).
  Unlinking `locks/allocator.lock` between two processes opening the path would
  leave them holding `flock`s on different inodes, reintroducing the race the
  stable lock prevents.
  `list_orphaned_lock_files` must exclude `locks/allocator.lock` by name (Phase
  2).

## Implementation Plan

### Phase 1: Occupancy on `LoadBackend`

Add the occupancy operation to `LoadBackend`, implemented for `FsStorageBackend`
as a union scan across both roots reading `.archive/` before the active
partition, and for `InMemoryStorageBackend` from its in-memory map.
The operation fails closed by propagating any I/O error.

- Test: a workspace holding an archived conversation reports that ID as
  occupied.
- Test: an ID present only in the user root is reported as occupied.

### Phase 2: Allocation

Add the allocator-lock operation to `LockBackend` and its three implementations,
and exclude `locks/allocator.lock` from `list_orphaned_lock_files` so end-of-run
reclamation leaves it in place.
Add the allocator to `Workspace`: acquire the allocator lock, scan occupancy,
walk from `max(now, process_high_water + 1)` to the first unoccupied candidate,
claim it via `try_lock`, re-read occupancy for that candidate and advance if it
is now occupied, and release the allocator lock.
Route `create_and_lock_conversation*` through the allocator, and document
`create_conversation` as offering no collision guarantee.

- Test: `jp conversation fork A B` yields two distinct IDs and two surviving
  conversations.
  This reproduces the bug described in Motivation and must fail before this
  phase; it must not pass `--compact`, whose per-iteration LLM call separates
  creations across deciseconds.
- Test: `cleanup_stale_files` removes an orphaned conversation lock and leaves
  `locks/allocator.lock` in place.
- Test: an ID persisted by another process after this process's first allocation
  is skipped by its second allocation (rescan under lock).
- Test: if a candidate becomes occupied between the initial scan and the
  re-read, the allocator releases the candidate's lock and advances to the next
  slot.
- Test: an occupied slot at `now` with a directory on disk and no lock held
  allocates `now + 1`.
- Test: when a second process holds the conversation lock for `T`, allocation
  claims `T + 1`.
- Test: holding the allocator lock past `ALLOCATOR_LOCK_WAIT` causes a second
  process to fail with the typed contention error rather than blocking
  indefinitely or allocating a duplicate ID.

### Phase 3: Drift budget

Add `MAX_ID_DRIFT` and the typed drift error.

- Test: an occupancy set filling every slot from `now` to `now + MAX_ID_DRIFT`
  returns the drift error rather than allocating an ID.
- Test: a single future-dated occupied slot beyond `now + MAX_ID_DRIFT` is
  skipped rather than treated as the floor, so allocation succeeds at `now`.

### Phase 4: Exact `created_at`

Add `created_at` to `Conversation` and stamp it in the allocating creation path
from the timestamp that seeds the decisecond floor.
Prefer `Conversation::created_at` over `id.timestamp()` when seeding
`ConversationStream::created_at`, add the `Created` row and top-level
`created_at` JSON key to `conversation show`, and switch `created_at` in
`conversation ls -F json` to the stored value.

- Test: forking a conversation with a known `created_at` records the fork's own
  creation time, not the source's.
- Test: a conversation created through `jp query --new` has `created_at` set.
- Test: a conversation written without `created_at` loads and falls back to
  `id.timestamp()`.
- Test: a `metadata.json` round-trip serializes `created_at` as RFC 3339 and
  `last_activated_at` in the existing space-separated format.
- Test: `conversation show -F json` includes a top-level RFC 3339 `created_at`.
- Test: `conversation ls -F json` emits the stored `created_at` when present,
  and falls back to `id.timestamp()` when absent.

### Phase 5: Vocabulary

Add a Conversation ID entry to `docs/wiki/00dxybd-ubiquitous-language.md`
defining it as an allocated identifier that approximates creation time,
distinguished from `created_at`.

## References

- [RFD 031]: defines the storage-root pair that bounds the allocation domain.
- [RFD 050]: introduces the scripting surface that makes creation bursts
  routine, and owns the `conversation new` persistence constraint in Risks.
- [RFD 054]: declared the ID canonical for `ConversationStream.created_at`,
  which Phase 4 revises.
- [RFD 073]: defines the backend decomposition that occupancy checking extends.
- [RFD 097]: establishes insertion-time uniqueness and load-time repair for
  event IDs.

[RFD 031]: 031-durable-conversation-storage-with-workspace-projection.md
[RFD 050]: 050-scripting-ergonomics-for-conversation-management.md
[RFD 054]: 054-split-conversation-config-and-events.md
[RFD 073]: 073-layered-storage-backend-for-workspaces.md
[RFD 097]: 097-stable-event-identifiers.md
