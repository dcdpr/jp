# RFD D61: Session Record Retention and Read Surface

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-08-17

## Summary

Session records are deleted the moment their originating process dies, and the
only way to read one is to parse an internal on-disk format.
This RFD marks a dead session record instead of unlinking it, keeps it for a
retention window, and adds a `jp session` read surface.

## Motivation

`Workspace::cleanup_stale_files` runs at the end of every invocation and unlinks
any session mapping whose session-leader PID is confirmed dead.
Three problems follow.

**The data is gone before anything can use it.** A terminal emulator that
restores its window layout restores surfaces, not processes: every restored tab
gets a fresh shell with a new PID, so every existing `getsid-<pid>.json` names a
dead process.
Any tool that wants to rebuild the tab-to-conversation bindings has to run
before the next `jp` invocation in that workspace, which is a race it cannot
reliably win — any tab can trigger the sweep.

**Reading a record means depending on a private encoding.** There is no
supported way to ask which conversation a session has active, so external
tooling parses the JSON directly.
That encoding is not obvious: `ConversationId` serializes through `jp_id::serde`
as a bare decisecond integer, while [RFD 020]'s illustrative session-mapping
payload shows `"id": "jp-c17528832001"`.
A script written against the documented shape does not match the written shape,
and fails by silently selecting nothing.

**A recycled PID can adopt a dead session's history.** `read_matching_mapping`
guards against filename aliasing by comparing the stored `id` and `source`
against the live session, but for `SessionSource::Getsid` the stored `id` *is*
the PID.
A new shell that lands on a recycled PID matches an old record exactly.
Eager deletion makes this unlikely rather than impossible, and nothing orders
the sweep ahead of the next shell's first `jp` call.

## Design

### Dead marking

`SessionMapping` gains one field:

```rust
/// When the session's originating process was first observed dead.
///
/// `None` while the session is live, or while liveness is unknown.
pub dead_at: Option<DateTime<Utc>>,
```

`cleanup_session_file` sets `dead_at` on the first observation of
`Liveness::Dead` and writes the record back, instead of removing the file.
The timestamp is recorded at observation time, never derived from
`activated_at`: a live session can sit idle for months, and deriving the
deadline from last activation would collect sessions that are still in use.

For `SessionSource::Env`, liveness is unknown, so the existing
conversation-existence heuristic remains the trigger — when no conversation in
the history survives, the record is marked dead rather than unlinked.

Records written before this field exists deserialize with `dead_at: None`, which
is the correct reading: not known to be dead.

### Matching

A record with `dead_at` set never matches a live session.
`read_matching_mapping` rejects it, so the PID-reuse adoption path is closed by
construction rather than by winning a race.

### Collection

A marked record is removed once `now - dead_at` exceeds the retention window.
The window is a constant in this RFD — 30 days — not a config key.
Retention becomes configurable when someone asks for a different value.

A dead record must not keep an ephemeral conversation alive.
`all_active_conversation_ids` feeds `remove_ephemeral_conversations`, and it
skips records marked dead, so a `--tmp` conversation whose terminal has closed
is still collected on schedule.

### Read surface

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

`ls` lists live records; `--dead` includes marked ones and shows when each died.
`show` prints one record with its full history and per-entry `activated_at`.
The JSON form is the supported contract for external tooling, and it reports
conversation ids in their display form (`jp-c17864439611`), not the on-disk
integer.

The record type stays in `jp_workspace`; the command lives in `jp_cli`.

## Drawbacks

The session directory grows, bounded by tab churn over the retention window
rather than by anything the user controls directly.

`jp session ls --format=json` becomes a public contract.
That is the point of the RFD, but it is still a surface we have to keep stable,
and it replaces an implicit dependency on the file layout with an explicit
dependency on a command.

One more field on a persisted type, and one more state a reader has to
understand: a record can now be present but dead.

## Alternatives

**A command plugin reading the session files.** Rejected: the plugin API already
exposes `paths.user_workspace`, so this is writable today with no core change,
but it would make the on-disk record shape a public contract — and this RFD
changes that shape in its first paragraph.
The break would be silent, a filter that stops matching rather than a parse
error.

**Keep eager deletion; have external tools snapshot before quitting.** Requires
the user to remember, and loses everything on a crash or a forced restart, which
is when the data matters most.

**Never collect.** Unbounded growth, and stale keys accumulate in every listing.

## Non-Goals

Restoring session bindings automatically.
This RFD makes an external restore script *possible*; deciding which restored
surface corresponds to which dead record needs facts only the client has, and is
not JP's job.

Defining what a session means outside the CLI.
The read surface and the retention policy hold under any later definition.

A session-scoping flag for addressing another session's records.

Making retention configurable.

## Risks and Open Questions

Is 30 days right?
It is a guess.
Tab churn varies by orders of magnitude between users, and the only cost of
being wrong high is disk.

[RFD 087] adds a second, user-global session store with its own cleanup pass
that drops records when the source is dead.
The two stores should share this policy or they will diverge; sequencing that is
an open question, since 087 is Accepted but not yet implemented.

<!-- Decide before promoting: does `jp session` as a noun pre-commit us on the
term? If the cross-client naming question lands on a different word, this
command surface is the thing that has to be renamed. -->

## Implementation Plan

**Phase 1 — record and policy.** Add `dead_at`, mark instead of unlink, reject
dead records in matching, collect after the window, and exclude dead records
from `all_active_conversation_ids`.
Contained in `jp_workspace`; mergeable alone.
Characterization tests for the existing cleanup paths go in first, since
`session_mapping_tests.rs` covers the current delete-on-dead behavior and those
assertions invert.

**Phase 2 — read surface.** `jp session ls` and `jp session show`, table and
JSON.
Depends on phase 1 for the `dead_at` column; mergeable alone.

**Phase 3 — apply to the user-global store.** Same marking and retention for
[RFD 087]'s session store.
Depends on 087 being implemented.

## References

- [RFD 020] — the per-workspace session mapping this RFD amends.
- [RFD 087] — the user-global session store that needs the same policy.

[RFD 020]: ../020-parallel-conversations.md
[RFD 087]: ../087-session-scoped-active-workspace.md
