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
text streams, Claude Code discards that text and has the model produce the
response again.
JP has rendered and committed the text by then, because the `content_block_stop`
Claude Code sends for the interrupted block commits it.
The conversation then holds the start of the answer twice: the interrupted text,
followed by the complete answer.
Both are sent back to the model on the next request.

Two recordings show it, each replayed through `transport::drive` by a test in
`crates/jp_llm/src/provider/anthropic/acp/cut_off_tests.rs`:

- `cut_off_text.jsonl`: a cut before any block of the response completed.
  Claude Code closes the text block, sends `message_stop`, reports a `system`
  message with subtype `api_retry`, and re-issues the request.
  Pinned by `a_response_claude_code_retries_keeps_the_text_of_both_attempts`.
- `cut_off_reasoned_text.jsonl`: reasoning on, and a cut mid-text after the
  thinking block completed.
  Claude Code closes the text block, sends `message_stop`, emits no assistant
  message for the text, and sends the model a synthetic user message: "Your
  response above was cut off mid-stream.
  Resume directly from where it stops [...]
  If none of it survived, answer the request from the start."
  The model thinks again and writes the answer from the start.
  Pinned by `a_response_claude_code_resumes_keeps_the_text_of_both_attempts`.

In both, the interrupted response ends with `message_stop` and no
`message_delta` carrying a stop reason, and no assistant message carries an
`error`.

The API-key route does not have this problem.
JP's own retry commits the partial response and rebuilds the request from
committed history, so the model continues from it (`handle_stream_error` in
`crates/jp_cli/src/cmd/query/stream/retry.rs`).

## Fix

Make JP the only side that recovers from a lost connection, so the ACP route
behaves like the API-key route: Claude Code reports the failure, and JP's retry
commits the partial response and restarts the request from committed history.

None of the settings this needs is verified.
Settle each with a probe recording (the `Scenario` harness in
`cut_off_tests.rs`) before changing code:

- [ ] Check whether `CLAUDE_CODE_MAX_RETRIES=0` is accepted, whether it stops
      the mid-stream re-send in `cut_off_text.jsonl`, and what Claude Code
      reports instead.
- [ ] Check whether any setting stops the synthetic resume prompt in
      `cut_off_reasoned_text.jsonl`.
      Claude Code's error reference documents the prompt (up to three in a row
      in a non-interactive session) without naming a switch.
- [ ] Check what the `server_error` path in `cut_off.jsonl` does under those
      settings, since JP holds that report until the SDK result.
- [ ] Set the settings in `options::environment`, and map the failure Claude
      Code then reports to a retryable `StreamError`.
- [ ] Replace the two `keeps_the_text_of_both_attempts` tests with ones that
      assert a single copy of the answer.

If the resume prompt cannot be turned off, retract on that path instead: treat a
`message_stop` with no preceding stop reason as an abandoned response, and
remove what its last block committed, the way `FinishReason::Refused` revokes
content.
That corrects the conversation but not the terminal, which has already shown the
text.

Each JP retry on this route starts a new adapter process and reloads the
transcript, so a retry costs more here than on the API-key route.
