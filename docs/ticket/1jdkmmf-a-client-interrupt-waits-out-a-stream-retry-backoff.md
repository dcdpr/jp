# A client interrupt waits out a stream retry backoff

- **Status**: Done
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
- On graceful shutdown, the host sends `Stop` to every delegated turn
  (`RunningTurns::stop_all`) and aborts any turn still running after 5 seconds
  (`TURN_STOP_GRACE`).
  Because a turn sleeping in retry backoff does not poll `TurnInterrupts`, it
  ignores the stop request and is aborted after the grace period instead of
  stopping cleanly.

## Fix

Poll the turn's interrupts in the backoff `select!` and hand the action back to
the caller, the way a Ctrl-C is handed back as
`StreamErrorOutcome::Interrupted`.
The caller applies it with `apply_streaming_interrupt`, as the streaming loop
does for `StreamingLoopEvent::ClientInterrupt`.

- [x] Pass the turn's `TurnInterrupts` to `handle_stream_error`, and take one
      interrupt in the backoff `select!`.
- [x] Apply the returned action in `turn_loop.rs`.
- [x] Check what a `reply` does on this path: `prepare_retry_continuation` has
      already run by the time the wait starts.
- [x] Test it beside `interrupt_during_backoff_cuts_wait_short` in
      `retry_tests.rs`: a stop queued during a long backoff ends the wait, and
      its `Delivery` resolves.

Done when a stop sent during a backoff ends the turn without waiting for the
backoff to run out.

## Comments

-----

- **From**: jp
- **Date**: 2026-11-16T13:14:25Z

Fixed in `handle_stream_error` and `turn_loop.rs`: the backoff `select!` also
takes from `TurnInterrupts`, and returns the action as
`StreamErrorOutcome::ClientInterrupt`.
The turn loop flushes the renderer and applies the action with
`apply_streaming_interrupt`.

On `reply`: by the time the wait starts, `commit_partial_response` has already
committed the partial response and `prepare_retry_continuation` has reset the
event builder.
So `apply_streaming_interrupt` has no partial events left to push.
It appends the reply once, after the committed response, and returns `Break`,
which sends a fresh request that includes the reply.
`client_reply_during_backoff_follows_the_committed_partial_response` pins this.

The renderer is flushed with `flush_renderer` before the action is applied,
because the retry flush left the reasoning region open for the resent request
and the response ends here instead.

Both new tests fail with a timeout when the new select arm is disabled.
