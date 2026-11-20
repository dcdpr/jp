# Decide how to resolve conflicting copies of one conversation

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-20
- **Label**: domain=storage
- **Label**: package=jp_cli
- **Label**: package=jp_storage
- **Label**: type=pre-rfd

One conversation ID can end up with two copies that disagree, and JP has no
single concept or command for resolving them.
The cases known so far:

- A projected conversation whose workspace copy is broken while its user-local
  copy loads, or the reverse (noted on T-1n2tta8).
  If the broken copy is the newer one, loading picks it and fails.
- T-0sr3zez: saving a conversation deletes every other directory with its ID,
  including a copy the load-failure report just listed.

Both copies are the same conversation that has diverged, so this is
merge-conflict resolution, not the trash-restore case (which gives the restored
copy a new ID, since nothing pointed at it).
The likely choices per conflict are "keep this copy", "keep that copy", and
"split one off under a new ID"; which copy keeps the original ID, and whether
sessions and references follow it, is the decision an RFD needs to settle.
`jp conversation repair` (T-1n2ttab) is the natural place for the actions, and
`jp doctor` (T-1n2ttac) for listing conflicts.
