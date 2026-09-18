# Stage elision before summarization in D50's automatic compaction

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-09-18
- **Label**: domain=conversation
- **Label**: domain=llm
- **Label**: package=jp_config
- **Label**: package=jp_conversation
- **Label**: type=task

RFD D50 adds automatic compaction with a single `trigger_ratio` and a single
standing rule.
The harness study in T-0n0nwz3 measured that shape against a staged one, and the
staged one won.

Their T4 elides at a soft threshold and summarizes only at a hard one.
It had the lowest mean cost in seven of eight model/benchmark panels and the
lowest peak-context ratio at all four window budgets, at success rates
comparable to every other managed tier.
The mechanism is that cheap rule-based elision handles most of the pressure
before an LLM summarization call is needed at all.
Summarization alone (their T3) and elision alone (T1, T2) both cost more.

## What changes in D50

JP already has both policies.
This is about which fires when.

D50's config today:

```
trigger_ratio = 0.75

[conversation.compaction.auto.rule]
keep_first = 1
keep_last = 3
reasoning = "strip"
tool_calls = "strip"
```

The staged form needs two thresholds and two rules: an elision rule (`tool_calls
= "strip"` with an `over` bound, which is exactly their M1) at the soft
threshold, and a summary rule (their M3) at the hard one.
Their values are 0.6 and 0.85 of the usable window, with the verbatim recent
window budgeted at 0.3 and floored at two turns.

Adopting their constants is worth more than deriving new ones.
D50 currently notes that 0.75 "is a starting guess".

## Two other changes worth making

- **Budget the recent window by size, not only by turn count.** D50's `keep_last
  = 3` protects three turns whatever they weigh.
  The paper budgets the verbatim window by tokens and floors it at two turns,
  which covers the case where the protected tail alone exceeds the budget.
  That case is D50's own open question, "what if the projection is still over
  the threshold after compacting".
- **The backstop D50 assumes does not exist.** D50 says a single turn that
  overflows on its own "remains the domain of hard-fail and truncation".
  The query path has no truncation.
  See T-0n0pex2.

## Ranking

D50 sits at 24 on the priority board, below the Internal Release v0.1 milestone.
On this evidence it belongs higher.
The paper's strongest result is that the gap between no context management and
any context management is large, while the gaps among managed strategies are
small: 35.7 percentage points of success rate at 32k, still 2.7 at 128k, against
single-digit differences between tiers.

Findings and the rest of the proposals: T-0n0nwz3.
