# Promote D24 and rank bounded tool output for implementation

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-09-18
- **Label**: domain=conversation
- **Label**: domain=tooling
- **Label**: package=jp_cli
- **Label**: type=task

JP places no ceiling on a tool call response.
`commit_tool_responses` writes whatever the tool produced into the stream and
flushes it, so an oversized response is durable: every later turn re-sends it,
the provider rejects the request, and the conversation needs hand-editing to
recover.
RFD D24 records the case that prompted it, a 1,293,623-token request against a
1,000,000-token limit.

D24 has been a Draft since 2026-07-27 and sits unranked in the backlog.

## Why now

The harness study in T-0n0nwz3 measures what context overflow costs an agent:
with no context management, 78.7% of SWE-Bench Verified tasks terminated on
window overflow at a 32k budget, and 8.7% still did at 128k.
Every managed tier overflowed on zero tasks at every budget.

Their harness truncates each tool result at 24k characters as part of the fixed
substrate, below the tiers they varied.
It is not one of the interventions; it is the floor the interventions stand on.
JP has no such floor.

Of the changes that reduce context pressure, this is the cheapest: one config
key resolved through the existing per-tool and `'*'` chain, no new mechanism, no
new axis.

## What to do

1. Promote D24 to Discussion.
2. Rank it.
   It gates the value of everything else done about context pressure: an
   automatic compaction trigger that fires on a conversation already poisoned by
   a single 1.2M-token response has nothing useful to do.

Findings and the rest of the proposals: T-0n0nwz3.
