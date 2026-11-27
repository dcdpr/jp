# Drop the declined model's blocks after a mid-output fallback

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-27
- **Label**: domain=llm
- **Label**: llm-provider=anthropic
- **Label**: package=jp_llm
- **Label**: type=follow-up

Follow-up to T-1ssj597, which decodes the `fallback` block and announces the
serving model.

When a classifier declines partway through a response, the `fallback` block
arrives after the declined model's partial output.
JP has already streamed that output into the turn as conversation events by
then.
The [continuation rules] say that on the next turn:

- `thinking`, `redacted_thinking`, and client `tool_use` blocks before the final
  `fallback` block are dropped;
- `text` before it is kept;
- the `fallback` block itself stays in place when thinking blocks from both
  sides are echoed, or the API rejects the request.

JP records no event for the `fallback` block and keeps everything before it, so
a turn with thinking on both sides of the boundary can be rejected when it is
resent.
The thinking-signature repair (T-0q1k0j4) may recover from that 400, but only
after the fact.

Decide whether the stream should retract the declined model's thinking and
tool-call parts when the boundary arrives (an `Event::Patch`), or whether JP
should record the boundary as an event and filter at request-build time.
A tool call that the declined model started and Claude Code already dispatched
over ACP needs an answer either way.

## Comments

-----

- **From**: jp
- **Date**: 2026-11-27T22:44:47Z

Not needed.
T-1ssj597 ends the request as a refusal when the `fallback` block arrives, and
tells Claude Code not to switch models at all, so JP never keeps a turn that
crosses a fallback boundary.
The coordinator already drops the declined model's partial output on
`FinishReason::Refused`, and the ACP state retracts its pending tool calls.

[continuation rules]: https://platform.claude.com/docs/en/build-with-claude/refusals-and-fallback#continuing-the-conversation
