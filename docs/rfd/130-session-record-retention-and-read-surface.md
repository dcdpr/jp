# RFD 130: Session Record Retention and Read Surface

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-08-17
- **Extends**: [RFD 020], [RFD 087]
- **Summary**: Keeps closed session records for a retention window, records each
  activation's checkout, and adds `jp session` to read them.

## Summary

JP deletes session records as soon as their originating process exits, omits the
checkout where each conversation was activated, and provides no way to read a
record without parsing an internal on-disk format.
This RFD records the checkout root of each activation, retains closed session
records for a 30-day window, and adds a `jp session` read surface.

## Motivation

`Workspace::cleanup_stale_files` runs at the end of every invocation and unlinks
any session mapping whose session-leader PID is confirmed dead.
Eager deletion causes three problems, and an omission in the session record
causes a fourth:

**Session records are deleted before restore tools can read them.** A terminal
emulator that restores its window layout restores surfaces, not processes: every
restored tab gets a fresh shell with a new PID, so every existing
`getsid-<pid>.json` names a dead process.
Any tool that rebuilds tab-to-conversation bindings must run before the next
`jp` invocation in that workspace.
Because any tab can trigger the cleanup sweep, the restore tool races against
normal use.

**Reading a record requires depending on a private encoding.** Outside the
workspace crates, which can call `Workspace::session_overviews()`, there is no
supported command to query a session's active conversation, so external tooling
parses the JSON files directly.
That encoding is not obvious: `ConversationId` serializes through `jp_id::serde`
as a bare decisecond integer, whereas [RFD 020]'s illustrative session-mapping
payload shows `"id": "jp-c17528832001"`.
A script written against the documented format does not match the on-disk format
and silently selects nothing.

**A recycled PID can adopt a closed session's history.** `read_matching_mapping`
guards against filename aliasing by comparing the stored `id` and `source`
against the current session, but for `SessionSource::Getsid` the stored `id` is
the PID.
A new shell assigned a recycled PID matches the old record.
Eager deletion makes this unlikely rather than impossible, because nothing
guarantees that the cleanup sweep runs before the new shell's first `jp` call.

**Records omit the checkout where a conversation was activated.** Every checkout
of a workspace shares one session directory, and a shell can `cd` between
checkouts, so a session's history names its conversations without the checkout
each one ran in.
A tool that displays each checkout's recent activity, such as a workspace
dashboard, can only associate a conversation with a checkout through the working
directory of the session's live shell.
Once that shell exits, the checkout association is lost and eager deletion
removes the record.
The roots registry ([RFD 087]) tracks checkouts without their conversations; the
session record is the only place that observes both at activation time.

## Design

### Activation Root

`SessionHistoryEntry` records the checkout root where the activation occurred:

```rust
/// The canonical root of the checkout this conversation was activated in.
///
/// `None` for entries written before this field existed.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub root: Option<Utf8PathBuf>,
```

`Workspace::record_session_activation` populates `root` from the workspace root
directory (the directory containing the workspace's storage, matching the roots
registry), regardless of how the checkout was created or which subdirectory the
process ran from.
Every activation path records `root` without caller changes: `jp query`, `jp
conversation use` (including its lock-contention fallback), and `jp conversation
fork`.
Re-activating a conversation in another checkout moves its entry to the front of
the history and updates both `activated_at` and `root`.
Because the session directory is user-local and never projected into the
workspace, storing an absolute path is safe.

### Closed Sessions

A session is open while its originating process is running and closed once that
process exits, whether cleanly, by crashing, or across a reboot.
`SessionMapping` gains one field:

```rust
/// When JP first observed that the session's originating process was gone.
///
/// `None` while the session is open, while liveness is unknown, and before
/// any cleanup pass has observed it close.
pub closed_at: Option<DateTime<Utc>>,
```

When `cleanup_session_file` first observes `Liveness::Dead`, it sets `closed_at`
to the current time and renames the record from `<key>.json` to
`<key>~<closed_at as Unix seconds>.json` instead of deleting it.
Renaming frees `<key>.json` so a new session assigned the same PID starts a
fresh record without overwriting the closed one, which retains its own key for
the retention window.

`closed_at` records when JP observed that the session closed, not when the
process exited.
It is never derived from `activated_at`, because an open session can sit idle
for months and expiring records from last activation would collect sessions that
are still in use.

For `SessionSource::Env`, process liveness is unknown, so the existing
conversation-existence heuristic remains the trigger: the record closes when no
conversation in its history still exists.
Because an empty record has no history left to retain, cleanup removes it
immediately, preserving existing behavior for `Env` sessions.

Records written before this field existed deserialize with `closed_at: None` and
are treated as open until observed closed.

### Matching Open Sessions

An open session reads only `<key>.json`, ignoring renamed closed records.
`read_matching_mapping` also rejects any record with `closed_at` set, covering
the window between writing `closed_at` and renaming the file.

