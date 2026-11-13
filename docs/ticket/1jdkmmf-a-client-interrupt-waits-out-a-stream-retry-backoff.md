# A client interrupt waits out a stream retry backoff

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-13
- **Label**: client=cli
- **Label**: domain=plugins
- **Label**: package=jp_cli
- **Label**: type=bug

A turn that is waiting to retry a failed provider request does not act on a
client interrupt until the wait ends.

## What happens

`handle_stream_error` (`crates/jp_cli/src/cmd/query/stream/retry.rs`) sleeps for
the backoff inside a `select!` with two arms: the sleep, and a temporary
signal-router handler that lets a Ctrl-C cut the wait short.
The turn's `TurnInterrupts` queue is not polled there.
The streaming loop, the tool phase, and the gap between phases all poll it.

An interrupt queued during the wait is taken once the sleep ends, by
`client_interrupt_between_phases` in `turn_loop.rs`.

The wait is at most `assistant.request.max_backoff_secs` (default `60`) plus up
to a quarter of that in jitter, so 75 seconds at the defaults.
A provider's `Retry-After` is used when given, under the same cap.

## What it costs

- A plugin's `interrupt` request (`stop`, `abort` or `reply`) sent while the
  turn backs off goes unanswered for the rest of the wait.
  For `jp serve-chat`, a "stop" sent during a rate-limit wait appears to hang,
  and the conversation stays locked until the wait ends.
- On a graceful shutdown the host queues a stop for every delegated turn
  (`RunningTurns::stop_all`) and aborts what is still running five seconds later
  (`TURN_STOP_GRACE`).
  A turn in a backoff does not read the stop, so it is aborted rather than
  ending on its own.

## Fix

Poll the turn's interrupts in the backoff `select!` and hand the action back to
the caller, the way a Ctrl-C is handed back as
`StreamErrorOutcome::Interrupted`.
The caller applies it with `apply_streaming_interrupt`, as the streaming loop
does for `StreamingLoopEvent::ClientInterrupt`.

- [ ] Pass the turn's `TurnInterrupts` to `handle_stream_error`, and take one
      interrupt in the backoff `select!`.
- [ ] Apply the returned action in `turn_loop.rs`.
- [ ] Check what a `reply` does on this path: `prepare_retry_continuation` has
      already run by the time the wait starts.
- [ ] Test it beside `interrupt_during_backoff_cuts_wait_short` in
      `retry_tests.rs`: a stop queued during a long backoff ends the wait, and
      its `Delivery` resolves.

Done when a stop sent during a backoff ends the turn without waiting for the
backoff to run out.
