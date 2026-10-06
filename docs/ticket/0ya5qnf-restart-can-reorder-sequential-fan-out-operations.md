# Restart can reorder sequential fan-out operations

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-10-06
- **Label**: domain=mcp
- **Label**: package=jp_mcp

With `fan_out = { concurrency = 1 }`, the operations of one call are meant to
run in the order the assistant wrote them (RFD 117, "Scheduling").
A restart from the interrupt menu can break that order.

The parent's `Gate` (`jp_mcp::server::service`) marks an operation as passed
when it first enters (`try_enter`), and never clears that mark.
A restart pauses the unfinished children (`Service::pause_call`) and resumes
them on the same parent, with the same gate.
After the resume, an earlier operation that had already entered counts as passed
even though it has not entered again, so a later operation can take the free
slot first and run before it.

Input that hits it: an ordered batch of writes (`concurrency = 1, on_error =
"stop"`), Ctrl-C during the first write, then Restart.
The second write can run before the first one runs again.

Found by reading the code during review of #1182; not reproduced in a test yet.

## Proposal

Track "settled" separately from "has entered at least once".
An operation that was paused before it settled goes back to not passed when its
attempt ends, so the ordering check waits for it again.
A test needs a tool that blocks until released, a pause of both children while
the first runs, and a resume in reverse order, asserting the run order.
