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

Remove the blocks Claude Code abandoned from the conversation, so JP's record
matches what Claude Code kept in its own context.

Claude Code names those blocks.
The SDK message that carries the interrupted response's `message_stop` has an
`abandoned_blocks` field, holding the ID of the API message and the index of the
first block Claude Code discarded:

| Recording                         | `from_block_index` | Abandoned        | Kept                               |
| --------------------------------- | ------------------ | ---------------- | ---------------------------------- |
| `cut_off_text.jsonl:92`           | `0`                | the text block   | nothing                            |
| `cut_off_reasoned_text.jsonl:114` | `1`                | the text block   | the thinking block                 |
| `cut_off.jsonl:51`                | `2`                | the `store` call | a text block and the `lookup` call |

The third recording is the `server_error` path, so the field is not specific to
text.
In all three, no other `message_stop` carries the field.
JP discards it today: `SdkMessage::StreamEvent` in
`crates/jp_llm/src/provider/anthropic/acp/protocol.rs` decodes only `event` and
`parent_tool_use_id`.

- [ ] Decode `abandoned_blocks` from the `stream_event` envelope.
- [ ] On a `message_stop` that carries it, emit a stream event that tells the
      turn loop which blocks of the response Claude Code abandoned.
- [ ] In the turn loop, remove the chat responses committed for those blocks and
      persist the removal, the way `FinishReason::Refused` revokes content.
- [ ] Replace the two `keeps_the_text_of_both_attempts` tests with tests that
      assert a single copy of the answer is committed.

Only chat responses are removed.
A tool call JP has committed was dispatched and may have run, so it stays even
if Claude Code lists it as abandoned.
In `cut_off.jsonl` the abandoned `store` call was never committed, because JP
holds a tool call back until Claude Code asks permission for it.

Without the field, nothing is removed.
Claude Code closes an interrupted text block with `content_block_stop`, so
nothing else in the stream separates an abandoned block from a complete one.
Inferring abandonment from the missing stop reason would remove a complete block
in `cut_off.jsonl`, where the last block committed before `message_stop` is the
`lookup` call.
The type the Agent SDK reference gives for this message
(`SDKPartialAssistantMessage`) does not list the field.
Its presence is pinned by the three recordings, taken on the adapter and Claude
Code versions JP qualifies.

## Follow-up

Removing the abandoned blocks corrects the conversation but not the terminal,
which has already printed the abandoned text and then prints the answer from the
start.

To fix that as well, have JP handle connection recovery on the ACP route the
same way it does on the API-key route: let Claude Code report the failure, and
let JP commit the partial response and restart the request from committed
history.
None of the Claude Code settings this depends on is verified.
Check each with the `Scenario` harness in `cut_off_tests.rs` before changing
production code:

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
