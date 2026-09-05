# RFD 106: Collision-Free Conversation ID Allocation

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-08-05
- **Extends**: [RFD 031], [RFD 054], [RFD 073]

## Summary

Conversation IDs encode a decisecond timestamp, so two conversations created
within the same 100 ms window get the same ID and one silently destroys the
other on disk.
This RFD keeps the ID format and replaces `Utc::now()` with an allocator that
claims the first unoccupied slot at or after the current decisecond, and adds an
exact `created_at` to conversation metadata.

## Motivation

`ConversationId` is a `DateTime<Utc>` truncated to deciseconds, rendered as
`jp-c17457886043`.
`ConversationId::default()` returns `Utc::now()` truncated, and every creation
path goes through it.

`jp conversation fork A B` loops over its sources in one process (`fork_each` in
`fork.rs`), calling `create_and_lock_conversation_with_projection` per
iteration.
Both land in the same decisecond on typical local storage, where the
per-iteration persist is three small JSON writes.
Then:

1. The second creation runs `entry(id).insert_entry(OnceLock::new())`
   (`create_conversation_with_projection` in `jp_workspace/src/lib.rs`),
   replacing the first fork's in-memory state; the result is discarded with `let
   _err =`.
2. The first fork's `ConversationLock` dropped at the end of its iteration, so
   `try_lock` succeeds and no error surfaces.
3. On persist, `reconcile_conversation_dir` (`jp_storage/src/lib.rs`) renames
   the first fork's directory into the second's name and `remove_dir_all`s the
   rest.

The first fork is gone.
No error, no warning, exit status 0.

This needs no concurrency and no unusual timing, though it is not guaranteed:
`fork --compact` runs an LLM call per iteration (`fork.rs`), which separates
them by seconds.
The cross-process cases are probabilistic, and [RFD 050] makes them routine —
`conversation new` and `fork` exist to be driven from scripts, and `fork` prints
one ID per source, so a script can receive the same ID twice.

The existing test (`test_conversation_fork` in `fork_tests.rs`) forks a *single*
source against a stubbed epoch clock, so it always passes; the multi-source path
has no coverage.

## Design

### What the user sees

The ID format is unchanged: `jp-c17457886043` stays an eleven-digit decisecond
timestamp, directory names are unchanged, and existing conversations keep their
IDs.

The contract:

> Conversations auto-allocated and persisted through one allocation domain —
> one machine's pair of storage roots — receive distinct IDs.

Three behavioral changes:

- **Every created conversation gets a distinct ID.** A burst of N creations
  yields N distinct IDs in ascending order, skipping occupied slots.
- **IDs allocated within one process are strictly increasing.** Across process
  restarts, clock corrections, and conversations arriving through git they stay
  unique but do not define creation order.
  `created_at` records the exact instant of creation, undrifted, but it orders
  conversations only as far as the clocks that stamped them agree.
- **A burst that runs the allocator far ahead of wall-clock time fails with a
  clear error** rather than succeeding with a badly-dated ID.

The ID-ordered interfaces stay approximate by design.
`--sort created`, the `--created-since` / `--created-before` thresholds, and the
`newest` target all read `id.timestamp()` (`cmd/target.rs`), so a backwards
clock correction can leave `newest` pointing at the earlier conversation.
(`latest` is unaffected: it sorts on `last_activated_at`.)

`metadata.json` gains an exact `created_at`, and `jp conversation show` a
`Created` row backed by a top-level `created_at` key in its `-F json` payload
(`DetailsFmt::json` in `format/conversation.rs`).
`jp conversation ls -F json` already emits a `created_at` derived from the ID
(`payload` in `ls.rs`); it switches to the stored value, falling back to
`id.timestamp()` when the field is absent, so the two commands cannot report
different creation times for the same conversation.
Ordering and filtering stay on the ID everywhere, `--sort created` included.

### Why the ID format is worth keeping

Seven behaviors read the ID's timestamp: `--sort created` (`ls.rs`, `grep.rs`),
the `last_event_at` fallback for an empty conversation (`ls.rs`), the
`created_at` key in `conversation ls -F json` (`payload` in `ls.rs`), the
`--created-since` / `--created-before` thresholds (`CreationRange::matches` in
`time.rs`), `expires_at = id.timestamp() + ttl` (`create_new_conversation` in
`query.rs`), `newest` resolution (`target.rs`), and the seed for
`ConversationStream::created_at` (`jp_storage/load.rs`,
`jp_workspace/src/lib.rs`).
Any format change — random suffix, ULID, added precision — invalidates some or
all of them plus every directory name on disk.
Allocating within the existing encoding fixes the collision without touching the
contract.

### The allocator

The requirement is uniqueness, not global ordering: across machines IDs never
could increase monotonically, since a teammate's conversation can arrive through
git carrying any timestamp.
So the allocator takes the first slot nobody else has rather than chasing a
maximum.
It replaces `ConversationId::default()` in the creation path:

```text
candidate = max(now_decisecond, process_high_water + 1)
while candidate is occupied:
    candidate += 1
