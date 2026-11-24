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

Remove the blocks that Claude Code abandoned from the conversation so JP's
history matches what Claude Code kept in its context.

The `stream_event` SDK message carrying the interrupted response's
`message_stop` includes an `abandoned_blocks` field with the API message ID and
the index of the first discarded block (`from_block_index`):

| Recording                         | `from_block_index` | Abandoned        | Kept                               |
| --------------------------------- | ------------------ | ---------------- | ---------------------------------- |
| `cut_off_text.jsonl:92`           | `0`                | the text block   | nothing                            |
| `cut_off_reasoned_text.jsonl:114` | `1`                | the text block   | the thinking block                 |
| `cut_off.jsonl:51`                | `2`                | the `store` call | a text block and the `lookup` call |

`cut_off.jsonl` is the `server_error` path, so `abandoned_blocks` is not
specific to text responses.
In all three fixtures, no other `message_stop` carries the field.
`SdkMessage::StreamEvent` in
`crates/jp_llm/src/provider/anthropic/acp/protocol.rs` currently deserializes
only `event` and `parent_tool_use_id`, ignoring `abandoned_blocks`.

- [ ] Deserialize `abandoned_blocks` on `SdkMessage::StreamEvent`.
- [ ] On a `message_stop` carrying `abandoned_blocks`, emit a stream event
      identifying the abandoned block range.
- [ ] In the turn loop, remove the `ChatResponse` events committed for those
      blocks and persist the removal (similar to how `FinishReason::Refused`
      revokes content).
- [ ] Update the two `keeps_the_text_of_both_attempts` tests to assert that only
      a single copy of the answer is committed.

Only `ChatResponse` events should be removed.
A committed tool call has already been dispatched and may have executed, so it
must remain in the conversation even if Claude Code lists it in
`abandoned_blocks`.
In `cut_off.jsonl`, the abandoned `store` call was never committed because JP
only commits tool calls when Claude Code sends `session/request_permission`.

Do not remove blocks when `abandoned_blocks` is absent.
Because Claude Code closes an interrupted text block with `content_block_stop`,
nothing else in the stream distinguishes an abandoned block from a completed
one; inferring abandonment from a missing stop reason would wrongly remove the
completed `lookup` block in `cut_off.jsonl`.
Although the Agent SDK reference for `SDKPartialAssistantMessage` does not
document `abandoned_blocks`, the three recorded fixtures verify its presence on
the qualified adapter and Claude Code versions.

## Follow-up

Removing abandoned blocks fixes the conversation history, but the terminal has
already printed the interrupted text before printing the restarted answer.

To avoid duplicate terminal output, have JP handle connection recovery on the
ACP route the same way it does on the API-key route: let Claude Code report the
failure, and let JP commit the partial response and restart the request from
committed history.
Verify which settings control Claude Code's retry and resume behavior using the
`Scenario` harness in `cut_off_tests.rs` before changing production code:

- Whether `CLAUDE_CODE_MAX_RETRIES=0` is respected, whether it disables the
  mid-stream retry in `cut_off_text.jsonl`, and what error Claude Code reports
  instead.
- Whether any setting disables the synthetic resume prompt in
  `cut_off_reasoned_text.jsonl` (Claude Code's error reference documents up to
  three consecutive resume prompts in non-interactive sessions without naming a
  toggle).
- How the `server_error` path in `cut_off.jsonl` behaves under those settings,
  since JP holds `server_error` until the final SDK result.

Note that each JP retry on the ACP route spawns a new adapter process and
reloads the transcript, so retries are heavier than on the API-key route.
