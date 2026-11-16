# Report how a delegated turn ended on query_complete

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-16
- **Label**: client=cli
- **Label**: domain=plugins
- **Label**: package=jp_cli
- **Label**: package=jp_plugin
- **Label**: type=feature

The host answers a plugin's `query` with `query_complete` whether the turn
finished on its own, was stopped, or was aborted.
A plugin can't tell a complete answer from one that was cut short.

## What happens

`drive_turn` (`crates/jp_cli/src/cmd/query/turn_loop.rs`) returns `Ok(())` for
all three endings, and `TurnPhase::Complete` covers both a normal finish and a
stop.
The turn's task in `run_query` (`crates/jp_cli/src/cmd/plugin/dispatch.rs`) maps
`Ok(())` to `completed`, which builds a `QueryCompleteResponse` holding only the
conversation and, with a `schema`, the structured data.

A plugin that sent the `interrupt` itself knows the turn was stopped.
One whose turn was stopped another way does not: a Ctrl-C at the host's terminal
goes to the running turn's interrupt handler, which sits above the plugin's, and
the plugin is told nothing but `query_complete`.

A query with a `schema` already reports a turn that was cut short, as an error:
`completed` finds no structured data to return.

## Proposal

Add `outcome` to `QueryCompleteResponse`:

| `outcome`  | The turn                                               |
| ---------- | ------------------------------------------------------ |
| `finished` | Ended on its own.                                      |
| `stopped`  | Was stopped, and kept what the assistant had produced. |
| `aborted`  | Was aborted, and dropped what was not yet persisted.   |

An absent field reads as `finished`, which is all an older host can say.
The field is a new protocol version, so a plugin that needs it can require it.

The turn has to return which ending it took, in place of `()`.
The exits that are a stop or an abort today:

- `TurnInputs::run` and `run_turn` in `cmd/query.rs`, which return early when a
  client's stop arrives while MCP servers load or the model is looked up.
- `drive_turn`: `complete_early` between phases, `apply_streaming_interrupt`
  during streaming (`Break` after a stop, `Abort`), and the tool phase's
  `ExecutionOutcome`.

`jp query` runs through the same functions and ignores the value.

- [x] Return the ending from every exit of the turn, with a test per exit
- [x] Send it as `outcome` on `query_complete`, and add the protocol version to
      the table in `jp_plugin::protocol`
- [x] Test through `run_plugin`: a finish, a stop sent with `interrupt`, a stop
      from the turn's interrupt handler, and an abort each report their own
      outcome
- [x] Note the field in RFD 072's description of `query_complete`

RFD D72 (`jp-review`) records how each review pass ended and depends on this.

## Comments

-----

- **From**: jp
- **Date**: 2026-11-16T14:04:22Z

Implemented.
Two things the description left open:

- **A client `abort` during tool execution reports `stopped`.** The tool phase
  already turns an abort into a stop (`as_tool_interrupt`): the running tools
  are cancelled and their cancellation responses are kept.
  `outcome` reports what happened, so it says `stopped`.
- **A stop that arrives after the assistant finished reports `finished`.**
  `TurnCoordinator` records a stop only if the turn was not already `Complete`,
  and clears it when a reply continues the turn.

The `run_plugin` tests run a real turn through `Ctx::stubbed_provider`, a
`#[cfg(test)]` seam modelled on `stubbed_now`.
To make room for it, `Query::run_turn` now takes the provider as an argument;
`TurnInputs::run` builds it at the same point in the turn as before.

Not covered by a test of its own: the abort exit inside the stream-error backoff
(`StreamErrorOutcome::Interrupted`).
It goes through the same `apply_streaming_interrupt` → `TurnPhase::Aborted`
path as the tested exits.

-----

- **From**: jp
- **Date**: 2026-11-16T14:34:18Z
- **Re**: #1

Rebasing onto `main` brought in #1349, which adds a client-interrupt exit inside
the retry backoff (`StreamErrorOutcome::ClientInterrupt`).
Its abort arm now returns `TurnEnding::Aborted`.
`a_client_abort_during_retry_backoff_ends_the_turn_as_aborted` covers it and
fails if the arm reports `Stopped`.
The Ctrl-C abort during the backoff (`StreamErrorOutcome::Interrupted`) still
has no test of its own.