claim candidate
```

A slot is **occupied** when a conversation directory exists for it in any
partition of any storage root, or when its conversation lock is currently held.
Neither condition alone covers a slot for its whole life: between the claim and
the persist only the lock does, and once the lock is released only the directory
does.
A `ConversationLock` persists before releasing its guard, so the two overlap
instead of leaving a gap, and the implementation must preserve that ordering.
Observing both without a gap is a separate problem, which
[Exclusion](#exclusion) covers.
`process_high_water` is bumped per allocation so a twelve-source fork does not
hand out a slot it claimed but has not yet persisted.

**Occupancy comes from storage, not from a cursor file.** Allocated IDs are
already durably recorded as directory names, and `scan_conversation_ids`
(`jp_storage/load.rs`) already reads them.
The scan runs once per allocation, under the allocator lock, into a `HashSet`.
It cannot be hoisted to once per process: a set built before the first
allocation goes stale as soon as another process persists, which is the race
[Exclusion](#exclusion) covers.
Nor is a per-candidate check cheaper — directories are named `{id}-{title}`, so
testing one candidate is itself a `read_dir` (`conversation_dirs_for_id` in
`jp_storage/src/lib.rs`).

The scan covers both roots *and* both partitions.
`load_conversation_index` filters to the active directory or `.archive/`, not
both (`jp_storage/load.rs`), so an occupancy set built from the workspace index
alone would miss archived IDs and `unarchive_conversation` could land on an
occupied directory.

Occupancy is a read, so it goes on `LoadBackend`, which already owns reading
conversations ([RFD 073] made `Storage` an internal detail behind it):
`FsStorageBackend` answers with a union scan, `InMemoryStorageBackend` from its
own map.
Allocation therefore reads through `loader` and claims through `locker`
(`jp_workspace/src/lib.rs`), which is sound only when both describe the same
conversation set — a requirement worth stating rather than assuming.
`--no-persist` breaks it deliberately: the loader stays filesystem-backed while
the locker becomes `NullLockBackend`, so occupancy reflects durable state the
run will never add to.

Because the floor is `now` rather than the stored maximum, an imported
future-dated ID costs one skipped slot instead of blocking creation until local
time catches up.
After a backwards clock jump the next process reuses the low free slots: IDs
stop increasing, uniqueness holds.
Within the process that saw the jump the high-water mark still applies, so
allocation stays above the last ID it handed out and the drift budget decides
what happens next.

### Exclusion

A scan is a snapshot, blind to what another process writes afterwards.
Given two concurrent multi-source forks, P can claim `T` while Q claims,
persists, and releases `T+1`; P's second allocation then finds `T+1` unlocked
and absent from its stale snapshot, and overwrites Q.

The per-conversation lock cannot close that on its own, and not only because of
the snapshot: `ConversationFileLock::drop` releases the `flock` *before*
unlinking the pathname (`jp_storage/src/lock.rs`), so two processes can hold
exclusive locks on different inodes for the same path.

So each allocation runs under a single stable lock, `locks/allocator.lock`, held
from the scan through the claim:

```text
acquire locks/allocator.lock
    scan occupancy into a set
    walk to the first candidate the set does not hold and no lock covers
    claim it (per-conversation lock, as today)
    re-read occupancy for that one ID
    still free: keep it. occupied: release the claim, advance, repeat
