# RFD 068: Forced tool retry without reasoning

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-27

## Summary

When the Anthropic API cannot combine forced tool use with extended thinking,
JP falls back to a "soft force" via a system prompt directive. If the model
ignores that directive and completes without calling the required tool, JP
should automatically retry the request with reasoning disabled so it can use
the real `tool_choice` API parameter.

## Motivation

Anthropic's API does not support `tool_choice: {type: "tool", name: "..."}` (or
`any`) when extended thinking is enabled. JP currently works around this by
switching to `tool_choice: auto` and injecting an `IMPORTANT: You MUST call...`
system prompt. This works most of the time, but the model sometimes ignores the
directive and responds with text instead of a tool call.

This breaks workflows that depend on a specific tool being called on the first
turn. The stager persona, for example, sets `tool_choice = "git_list_patches"`
so the model immediately lists patches before doing anything else. When the
model skips this, the downstream commit workflow operates on stale or missing
data.

## Design

The change lives in the turn loop (`turn_loop.rs`), not the provider layer.

### Detection

Before starting the streaming phase, record two things:

1. The original `tool_choice` value (before the provider downgrades it).
2. Whether reasoning was active for this request.

If both are true, the turn is in "soft-force mode."

After the streaming phase completes with `FinishReason::Completed` (not
interrupted, not tool calls), check:

- Was this a soft-force turn?
- Did the model produce zero tool calls?
- (If `Function(name)`: did it not call the named tool?)

If all conditions hold, trigger a retry.

### Retry

1. Keep the conversation as-is. The model's reasoning and text response from
   the first attempt are already in the conversation history, giving it full
   context.
2. Build a new `ChatQuery` with:
   - The original `tool_choice` (e.g. `Function("git_list_patches")`)
   - Reasoning disabled (override the config for this single request)
3. The provider layer now sees a forced tool call with no reasoning config, so
   it sends the real `tool_choice` parameter to the API.
4. Continue the normal turn loop from the streaming phase.

### Limits

Retry at most once. If the second attempt also fails to call the tool (which
should not happen with real `tool_choice`), proceed normally and let the user
see the response.

## Drawbacks

- Extra API call and latency when the soft-force fails. In practice this should
  be rare.
- The retry turn has no reasoning, so the tool arguments may be lower quality
  than a reasoned call. Mitigated by the fact that the model already reasoned in
  the first attempt and that context is in the conversation.

## Alternatives

- **Structured output workaround**: Use `--schema` to force JSON output of the
  tool's expected result, then manually invoke the tool. This works for specific
  workflows (e.g. staging) but doesn't generalize.
- **Always disable reasoning for forced tool calls**: Loses the benefit of
  reasoning entirely. The current soft-force works most of the time; this retry
  is a safety net for when it doesn't.

## Implementation Plan

Single phase:

1. Add soft-force tracking state to the turn loop (original `tool_choice` +
   whether reasoning was active).
2. After the streaming phase, check if a retry is needed.
3. If so, re-enter the streaming phase with reasoning disabled and the original
   `tool_choice`.
4. Add a test with `SequentialMockProvider` that verifies the retry fires when
   the first response has no tool calls.
