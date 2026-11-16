# RFD 068: Forced Tool Retry Without Reasoning

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-27
- **Summary**: Retries forced tool calls with reasoning disabled when an
  Anthropic model ignores a soft-forced tool directive.

## Summary

When the Anthropic API cannot combine forced tool use with extended thinking, JP
falls back to a soft-force system prompt directive.
If the model ignores that directive and finishes without calling the required
tool, JP retries the request with reasoning disabled (or escalating nudges on
always-thinking models) to enforce the tool call.

## Motivation

Anthropic's API does not support `tool_choice: {type: "tool", name: "..."}` (or
`any`) when extended thinking is enabled.
JP works around this by switching to `tool_choice: auto` and injecting an
`IMPORTANT: You MUST call...` system prompt.
This works most of the time, but the model sometimes ignores the directive and
responds with text instead of a tool call.

This breaks workflows that require a specific tool call on the first turn.
For example, the stager persona sets `tool_choice = "git_list_patches"` so the
model lists patches before doing anything else.
When the model skips that call, the downstream commit workflow operates on stale
or missing data.

## Design

> [!NOTE]
> As implemented, the retry lives in the Anthropic provider
> (`crates/jp_llm/src/provider/anthropic.rs`) rather than the turn loop
> described below.
> The provider already tracks the original `tool_choice` and whether thinking
> was downgraded, so handling the retry there keeps this provider-specific API
> restriction out of the turn loop.
>
> The implementation also covers models that cannot disable thinking
> (`claude-fable-5`).
> Because the API always rejects forced `tool_choice` for those models, the
> retry described below is unavailable; instead, the provider sends escalating
> soft-force nudges with thinking enabled.

The change lives in the turn loop (`turn_loop.rs`), not the provider layer.

### Detection

Before starting the streaming phase, record:

1. The original `tool_choice` value (before the provider downgrades it).
2. Whether reasoning was active for the request.

When the request has both a forced `tool_choice` and active reasoning, the turn
runs in "soft-force mode."

After the streaming phase completes with `FinishReason::Completed` (neither
interrupted nor ending in tool calls), trigger a retry if all of the following
hold:

- The turn ran in soft-force mode.
- The model produced zero tool calls.
- For `Function(name)`, the model did not call the named tool.

### Retry

1. Keep the conversation history intact.
   It already contains the model's reasoning and text response from the first
   attempt, preserving that context for the retry.
2. Build a new `ChatQuery` with:
   - The original `tool_choice` (e.g. `Function("git_list_patches")`).
   - Reasoning disabled (overriding the config for this single request).
3. Because the retry request has a forced `tool_choice` and no reasoning config,
   the provider layer sends the forced `tool_choice` parameter to the API.
4. Resume the turn loop from the streaming phase.

### Limits

Retry at most once.
If the second attempt also fails to call the tool (which should not happen when
the API enforces `tool_choice`), proceed normally and show the response to the
user.

## Drawbacks

- A failed soft-force directive adds an extra API call and latency, though
  soft-force failures are rare in practice.
- Because reasoning is disabled on the retry, the tool arguments may be lower
  quality than a reasoned call.
  Retaining the first attempt's reasoning and text response in the conversation
  history mitigates this.

## Alternatives

- **Structured output workaround**: Use `--schema` to force JSON output of the
  tool's expected result, then invoke the tool manually.
  This works for specific workflows (e.g. staging) but does not generalize.
- **Always disable reasoning for forced tool calls**: Disabling reasoning on the
  first attempt loses reasoning on every forced tool call.
  Soft-forcing works for most requests; retrying without reasoning acts as a
  fallback only when the directive fails.

## Implementation Plan

Single phase:

1. Track soft-force state in the turn loop (the original `tool_choice` and
   whether reasoning was active).
2. After the streaming phase, check whether the response requires a retry.
3. On retry, re-enter the streaming phase with reasoning disabled and the
   original `tool_choice`.
4. Add a `SequentialMockProvider` test verifying that a first response with no
   tool calls triggers the retry.
