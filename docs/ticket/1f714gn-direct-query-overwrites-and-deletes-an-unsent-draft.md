# Direct query overwrites and deletes an unsent draft

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-07
- **Label**: client=cli
- **Label**: domain=conversation
- **Label**: package=jp_cli
- **Label**: relevant-rfd=131
- **Label**: type=bug

A query sent without composing (`jp q <query>`, piped stdin, or `--no-edit`)
destroys an unsent `QUERY_MESSAGE.md` draft: the draft is replaced before the
turn starts, and the file is removed after the turn succeeds.
The draft is lost even when the turn fails.

## Reproduce

1. Leave a draft behind: compose with `jp q --edit` and interrupt before the
   turn starts, or have the turn fail.
2. Run `jp q "something else"` on the same conversation.
3. `QUERY_MESSAGE.md` now holds `something else`, and after a successful turn it
   is gone.

Requires user-local storage; without it no recovery copy is written.

## Cause

In `Query::run` (`crates/jp_cli/src/cmd/query.rs`):

- For any query not composed in the editor, `preserve_query_message_file` writes
  the request text over `QUERY_MESSAGE.md` as a recovery copy for a pre-turn
  interrupt.
- `draft_fingerprint` is taken after that write, so the cleanup after a
  successful turn (`DraftRemoval::IfUnchanged`) matches the overwritten file and
  removes it.

[RFD 093] states the opposite: a direct send leaves the draft neither modified
nor consumed.

## Fix

[RFD 131] Phase 6, independent of the rest of that RFD:

- [ ] Skip the recovery copy while an unsent draft exists; the query came from
      the command line or a pipe, so the user still has it.
- [ ] On an interactive terminal, ask: `[s]` send and discard the draft, `[k]`
      send and keep it, `[e]` edit the draft with the query appended, `[x]`
      cancel.
- [ ] Without an interactive terminal, send and keep the draft.
- [ ] Tests: the draft survives a direct send whose turn fails, and one whose
      turn succeeds with `[k]`.

[RFD 093]: ../rfd/093-inline-first-query-composition.md
[RFD 131]: ../rfd/131-queued-replies.md
