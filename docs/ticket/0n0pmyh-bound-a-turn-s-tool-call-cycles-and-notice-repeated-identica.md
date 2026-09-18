# Bound a turn's tool-call cycles and notice repeated identical calls

- **Status**: Todo
- **Kind**: Feature
- **Authors**: jp
- **Date**: 2026-09-18
- **Label**: domain=conversation
- **Label**: domain=tooling
- **Label**: package=jp_cli
- **Label**: type=feature

The turn loop cycles streaming to executing with no cap.
`run_turn_loop` breaks only when the execution plan is empty, the user
intervenes, or a stream error is fatal.
Nothing counts the cycles, and nothing notices a repeated identical tool call.

`TurnState::request_count` is incremented at `turn_loop.rs:373` and read
nowhere.
Its doc comment describes a check that does not exist: "Every retry increments
this counter, until a maximum number of retries is reached, after which the turn
ends in an error."

The guards today are a per-response byte ceiling
(`assistant.request.max_response_bytes`), a per-stream idle timeout, and Ctrl-C.
All three are per-request.
None bounds a turn.

## What it costs

A model that reissues the same failing call grinds until the user stops it, on
the user's own API key.
Unattended runs (`run = "unattended"`, a persona driving a long task) have no
user watching.

The harness study in T-0n0nwz3 caps each task at 300 steps and adds streak
detection: a reminder once a streak reaches five identical calls, and early
termination at eight consecutive identical failing calls.
A streak is calls sharing a tool name and byte-identical arguments, so a
paginated read at a new offset ends it, and permission denials do not count as
failures.
The machinery is small and the thresholds are theirs to borrow.

## Shape

Two counters on `TurnState`, which already exists and already half-holds one:

- A cycle cap, configurable, `0` meaning unbounded.
- A streak counter over `(tool_name, arguments)` byte-identity, excluding
  permission denials.

## Where JP should diverge

Do not inject a reminder into the model input.
That is the paper's only option; JP has better ones.
In an attended session a streak should reach the user through the inquiry or
interrupt surface, where they can redirect rather than watch.
In an unattended one it should abort.
RFD 104's promptability signal is how the loop tells those apart.

Picking either threshold is guesswork until per-turn usage is recorded
(T-0n0pgw6), so the counters are worth landing before the numbers are argued
about.

Findings and the rest of the proposals: T-0n0nwz3.
