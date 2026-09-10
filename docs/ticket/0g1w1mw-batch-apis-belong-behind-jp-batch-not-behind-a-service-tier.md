# Batch APIs belong behind `jp batch`, not behind a service tier

- **Status**: Done
- **Kind**: Feature
- **Authors**: jp
- **Date**: 2026-09-09
- **Label**: client=cli
- **Label**: domain=llm
- **Label**: llm-provider=anthropic
- **Label**: package=jp_config
- **Label**: package=jp_llm
- **Label**: type=pre-rfd

Anthropic sells no `flex` tier, so `jp q --tier flex` against `anthropic` fails
with `UnsupportedServiceTier`.
Their Message Batches API is the nearest thing they offer: same 50% discount,
asynchronous, one endpoint away.

An implementation that mapped `flex` onto that API was built and abandoned on
branch `anthropic-flex` (PR link to be added).
This ticket records why, so the next person to look at batch APIs starts from
the findings rather than the idea.

## The finding

A batch holding one request uses none of what the batch API is for.
Throughput amortization across many requests is the entire mechanism; with a
single request you pay the queue wait and get nothing back for it.

Observed on the branch: `jp q -n --tier flex "Hello there, is this working?"`
sat at 2078s and counting.
Queue latency has no relationship to prompt size.
Anthropic's "most batches complete within 1 hour" is a throughput claim about
large batches, not a latency claim about small ones.

## Why it does not work as a tier

**It is not a tier.** A service tier means "same request, different capacity."
Batch means a different interaction model: no streaming, unbounded latency,
partial billing on cancel, a different cache TTL, and it must never serve an
inquiry.
The two are independent on the wire — Anthropic's batch API accepts
`service_tier` inside `params`, rejecting only `stream`, `speed`, and
`max_tokens: 0` — so folding batch into `ServiceTier` welds two axes together.

**It is per request, not per turn.** Every `TurnPhase::Streaming` cycle submits
its own batch.
A turn with three tool calls is three queue waits.
The max-token chaining path (`MAX_CHAIN_DEPTH = 5`) and the forced-tool fallback
(`SOFT_FORCE_MAX_RETRIES = 3`) each re-enter the same path, so one truncating
response can cost five sequential batches.

**The cache economics invert.** JP defaults to `assistant.request.cache =
short`, a 5-minute TTL, which cannot survive a wait measured in tens of minutes.
Every round then re-sends the conversation as a cache write instead of a read.
Anthropic's multipliers on base input (they stack with the batch discount):
cache read 0.1x, 5m write 1.25x, 1h write 2x, batch 0.5x on everything.

For three tool rounds over a 40k-token prefix on Opus 5:

|                           | round 1                | rounds 2-3                           | total input |
| ------------------------- | ---------------------- | ------------------------------------ | ----------- |
| streamed, `cache = short` | write @ 1.25x = $0.25  | read @ 0.1x = $0.02 each             | $0.29       |
| batched, `cache = short`  | write @ 0.625x = $0.13 | cache dead, write again = $0.13 each | $0.38       |
| batched, `cache = long`   | write @ 1.0x = $0.20   | read @ 0.05x = $0.01 each            | $0.22       |

With the default cache setting, batched agentic work costs *more* than streaming
and takes an hour.
`cache = long` fixes the arithmetic and leaves a saving too thin to justify the
wait.
Batch cache hits are best-effort anyway (Anthropic quotes 30-98%).

**Naming it `flex` is a one-way door.** Anthropic may ship a real `flex` tier.
`--tier flex` persists as a conversation config delta, so the meaning would
already be on disk in users' workspaces: honour it and the flag lies, change it
and stored conversations break.

**The foreground process holds the conversation lock** for the whole wait.
Anything worth walking away from belongs in [RFD 027]'s detached execution, and
a blocking batch is a worse version of that.

## The shape that would work

`jp batch` as its own command, batching multiple pre-built queries.
That uses the API for what it is good at, and the discount becomes a side effect
rather than the motivation.
Semantics to work out: where the queries come from, how results land in
conversations, what happens to a batch that outlives the process.

`jp q --tier batch` was considered as an intermediate step and rejected for the
"it is not a tier" reason above.

## What might pull batch back in on its own

Anthropic's `output-300k-2026-03-24` beta raises `max_tokens` to 300,000 and is
available on the Batches API only.
That is a capability nothing else offers, and it belongs with a long-generation
feature or with `jp batch`, not with a tier.

## What the abandoned branch contains

Recoverable from the closed PR if any of it is wanted again:

- `crates/jp_llm/src/provider/anthropic/batch.rs`: submit a one-request batch,
  poll to completion, cancel on drop, tolerate transport failures, and replay
  the finished message as the stream events the SSE endpoint would have produced
  (so `map_event` stays the only place that maps Anthropic content onto JP
  events).
- A `Transport` seam in the Anthropic provider threaded through `call`, `chain`,
  and the forced-tool retries, so both routes share the chaining and repair
  machinery.
- `Event::KeepAlive { detail: Option<String> }`, letting a provider label what a
  silent wait is waiting on, surfaced in the CLI's waiting region.
- `providers.llm.anthropic.batch_poll_interval_secs` and `batch_max_wait_secs`.
- Withholding `service_tier` from what an inquiry inherits from the assistant,
  on the grounds that an inquiry blocks a tool call and cannot pay in latency.
  Worth revisiting on its own merits if a genuinely slow tier ever lands; it was
  not kept, because absent batch the motivation is thin and it changes what
  inquiries cost.

[RFD 027]: ../rfd/027-client-server-query-architecture.md
