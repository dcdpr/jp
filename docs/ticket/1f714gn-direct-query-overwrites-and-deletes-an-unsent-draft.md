# Direct query overwrites and deletes an unsent draft

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-07
- **Label**: client=cli
- **Label**: domain=conversation
- **Label**: package=jp_cli
- **Label**: relevant-rfd=131
- **Label**: type=bug

Sending a query that bypasses composition (`jp q <query>`, piped stdin, or
`--no-edit`) destroys any unsent `QUERY_MESSAGE.md` draft: JP overwrites the
draft before the turn starts and deletes the file after the turn succeeds.
Even if the turn fails, the previous draft text is already overwritten.

## Reproduce

1. Leave an unsent draft on disk by composing with `jp q --edit` and
   interrupting before the turn starts, or by letting a turn fail.
2. Run `jp q "something else"` on the same conversation.
3. `QUERY_MESSAGE.md` is overwritten with `something else`, and once the turn
   succeeds the file is deleted.

Requires user-local storage; without it, JP does not write `QUERY_MESSAGE.md`.

## Cause

In `Query::run` (`crates/jp_cli/src/cmd/query.rs`):

- For any query not composed in the editor, `preserve_query_message_file`
  overwrites `QUERY_MESSAGE.md` with the new request text as a recovery copy for
  pre-turn interrupts.
- `draft_fingerprint` is computed after that write, so post-turn cleanup
  (`DraftRemoval::IfUnchanged`) sees a matching fingerprint and deletes the
  file.

[RFD 093] specifies the opposite: a direct send must neither modify nor consume
an existing draft.

## Fix

Implemented as [RFD 131] Phase 6 (independent of the rest of [RFD 131]):

- [ ] Skip writing the pre-turn recovery copy when an unsent draft already
      exists; the direct query came from CLI arguments or stdin that the caller
      still holds.
- [ ] On an interactive terminal, prompt before sending: `[s]` send and discard
      the draft, `[k]` send and keep the draft, `[e]` edit the draft with the
      query appended, or `[x]` cancel.
- [ ] Without an interactive terminal, send the query and keep the draft.
- [ ] Add tests verifying that an unsent draft survives both a direct send whose
      turn fails and a direct send that succeeds with `[k]`.

[RFD 093]: ../rfd/093-inline-first-query-composition.md
[RFD 131]: ../rfd/131-queued-replies.md
