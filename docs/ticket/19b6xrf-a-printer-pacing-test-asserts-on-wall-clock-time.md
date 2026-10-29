# A printer pacing test asserts on wall-clock time

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-10-27
- **Label**: package=jp_printer
- **Label**: type=task

`bounded_latency_controller_drains_large_queue_quickly`, in
`crates/jp_printer/src/printer_tests.rs`, prints 200 characters through the
typewriter and asserts the drain takes less than 2 s.
On 2026-10-21 it failed twice on macOS, once during the full workspace run and
once on its own, at 25 s and 24.7 s.
It passed in 1.7 s when run on its own again shortly after, with no change in
between.

## Why it is fragile

Its comment expects the drain to take about 200 ms.
By the code it takes about 1 s: `flush` leaves the controller in live mode, so
the per-character delay, `min(50 ms, 200 ms / pending)`, grows from 1 ms toward
50 ms as the queue empties, and the 200 delays add up to about 0.96 s.
That leaves the 2 s limit about twice the time the test needs.

The 25 s failures come to 125 ms per character, longer than any delay the
controller asks for, so something stretched the timed waits themselves.
Timer coalescing on macOS would explain it, but that is unconfirmed.

## Suggested change

Test what the controller decides rather than how long the waits take.
`DelayControl::effective_delay` is pure, so the delay it returns for a given
queue depth, budget, and drain state can be asserted exactly.
Keep at most one end-to-end check, with a limit far enough from the expected
time that it only catches a controller that stopped shrinking the delay.
