# Serialize every ticket write, not only board moves

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-11
- **Label**: domain=tooling
- **Label**: type=follow-up

`ticket::board::move_to` and `board::rank` hold `.board.lock` from their first
read to their last write, so concurrent `ticket_move` calls and `jp ticket
status` runs can no longer lose a move (PR #1321 review).

The other writers don't take the lock:

- `store::relate`, behind `ticket_edit`'s `status` and `parent` and `jp ticket
  edit`, checks the parent rule (a parent closes only once its children are
  `done`) by reading the other tickets and then writes.
  A concurrent move or edit of a child can slip between the check and the write,
  closing a parent with an open child.

- A status change through `store::relate` or `store::set_status` (`ticket_edit`,
  `jp ticket close`, `jp rfd track`) keeps the ticket's entry in `.board.json`,
  so a ticket moved out of Todo that way and back returns at its old rank.
  `board::move_to` drops the entry; these paths don't.

- [ ] Take the ticket directory's lock in `store` for every write that reads
      other tickets first, and have `board` reuse it

- [ ] Drop a ticket's board entries whenever its status changes, whichever path
      changes it

- [ ] Regression tests that fail without the lock, as `board_tests` does for
      `move_to`
