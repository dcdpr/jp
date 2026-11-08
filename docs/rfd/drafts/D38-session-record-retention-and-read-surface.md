# RFD D38: Session Record Retention and Read Surface

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-08-17
- **Summary**: Marks dead session records with `dead_at`, retains them for a
  configurable window, and adds a `jp session` read command.

## Summary

JP deletes session records as soon as their originating process dies, and
reading a record requires parsing an internal on-disk format.
This RFD marks dead session records instead of unlinking them, retains them for
a 30-day window, and adds a `jp session` read surface.

## Motivation

`Workspace::cleanup_stale_files` runs at the end of every invocation and unlinks
any session mapping whose session-leader PID is confirmed dead.
This causes three problems:

**Session records are deleted before restore tools can read them.** A terminal
emulator that restores its window layout restores surfaces, not processes: every
restored tab gets a fresh shell with a new PID, so every existing
`getsid-<pid>.json` names a dead process.
Any tool that rebuilds tab-to-conversation bindings must run before the next
`jp` invocation in that workspace.
Because any tab can trigger the cleanup sweep, the restore tool races against
normal use.

**Reading a record requires depending on a private encoding.** There is no
supported command to query a session's active conversation, so external tooling
parses the JSON files directly.
That encoding is not obvious: `ConversationId` serializes through `jp_id::serde`
as a bare decisecond integer, whereas [RFD 020]'s illustrative session-mapping
payload shows `"id": "jp-c17528832001"`.
A script written against the documented format does not match the on-disk format
and silently selects nothing.

**A recycled PID can adopt a dead session's history.** `read_matching_mapping`
guards against filename aliasing by comparing the stored `id` and `source`
against the live session, but for `SessionSource::Getsid` the stored `id` is the
PID.
A new shell assigned a recycled PID matches the old record.
Eager deletion makes this unlikely rather than impossible, because nothing
guarantees that the cleanup sweep runs before the new shell's first `jp` call.

## Design

### Marking Dead Records

`SessionMapping` gains one field:

```rust
/// When the session's originating process was first observed dead.
///
/// `None` while the session is live, or while liveness is unknown.
pub dead_at: Option<DateTime<Utc>>,
```

When `cleanup_session_file` first observes `Liveness::Dead`, it sets `dead_at`
to the current time and writes the record back instead of removing the file.
The timestamp is recorded at observation time rather than derived from
`activated_at`, because a live session can sit idle for months and expiring
records from last activation would collect sessions that are still in use.

For `SessionSource::Env`, process liveness is unknown, so the existing
conversation-existence heuristic remains the trigger: when no conversation in
the history still exists, the record is marked dead rather than unlinked.

Records written before this field existed deserialize with `dead_at: None` and
are treated as live until observed dead.

### Matching Live Sessions

`read_matching_mapping` rejects any record with `dead_at` set, so a live session
never matches a dead record even if the OS recycles its PID.

### Retention and Garbage Collection

Cleanup removes a dead record once `now - dead_at` exceeds the retention window.
This RFD fixes the window at a 30-day constant rather than a config key, leaving
configuration until a concrete need arises.

Dead records must not keep ephemeral conversations alive.
`all_active_conversation_ids`, which feeds `remove_ephemeral_conversations`,
skips records marked dead so a `--tmp` conversation whose terminal has closed is
still collected on schedule.

### Read Surface

```console
$ jp session ls
KEY                  SOURCE          ACTIVE            LAST ACTIVE
getsid-79800         getsid          jp-c17864439611   2 minutes ago
getsid-12057         getsid          jp-c17862201040   4 hours ago
env-JP_SESSION-a1b2  env:JP_SESSION  jp-c17858834912   yesterday

$ jp session show getsid-79800
$ jp session ls --dead
$ jp -F json session ls
```

`ls` lists live records, while `--dead` includes dead records and shows when
each died.
`show` prints a single record with its full history and per-entry `activated_at`
timestamps.
The JSON output is the supported contract for external tooling and reports
conversation IDs in their display form (`jp-c17864439611`) rather than as the
on-disk integer.

The record type remains in `jp_workspace`, and the command lives in `jp_cli`.

## Drawbacks

The session directory grows with terminal tab churn over the 30-day retention
window rather than staying bounded by the number of open sessions.

`jp session ls --format=json` becomes a public contract that JP must keep
stable, replacing an implicit dependency on the file layout with an explicit CLI
schema.

`SessionMapping` gains a persisted field and an additional lifecycle state: a
record on disk can be either live or dead.

## Alternatives

**Read session files from a command plugin.** The plugin API exposes
`paths.user_workspace`, so a plugin could read session files without core
changes.
However, that would make the on-disk record format a public contract.
Schema changes such as adding `dead_at` would silently break plugin filters
rather than producing a parse error.

**Keep eager deletion and have external tools snapshot before quitting.** This
requires users or hooks to snapshot state before exiting and loses all session
bindings on crashes or forced restarts, when recovery matters most.

**Never collect dead records.** Without garbage collection, the session
directory grows without bound and dead keys accumulate in every listing.

## Non-Goals

- **Automatic session restoration.** Matching a restored terminal surface to a
  dead session record requires window and tab metadata that only the client has.
  This RFD makes an external restore script possible rather than restoring
  bindings inside JP.
- **Cross-client session semantics.** Defining what a session means outside the
  CLI is out of scope; the read surface and retention policy hold under any
  later definition.
- **Cross-session targeting.** This RFD does not add a session-scoping flag for
  addressing another session's records.
- **Configurable retention.** The 30-day retention window is not configurable.

## Risks and Open Questions

**Retention duration.** The 30-day window is an estimate.
Tab churn varies by orders of magnitude across users, though because session
records are small, erring on the side of a longer window only costs minor disk
space.

**Alignment with [RFD 087].** [RFD 087] adds a user-global session store with
its own cleanup pass that deletes records when the source is dead.
Both stores should share this retention policy so they do not diverge.
Sequencing the two changes remains an open question because [RFD 087] is
Accepted but not yet implemented.

<!-- Decide before promoting: does `jp session` as a noun pre-commit us on the
term? If the cross-client naming question lands on a different word, this
command surface is the thing that has to be renamed. -->

## Implementation Plan

**Phase 1: Record and retention policy.** Add `dead_at`, mark records instead of
unlinking them, reject dead records in `read_matching_mapping`, remove expired
records after the retention window, and exclude dead records from
`all_active_conversation_ids`.
Add characterization tests for the existing cleanup paths first, because
`session_mapping_tests.rs` asserts the delete-on-dead behavior that this change
inverts.
This phase is contained in `jp_workspace` and can merge on its own.

**Phase 2: Read surface.** Implement `jp session ls` and `jp session show` with
table and JSON output.
This phase depends on Phase 1 for the `dead_at` column and can merge on its own.

**Phase 3: User-global session store.** Apply the same `dead_at` marking and
retention policy to [RFD 087]'s session store once [RFD 087] is implemented.

## References

- [RFD 020]: Per-workspace session mapping amended by this RFD.
- [RFD 087]: User-global session store that needs the same retention policy.

[RFD 020]: ../020-parallel-conversations.md
[RFD 087]: ../087-session-scoped-active-workspace.md
