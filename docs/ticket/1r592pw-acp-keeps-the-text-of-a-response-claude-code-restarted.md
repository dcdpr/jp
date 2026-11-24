# ACP keeps the text of a response Claude Code restarted

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-24
- **Label**: domain=llm
- **Label**: llm-provider=anthropic
- **Label**: package=jp_llm
- **Label**: relevant-rfd=110
- **Label**: type=bug

On the Claude Code (ACP) route, when the API connection drops while a response's
text is streaming, Claude Code discards the partial text and requests the
response again.
By that point JP has already rendered and committed the partial text, because
Claude Code sends a `content_block_stop` for the interrupted block.
The conversation ends up holding the start of the answer twice (the interrupted
prefix followed by the complete answer), and both messages are sent back to the
model on the next turn.

Two recordings in `crates/jp_llm/src/provider/anthropic/acp/cut_off_tests.rs`
reproduce this through `transport::drive`:

- `cut_off_text.jsonl`: connection dropped before any content block in the
  response completed.
  Claude Code closes the text block, sends `message_stop`, emits a `system`
  message with subtype `api_retry`, and re-issues the request.
  Tested by `a_response_claude_code_retries_keeps_the_text_of_both_attempts`.
- `cut_off_reasoned_text.jsonl`: reasoning enabled, with the connection dropped
  mid-text after the thinking block completed.
  Claude Code closes the text block, sends `message_stop` without emitting an
  assistant message for the text, and sends a synthetic user message: "Your
  response above was cut off mid-stream.
  Resume directly from where it stops [...]
  If none of it survived, answer the request from the start."
  The model reasons again and writes the answer from the start.
  Tested by `a_response_claude_code_resumes_keeps_the_text_of_both_attempts`.

In both cases, the interrupted response ends with `message_stop` without a
preceding `message_delta` stop reason, and no assistant message carries an
`error`.

The API-key route does not have this problem: JP's retry handler commits the
partial response and rebuilds the request from committed history so the model
continues from where it stopped (`handle_stream_error` in
`crates/jp_cli/src/cmd/query/stream/retry.rs`).

## Fix

Have JP handle connection recovery on the ACP route the same way it does on the
API-key route: let Claude Code report the failure, and let JP commit the partial
response and restart the request from committed history.

Verify which settings control Claude Code's retry and resume behavior using the
`Scenario` harness in `cut_off_tests.rs` before changing production code:

- [ ] Check whether `CLAUDE_CODE_MAX_RETRIES=0` is respected, whether it
      disables the mid-stream retry in `cut_off_text.jsonl`, and what error
      Claude Code reports instead.
- [ ] Check whether any setting disables the synthetic resume prompt in
      `cut_off_reasoned_text.jsonl` (Claude Code's error reference documents up
      to three consecutive resume prompts in non-interactive sessions without
      naming a toggle).
- [ ] Check how the `server_error` path in `cut_off.jsonl` behaves under those
      settings, since JP holds `server_error` until the final SDK result.
- [ ] Configure the verified settings in `options::environment` and map the
      resulting Claude Code error to a retryable `StreamError`.
- [ ] Update the two `keeps_the_text_of_both_attempts` tests to assert that only
      a single copy of the answer is committed.

If the synthetic resume prompt cannot be disabled, retract the partial block on
that path instead: treat a `message_stop` with no preceding stop reason as an
abandoned response and revoke the last committed block (similar to how
`FinishReason::Refused` revokes content).
This keeps the conversation history clean, though the partial text has already
been printed to the terminal.

Note that each JP retry on the ACP route spawns a new adapter process and
reloads the transcript, so retries are heavier than on the API-key route.
