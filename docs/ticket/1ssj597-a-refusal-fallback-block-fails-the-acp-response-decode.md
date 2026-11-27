# A refusal fallback block fails the ACP response decode

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-27
- **Label**: domain=llm
- **Label**: llm-provider=anthropic
- **Label**: package=jp_llm
- **Label**: type=bug

When a safety classifier declines a request, Claude Code can hand it to another
model.
The response then carries a `fallback` content block marking the handoff
([refusals and fallback]):

```json
{"type": "fallback", "from": {"model": "claude-fable-5-2"}, "to": {"model": "claude-opus-4-8"}, "trigger": {"type": "refusal", "category": "cyber"}}
```

Claude Code forwards that response over ACP in `_claude/sdkMessage`.
`async_anthropic::types::MessageContent` had no variant for it, so decoding the
notification failed and the query ended with an opaque protocol error:

```
Claude ACP protocol error: unknown variant `fallback`, expected one of `tool_use`, `tool_result`, `text`, `thinking`, `document`, `redacted_thinking`
```

JP has no model fallback, so the subscription route must not answer with a model
the user did not choose.
A flagged request ends as a refusal, the same as on the HTTP route, and the
notice tells the user to reword the request or pick another model.

- [x] Claude Code is told not to switch models on a flagged request
      (`switchModelsOnFlag: false`).
- [x] `MessageContent` decodes a `fallback` block.
- [x] A `fallback` block ends the request as `FinishReason::Refused` on both
      routes, naming the suggested model, and the ACP connection closes before
      the other model's output is read.
- [x] The refusal notice tells the user to reword the request or retry with
      `--model`.
- [x] Tests drive a `fallback` block through the ACP notification path and a
      scripted connection.

[refusals and fallback]: https://platform.claude.com/docs/en/build-with-claude/refusals-and-fallback
