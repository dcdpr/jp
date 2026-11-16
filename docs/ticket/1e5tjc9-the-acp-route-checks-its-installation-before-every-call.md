# The ACP route checks its installation before every call

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-11-05
- **Label**: domain=llm
- **Label**: llm-provider=anthropic
- **Label**: package=jp_cli
- **Label**: package=jp_llm
- **Label**: type=bug

On the Claude Code (ACP) route, the Anthropic provider calls `acp::inspect`
before every `model_details`, `models`, and query
(`crates/jp_llm/src/provider/anthropic.rs`, around lines 369, 440, 464, and
499).
`inspect` runs three checks one after another, each in its own process: the
adapter version, the `claude` version, and the login status
(`crates/jp_llm/src/provider/anthropic/acp.rs`, around line 321).
That cost lands even on turns that never use the route.
When the inquiry model differs from the assistant's, `run_turn_loop` builds the
inquiry backend at the start of every turn and calls `model_details` on it
(`crates/jp_cli/src/cmd/query/turn_loop.rs`, around line 1236), so a query that
asks no inquiry still pays for it.

A trace of `jp q -n hello world?` against a vLLM model, with
`anthropic/claude-haiku-4-5` as the inquiry model on a subscription, shows the
three processes taking 640 ms before the turn starts (02.377 to 03.015 in the
trace), each ending with `ACP child cleanup: No such process`.

## Fix

Two independent changes:

- Run `inspect` at most once per process for a given login directory, caching a
  success; a failure is not cached, so a fix to the installation takes effect on
  the next call.
- Build the inquiry backend's model details on first use rather than at turn
  start.

## Test

A counting fake for the ACP checks: two `model_details` calls and a query run
the three checks once in total.
A turn with a distinct inquiry model and no inquiry never calls the inquiry
provider's `model_details`.