release locks/allocator.lock
```

**The re-read is what makes the snapshot safe to act on.** The scan describes
the directories at one instant and the lock test describes the locks at a later
one, so a writer that persists and releases in between reads as free on both:
absent from the snapshot, and unlocked by the time the walk asks.
Taking the observations in the other order settles it.
Nobody else can claim while this process holds the allocator lock, so a
candidate whose lock this process now holds and whose directory is absent from a
fresh read was unclaimed at both instants.
The up-front scan stays as the cheap skip-list; the re-read costs one more scan
per allocation, not one per candidate.
It also takes allocation out of reach of the release-then-unlink defect: two
processes holding `flock`s on different inodes for the same path still cannot
both keep the candidate, because the loser's re-read finds the winner's
directory.

**Partition moves are the one transition the scan cannot serialize.**
`archive_conversation` holds the conversation lock across the rename
(`jp_workspace/src/lib.rs`), so the lock test covers it.
`unarchive_conversation` takes no lock at all (ticket `T-02ynsca`), so a rename
out of `.archive/` can slip between the two halves of a scan and be missed by
both.
The union therefore reads `.archive/` before the active partition — the
ordering `cleanup_stale_files` already uses against the same defect — so
whichever read follows the rename finds the directory.
An archived ID only becomes a candidate after a backwards clock correction past
its creation time, so the ordering is cheap insurance rather than a live hazard.

The lock file is never unlinked, so it is a stable inode and the
release-then-unlink hazard does not apply to it.
Nothing may prune it while allocation can run, including the reclamation pass
that exists today (see Risks).
The critical section is two directory scans, so serializing it costs nothing for
a single-user CLI and leaves per-conversation lock semantics untouched.
Occupancy checking **fails closed**: a scan that returns an I/O error fails the
allocation rather than proceeding on unknown state.

**The allocator lock belongs to `LockBackend`**, which already owns exclusion
(`jp_storage/src/backend/lock.rs`).
It gains one operation returning a guard that releases the OS lock on drop and
leaves the pathname in place: `flock` for `FsStorageBackend`, a process mutex
for `InMemoryStorageBackend`, unconditionally granted by `NullLockBackend` (as
`try_lock` already is, since `--no-persist` writes nothing and promises no
uniqueness).
It is *not* `try_lock` with a synthetic identifier — that signature accepts
`"allocator"`, but its guard unlinks the path on drop
(`ConversationFileLock::drop` in `jp_storage/src/lock.rs`), the one property
this lock must not have.

**Contention waits briefly, then fails.** `try_lock` is non-blocking and the
creation path turns `Ok(None)` into an immediate error (`lock_new_conversation`
in `jp_workspace/src/lib.rs`); inheriting that would fail one of two concurrent
forks on a lock held for the duration of a scan.
The allocator lock blocks with a bounded retry against a constant,
`ALLOCATOR_LOCK_WAIT`, set to one second, then returns a typed contention error.
It does not prompt and does not reuse `style.lock_wait`, which covers a
ten-second human-scale wait on a lock held by another *session*, ending in an
interactive choice (`acquire_lock` in `jp_cli/src/cmd/lock.rs`).
This lock is held by a scan, and there is nothing for the user to decide.

The release-before-unlink defect still affects write exclusion on *existing*
conversations; that is a pre-existing bug, out of scope here.
`create_conversation`, the unlocked variant, takes no allocator lock and its doc
comment says it carries no guarantee; every production caller uses the locking
variants (`cmd/query.rs`, `conversation/fork.rs`).

### Drift budget

The allocator can run ahead of wall-clock time, and the consumers listed above
treat the ID's timestamp as a time: an ID 100 seconds ahead is excluded by a
`--created-before` cutoff at the current time and expires 100 seconds late.
Drift decays during idle time, but it needs a ceiling.

`MAX_ID_DRIFT` is a constant, 5 seconds (50 slots).
A candidate past `now + MAX_ID_DRIFT` returns a typed error.
Two things push a candidate that far: a local burst, and a backwards clock step
inside a process that has already allocated, whose high-water floor stays where
the old clock left it.
An imported future-dated ID is neither — it occupies one slot rather than
moving the floor — so the error can name both causes it has:

```text
Error: cannot allocate a conversation ID: allocation is 5s ahead of wall-clock
time. Either conversations are being created faster than the ID format allows
(10 per second), or the system clock moved backwards during this run. Slow down,
or open an issue describing the workload.
```

Five seconds covers every realistic burst ([RFD 050] designs for N positional
fork sources; a dozen is plausible) and keeps the error on derived behavior
negligible.
A clock step larger than the budget blocks creation in the process that saw it
until wall-clock time catches up; a process started afterwards is unaffected,
having no high-water mark to carry.

### Exact `created_at`

`Conversation` gains `created_at: Option<DateTime<Utc>>`, `#[serde(default,
skip_serializing_if = "Option::is_none")]`, so conversations written before this
change load unchanged and absent means "fall back to `id.timestamp()`".
It carries no `serialize_with`, so it serializes as RFC 3339 rather than the
`"YYYY-MM-DD HH:MM:SS.f"` shape the other timestamps use (`Conversation` in
`jp_conversation/src/conversation.rs`): RFC 3339 is where those fields are
headed, and `parse_dt` already reads both.

**The allocating path stamps the field.** `create_and_lock_conversation*`
captures one instant, derives the decisecond floor from it, and writes it after
the ID is claimed, overwriting whatever the caller passed in.
Both production sites need that: `jp query` passes `Conversation::default()`
(`create_new_conversation` in `cmd/query.rs`), and `fork_conversation` clones
the source's metadata, resetting only `last_activated_at` and `expires_at`
(`conversation/fork.rs`), so a fork would otherwise report its source's creation
time.
The same instant seeds `ConversationStream::created_at` — today derived from
`id.timestamp()` (`jp_storage/load.rs`) — so the two cannot disagree.
The explicit-ID variants, used only in tests, leave an existing `created_at`
alone: restoring or importing preserves creation time, allocating does not.

Sort and filter paths keep reading `id.timestamp()`.
Routing `--created-since` / `--created-before` through metadata would turn a
free integer comparison into a metadata read per conversation, and their error
is bounded by the drift budget.
`conversation ls -F json` is the exception, and only for the value it reports:
it already loads every conversation's metadata to build its rows (`ls.rs`,
`Workspace::conversations` in `jp_workspace/src/lib.rs`), so reporting the
stored `created_at` there costs nothing.

## Drawbacks

- **The ID stops being exactly the creation time.** Anyone reading a raw ID as a
  precise timestamp is now slightly wrong, where before they were right whenever
  they were the only writer.
- **IDs are no longer ordered against imported conversations,** and a backwards
  clock jump reorders local ones.
  Cross-machine ordering was never real; this makes the loss explicit rather
  than disguised.
- **Creation is serialized by a new lock** and gains a failure surface if
  `locks/allocator.lock` becomes unwritable.
- **One directory scan per conversation created.** A twelve-source fork scans
  twelve times over two roots and two partitions.
  Bounded by the conversation count, and the same scan already runs on most
  listing paths.
- **A new failure mode:** creation can fail on drift where it previously
  succeeded with a colliding ID.
  A backwards clock step larger than `MAX_ID_DRIFT` does the same to a process
  that has already allocated, until wall-clock time catches up.
- **`created_at` has no consumer that strictly needs it today.** It exists
  because once the ID is approximate, something has to hold the exact value.

## Alternatives

**A persistent cursor file.** A `next_id` counter on disk, incremented under a
lock.
Rejected: redundant state that desyncs from what it describes.
In `.jp/` it becomes a committed merge-conflict source; in user storage, wiping
it silently re-enables collisions.
Either way it has never seen conversations that arrived via `git pull`.
The directory names are the record.

**Sleep to bound drift.** Rejected: the sleep sits under lock acquisition,
serializing concurrent creators, and a silent multi-second stall in `jp
conversation new` is indistinguishable from a hang.
An error names the cause and invites the user to describe the workload.

**Change the format.** Added precision (centiseconds, milliseconds) lowers
collision probability without establishing uniqueness, so `fork A B` stays a
coin flip; a ULID or random suffix breaks the six timestamp consumers outright.
Both invalidate every directory name on disk and lengthen a deliberately
typeable ID.

**Process-local counter only.** A high-water mark in memory, no storage read.
Fixes `fork A B` and nothing else.
Rejected as a final design, but it is a strict subset of the proposed allocator,
not a separate implementation path.

**Allocate above the highest stored ID.** Rejected: it cannot coexist with the
drift budget.
A conversation projected from a machine whose clock is a minute fast would push
the floor a minute ahead, failing every local creation until wall-clock time
caught up — an outage from ordinary clock skew, misdiagnosed as a creation-rate
problem.

**A dedicated allocation backend trait.** Rejected: `LoadBackend` and
`LockBackend` already own reading and exclusion, and a third trait would
re-declare both for every backend that implements them.
It adds a name, not a decision.
(Implementation count is not the argument — it would have as many
implementations as the two it wraps.)

## Non-Goals

- **Cross-machine collisions via git projection.** The allocator is
  per-storage-root and cannot see the other side.
  Detecting and re-keying duplicate IDs is separate work; [RFD 097] solves the
  same shape one level down for event IDs, and its repair half is the eventual
  answer here.
- **What the storage layer does with a duplicate ID once one exists.** Under
  collision, `reconcile_conversation_dir` renames one match into the target and
  `remove_dir_all`s the rest (`jp_storage/src/lib.rs`), destroying a live
  conversation.
  That is one of several operations that delete conversation data as a side
  effect; making all of them non-destructive is separate work.
  Neither effort gates the other: this RFD lowers the rate at which same-ID
  directories appear, the other makes them survivable.
- **Reconciling copies that live one per root.** Deciding whether two same-ID
  directories in different roots are replicas or distinct conversations revises
  the last-write-wins resolution [RFD 031] defines.
  Out of scope here, and not covered by the displacement work either.
- **User-supplied IDs.** [RFD 050] rejects `--id` for pre-generated IDs; that
  stays rejected.
- **Changing how conversation directories are named or resolved.**
- **Raising the 10-per-second ceiling.** A workload that needs more gets an
  error and a conversation, not a format change.

## Risks and Open Questions

- **Creation must persist for the sequential case to hold.** A process leaves a
  directory behind only when the conversation was made dirty;
  `ConversationMut::drop` returns early otherwise
  (`jp_workspace/src/conversation_lock.rs`).
  A `jp conversation new` that never mutates would write nothing, and once its
  lock file is unlinked the next process has nothing to skip.
  [RFD 050] owns that command; this is a constraint on it, not something this
  RFD can verify.
- **Scan cost on very large workspaces.** Ten thousand conversations across two
  roots and two partitions is a scan per allocation, inside the critical
  section, twelve times for a twelve-source fork.
  Worth measuring before assuming it is free.
- **The reclamation that exists today would delete the allocator lock.**
  `Workspace::cleanup_stale_files` runs at the end of every invocation
  (`jp_cli/src/lib.rs`) and removes every unheld `*.lock` in the user `locks/`
  directory (`jp_workspace/src/session_mapping.rs`, `list_orphaned_lock_files`
  in `jp_storage/src/lib.rs`).
  An unlink landing between one process opening the path and another opening it
  leaves the two holding `flock`s on different inodes — the hazard the stable
  lock exists to avoid.
  Reclamation must skip this file by name; Phase 2 owns the change and the test.

## Implementation Plan

### Phase 1: Occupancy on `LoadBackend`

Add the occupancy operation to `LoadBackend`, implemented for `FsStorageBackend`
as a union scan across both roots reading `.archive/` before the active
partition, and for `InMemoryStorageBackend` from its own map.
Fails closed on I/O error.

- Test: a workspace holding an archived conversation reports that ID as
  occupied.
- Test: an ID present only in the user root is occupied.

### Phase 2: Allocation

Add the allocator-lock operation to `LockBackend` and its three implementations,
and exclude `locks/allocator.lock` from `list_orphaned_lock_files` so the
end-of-run reclamation leaves it alone.
Then add the allocator to `Workspace`: acquire the allocator lock, scan
occupancy, walk from `max(now, process_high_water + 1)` to the first unoccupied
candidate, claim it via the existing `try_lock`, re-read occupancy for that
candidate and advance if it is no longer free, release.
Route `create_and_lock_conversation*` through it.
Document `create_conversation` as offering no collision guarantee.

- Test: `jp conversation fork A B` yields two distinct IDs and two surviving
  conversations.
  This reproduces the Motivation bug and must fail before this phase; it must
  not use `--compact`, whose per-iteration LLM call separates the deciseconds
  and passes vacuously.
- Test: `cleanup_stale_files` removes an orphaned conversation lock and leaves
  `locks/allocator.lock` in place.
- Test: an ID persisted by another process after this process's first allocation
  is skipped by its second (the rescan-under-lock case).
- Test: an occupancy answer that changes between the scan and the re-read makes
  the allocation release that candidate and advance, rather than keeping a slot
  another writer has published.
- Test: an occupied slot at `now`, directory present and no lock held, allocates
  `now + 1` (the case a lock-only claim misses).
- Test: a second process holding the lock for `T` allocates `T + 1`.
- Test: a held allocator lock past `ALLOCATOR_LOCK_WAIT` fails the second
  process with the typed contention error rather than blocking or duplicating.

### Phase 3: Drift budget

Add `MAX_ID_DRIFT` and the typed error.

- Test: an occupancy set filling every slot from `now` to `now + MAX_ID_DRIFT`
  produces the error rather than an ID.
- Test: a single future-dated occupied slot far beyond the budget is skipped,
  not treated as a floor — allocation succeeds at `now`.

### Phase 4: Exact `created_at`

Add the field, stamp it in the allocating creation path from the instant that
seeds the decisecond floor, prefer it over `id.timestamp()` when seeding
`ConversationStream::created_at`, add the `Created` row and the top-level
`created_at` key to `conversation show`, and switch the `created_at` key in
`conversation ls -F json` to the stored value.

- Test: a fork of a conversation with a known `created_at` records its own
  creation time, not the source's.
- Test: a conversation created through `jp query --new` has `created_at` set.
- Test: a conversation written without the field loads and behaves as before.
- Test: a `metadata.json` round-trip emits RFC 3339 for `created_at` and the
  existing space-separated format for `last_activated_at`.
- Test: `conversation show -F json` carries a top-level RFC 3339 `created_at`.
- Test: `conversation ls -F json` reports the stored `created_at` when the
  conversation has one, and the ID's timestamp when it does not.

### Phase 5: Vocabulary

Add a Conversation ID entry to `docs/architecture/ubiquitous-language.md`: an
allocated identifier that approximates creation time, distinguished from
`created_at`.

## References

- [RFD 031] — the storage-root pair that bounds the allocation domain.
- [RFD 050] — the scripting surface that makes bursts routine, and the owner of
  the `conversation new` persistence constraint in Risks.
- [RFD 054] — declared the ID canonical for `ConversationStream.created_at`;
  Phase 4 revises that.
- [RFD 073] — the backend decomposition that occupancy checking extends.
- [RFD 097] — insertion-time uniqueness plus load-time repair for event IDs.

[RFD 031]: 031-durable-conversation-storage-with-workspace-projection.md
[RFD 050]: 050-scripting-ergonomics-for-conversation-management.md
[RFD 054]: 054-split-conversation-config-and-events.md
[RFD 073]: 073-layered-storage-backend-for-workspaces.md
[RFD 097]: 097-stable-event-identifiers.md
