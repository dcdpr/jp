# Session Record Retention and Read Surface

- **Status**: Todo
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-11-05
- **Label**: tracking-rfd=130
- **Label**: type=tracking

Tracking ticket for [RFD 130].

## Implementation plan

- [ ] **Record the activation root**: Add `root` to `SessionHistoryEntry` and
      populate it in `record_session_activation`.
      Update `session_overviews()` to report every retained history entry (both
      live and archived) with its `activated_at` and `root`.
      This phase is contained in `jp_workspace` and can merge on its own.
- [ ] **Keep closed session records**: Add `closed_at`, mark and rename closed
      records instead of unlinking them, and reject closed records in
      `read_matching_mapping`.
      Prune their history like open records, remove empty ones, remove expired
      records after the 30-day retention window, and exclude closed records from
      `all_active_conversation_ids`.
      Include closed records in `session_overviews()` with each record's state
      computed at read time.
      Add characterization tests for the existing cleanup paths first.
      This phase is contained in `jp_workspace` and can merge on its own.
- [ ] **Add the `jp session` read surface**: Implement `jp session ls`, `jp
      session ls --all`, and `jp session show` in `jp_cli` with table and JSON
      output (including `state`, `closed_at`, and `active`), running session
      maintenance before reading.
      This phase depends on phases 1 and 2 and can merge on its own.
- [ ] **Apply retention to the user-global store**: Apply the same marking,
      renaming, and retention policy to RFD 087's user-global session store.
      This phase depends on phase 2.

[RFD 130]: ../rfd/130-session-record-retention-and-read-surface.md
