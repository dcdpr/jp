# Decide whether a plan belongs in conversation history

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-09-18
- **Label**: domain=conversation
- **Label**: domain=llm
- **Label**: package=jp_cli
- **Label**: type=question

This workspace's `plan` tool (`.jp/mcp/tools/plan.toml`) writes its state into
tool call responses, which live in the conversation stream.
Every call leaves a copy, so a long task accumulates stale plans in history, and
the built-in compaction rule (`tool_calls = "strip"`) targets exactly those
events.

The harness study in T-0n0nwz3 deliberately does neither.
Its planning component holds the plan in external state and re-injects the
current version before each model call, "rather than appending previous copies
to the persistent trajectory".

## Why it might matter

Their planning result for the two strongest models is a cost result: roughly 30%
cheaper on SWE-Bench, with success rates 2.0 and 0.4 percentage points lower.
The saving is almost entirely post-edit verification, not localization or
repair.
Those are the cells that correspond to JP's users, who run frontier models.

Their weak-model result (+11.6 points for Nemotron-3 30B, at higher cost) does
not transfer.

## Why it might not

- The evidence is one prompt and one update mechanism, ablated only at their
  default T4/128k setting.
  The paper says so in its limitations.
- JP is interactive.
  The human is often the planner, and the value of a machine-maintained plan is
  correspondingly lower.
- Re-injection needs a slot in `Thread` for per-request ephemeral content that
  does not exist today.
  It would have to sit after the cached prefix so prompt caching survives, which
  is a constraint the paper never faced.

## What to do

Nothing yet.
Record per-turn usage first (T-0n0pgw6), then look at whether verification churn
and accumulated stale plan copies show up in real sessions.
If they do, the design question is where per-request ephemeral content lives in
`Thread`, which is worth an RFD rather than a patch.

Filed to keep the question findable, not to schedule it.

Findings and the rest of the proposals: T-0n0nwz3.
