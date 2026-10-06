# Write a process RFD for unattended agent runs

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-10-06
- **Label**: domain=tooling
- **Label**: type=task

Once the loop harness (T-0yh5hqj) and the rustqual mission (T-0yh5sy9) have run
for real, write a Process RFD describing how JP runs unattended agent work.
Write it from what actually happened, not ahead of time.

Cover:

- what a mission is and how to add one;
- when the harness may discard an iteration by resetting its own branch, and why
  the assistant itself never rewrites history;
- memory across conversations: the ledger, the attempted and rejected lists, and
  rotation;
- how rejected findings and overnight output get reviewed and merged, including
  the expected PR granularity;
- what an unattended run must never touch.