This prevents PID reuse from adopting any session that JP has observed closed.
It does not prevent PID reuse if a shell exits and the OS recycles its PID
before a cleanup pass runs (for example, across a reboot), because the unmarked
record still matches the new shell (see [Risks and Open
Questions](#risks-and-open-questions)).

### Retention and Garbage Collection

Cleanup removes a closed record once `now - closed_at` exceeds the retention
window.
This RFD fixes the window at a 30-day constant rather than a configuration key,
leaving configuration until a concrete need arises.

History pruning applies to closed records the same way it applies to open ones:
cleanup drops any entry whose conversation is neither live, archived, nor
locked, and removes any closed record whose history becomes empty.
Retention preserves a session's surviving conversations, not an audit log of
deleted ones.

Closed records must not keep ephemeral conversations alive.
`all_active_conversation_ids`, which feeds `remove_ephemeral_conversations`,
skips closed records so a `--tmp` conversation whose terminal has closed is
still collected on schedule.

### Read Surface

```console
$ jp session ls
KEY                  SOURCE          STATE    ACTIVE            LAST ACTIVE
getsid-79800         getsid          open     jp-c17864439611   2 minutes ago
getsid-12057         getsid          open     jp-c17862201040   4 hours ago
env-JP_SESSION-a1b2  env:JP_SESSION  unknown  jp-c17858834912   yesterday

$ jp session ls --all
KEY                      SOURCE          STATE    ACTIVE            LAST ACTIVE
getsid-79800             getsid          open     jp-c17864439611   2 minutes ago
getsid-12057             getsid          open     jp-c17862201040   4 hours ago
env-JP_SESSION-a1b2      env:JP_SESSION  unknown  jp-c17858834912   yesterday
getsid-61004~1762333920  getsid          closed   jp-c17851190372   3 days ago

$ jp session show getsid-79800
$ jp -F json session ls
```

A record's state is computed at read time using the same process liveness check
as cleanup: `open`, `closed`, or `unknown` when liveness cannot be checked (such
as `Env` sessions).
Checking `closed_at` alone is not sufficient because cleanup runs only at the
end of a writable `jp` invocation; a reader such as `session_overviews()`
observes a recently exited shell as `closed` with `closed_at: null` until the
next cleanup pass.

`jp session ls` and `jp session show` run session-record maintenance before
reading rather than waiting until the end of the invocation.
Marking and renaming closed records up front ensures that every key printed by
`ls` stays stable for the retention window; if a session closes after `ls` runs,
`show` on its old key reports it missing and `ls --all` lists it under its
renamed key.
Running maintenance early adds no writes beyond what a normal `jp` invocation
performs at exit, and read-only invocations that cannot write session state skip
maintenance and list the records as they stand on disk.

`ls` lists `open` and `unknown` records, while `--all` includes closed records
under their renamed keys.
`show` prints a single record and its history of live and archived
conversations, including each entry's `activated_at` and `root`.

`ACTIVE` is the session's active conversation, resolved using the same rules the
session itself uses: deleted conversations are skipped, and if the most recent
remaining entry is archived, the session has no active conversation.
For a closed record, `ACTIVE` is the conversation the session would have
continued.

The JSON output is the supported contract for external tooling and formats
conversation IDs in their display form (`jp-c17864439611`) rather than as the
on-disk integer:

```json
{
  "key": "getsid-79800",
  "source": "getsid",
  "state": "open",
  "closed_at": null,
  "active": "jp-c17864439611",
  "history": [
    {
      "id": "jp-c17864439611",
      "activated_at": "2026-11-05T09:12:00Z",
      "root": "/Users/jean/Projects/jp.git/ticket-0yh5hqj"
    }
  ]
}
```

External scripts querying a session's active conversation should read `active`
rather than `history[0]`.

In-process callers use `Workspace::session_overviews()`, which returns each
record's state, `closed_at`, active conversation, and history entries (including
`activated_at` and `root`) for both open and closed records, without shelling
out to the CLI or parsing session files directly.

The record type remains in `jp_workspace`, and the command lives in `jp_cli`.

## Drawbacks

The session directory grows with terminal tab churn over the 30-day retention
window rather than staying bounded by the number of open sessions.

`jp session ls --format=json` becomes a public contract that JP must keep
stable, replacing an implicit dependency on the file layout with an explicit CLI
schema.

Persisted session types gain two fields (`root` and `closed_at`) and an
additional lifecycle state: a record on disk can be either open or closed, and
closed records sit alongside open ones in the session directory under `~` keys.

Session records store absolute paths to the user's checkouts and `jp session`
prints them in JSON output.
Because session records already live in user-local storage alongside
conversations that reference those paths in tool calls, this does not expose new
data on disk.

## Alternatives

**Read session files from a command plugin.** Although the plugin API exposes
`paths.user_workspace`, reading session files directly from a plugin would make
the on-disk record format a public contract.
Changing that format (as this RFD does) would silently break plugin filters
rather than failing with a parse error.

**Keep eager deletion and have external tools snapshot before quitting.** This
requires users or hooks to snapshot state before exiting and loses all session
bindings on crashes or forced restarts, when recovery matters most.

**Never collect closed records.** Without garbage collection, the session
directory grows without bound and closed keys accumulate in every listing.

**Record the last conversation on the checkout's roots registry entry.** Adding
an optional conversation field to `RootEntry` (updated by `jp query`) would
duplicate activation state across two unlinked stores: the roots registry would
know each checkout's last conversation without its session, and the session
record would know its conversations without their checkouts.
Neither store would record which session ran which conversation in which
checkout.
Keeping activation history in the session record leaves the roots registry
responsible only for tracking workspace checkouts.

**Record the checkout directory on the conversation** (in conversation metadata
or on `TurnStart`).
Conversations can be projected into the workspace and committed to version
control, where local absolute paths do not belong.
Storing the checkout on the conversation would also require scanning every
conversation to find which conversation last ran in a given checkout.

## Non-Goals

- **Automatic session restoration.** Matching a restored terminal surface to a
  closed session record requires window and tab metadata that only the client
  has.
  This RFD makes an external restore script possible rather than restoring
  bindings inside JP.
- **Cross-client session semantics.** Defining what a session means outside the
  CLI is out of scope; the read surface and retention policy hold under any
  later definition.
- **Merging the per-workspace and user-global session stores.** Both the
  per-workspace session mapping ([RFD 020]) and the user-global workspace
  session store ([RFD 087]) keep a most-recent-first history per session;
  whether to unify them is left to a future cross-client session RFD.
- **Recording sessionless invocations.** An invocation for which JP cannot
  derive a session identity activates no conversation and writes no session
  record.
- **Cross-session targeting.** This RFD does not add a session-scoping flag for
  addressing another session's records.
- **Configurable retention.** The 30-day retention window is not configurable.

## Risks and Open Questions

**Retention duration.** The 30-day window is an estimate.
Tab churn varies across users, though because session records are small, a
longer window only costs minor disk space.

**Unobserved session exit and PID reuse.** A session that exits without being
observed by a cleanup pass can still be adopted via PID reuse if the OS recycles
its PID before the next `jp` invocation (for example, across a reboot), causing
the new shell to inherit the old session's conversation.
This limitation already exists and is not widened by this RFD.
Fixing it requires a per-process-lifetime identity (such as boot ID plus process
start time), which belongs in future cross-client session identity work.

**Command naming.** `jp session` uses the current domain term.
If a future cross-client session RFD settles on a different term, the command
can be renamed without affecting the underlying storage or retention model.

**Alignment with [RFD 087].** [RFD 087]'s user-global session store has its own
cleanup pass that deletes records when their session source is dead.
Both stores should share the same retention policy so they do not diverge; Phase
4 applies this RFD's policy to the user-global store.

**Read amplification across retained records.** A reader querying every
checkout's last activation, such as a dashboard polling every few seconds, must
read every session file retained within the 30-day window (one small file per
terminal tab opened in that period).
If directory scans become a bottleneck, `session_overviews()` can maintain an
index internally without changing the on-disk record format or CLI output.

## Implementation Plan

**Phase 1: Activation root.** Add `root` to `SessionHistoryEntry` and populate
it in `record_session_activation`.
Update `session_overviews()` to report every retained history entry (both live
and archived) with its `activated_at` and `root`, rather than only live
conversation IDs.
This phase is contained in `jp_workspace`, independent of the other phases, and
can merge on its own.

**Phase 2: Record and retention policy.** Add `closed_at`, mark and rename
closed records instead of unlinking them, reject closed records in
`read_matching_mapping`, prune their history like open records and remove empty
ones, remove expired records after the 30-day retention window, exclude closed
records from `all_active_conversation_ids`, and include closed records in
`session_overviews()` with each record's state computed at read time.
Add characterization tests for the existing cleanup paths first, because
`session_mapping_tests.rs` asserts the delete-on-dead behavior that this change
inverts.
This phase is contained in `jp_workspace` and can merge on its own.

**Phase 3: Read surface.** Implement `jp session ls`, `jp session ls --all`, and
`jp session show` in `jp_cli` with table and JSON output (including `state`,
`closed_at`, and `active`), running session maintenance before reading.
This phase depends on Phases 1 and 2 for the `root` and `closed_at` fields and
can merge on its own.

**Phase 4: User-global session store.** Apply the same marking, renaming, and
retention policy to [RFD 087]'s user-global session store.
This phase depends on Phase 2.

## References

- [RFD 020]: Per-workspace session mapping amended by this RFD.
- [RFD 087]: User-global session store that shares this retention policy, and
  the roots registry that tracks workspace checkouts.

[RFD 020]: 020-parallel-conversations.md
[RFD 087]: 087-session-scoped-active-workspace.md
