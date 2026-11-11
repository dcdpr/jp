# Serialize every ticket write, not only board moves

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-11
- **Label**: domain=tooling
- **Label**: type=follow-up

`ticket::board::move_to` and `board::rank` hold `.board.lock` from their first
read to their last write, so concurrent `ticket_move` calls and `jp ticket
status` runs cannot overwrite each other's moves (PR #1321 review).

The other writers don't take the lock:

- `store::relate` (used by `ticket_edit` and `jp ticket edit` for `status` and
  `parent` changes) checks parent-child completion rules by reading the other
  tickets before writing.
  A concurrent move or edit of a child can slip between the check and the write,
  closing a parent while a child is still open.

- A status change through `store::relate` or `store::set_status` (`ticket_edit`,
  `jp ticket close`, `jp rfd track`) keeps the ticket's entry in `.board.json`,
  so a ticket moved out of Todo and back via those paths retains its old rank
  instead of dropping it like `board::move_to` does.

- [ ] Take the ticket directory's lock in `store` for every write that reads
      other tickets first, and have `board` reuse it

- [ ] Drop a ticket's board entries whenever its status changes, whichever path
      changes it

- [ ] Add regression tests that fail without the lock, matching the `move_to`
      concurrency tests in `board_tests`
