# RFD D59: Cross-Session Rate-Limit Budgets

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-09-04

## Summary

Providers meter usage per account, but each `jp` process retries in isolation,
so several sessions sharing one API key overdraw the same bucket and then retry
into it together.
This RFD records what the provider reports about the remaining budget in a
user-local store keyed by credential, and makes a session wait before sending
when that budget is nearly spent.
The budget is **observed, never predicted**: JP stores what the provider said,
not what JP thinks a request will cost.

## Motivation

[Issue 1069] asks for per-minute throttling across all sessions against
Cerebras.
Cerebras enforces its limits per *organization*, and `jp` runs one process per
conversation, so parallel sessions ([RFD 020]) on one key contend for a bucket
none of them can see.

Today the only defence is the retry loop, which is reactive by construction: the
request goes out, comes back 429, and the turn stalls for the `retry-after` the
provider names.
With `max_retries` at its default of 5 and Cerebras answering `retry-after: 60`,
an unlucky session can spend five minutes waiting and still fail.

### Why the budget cannot be computed

The obvious design is a shared ledger: estimate what a request will cost,
decrement a counter, refill it at the limit rate.
That requires knowing the charging rule, and three rounds of controlled
measurement against Cerebras failed to produce one.

The reservation is taken at admission, before the response exists.
Six readings with a short prompt gave exactly `min(max_completion_tokens,
16384)` with no contribution from the prompt at all:

| `max_completion_tokens` | remaining after | reserved |
| ----------------------- | --------------- | -------- |
| 1                       | 499,999         | 1        |
| 512                     | 499,488         | 512      |
| 4,096                   | 495,904         | 4,096    |
| 16,384                  | 483,616         | 16,384   |
| 40,960                  | 483,616         | 16,384   |
| unset                   | 483,616         | 16,384   |

The same request with a ~64,000-token prompt reserved 80,384, which is `64,000 +
16,384`.
So the prompt contributes at that size and not at 68 tokens.
A prompt-caching explanation fit until the same large prompt was sent twice and
the second request was charged *more* than the first, not less.

The rule may be coarse-grained input estimation, or something else.
It is not derivable from the documented behaviour, and it is not stable enough
to encode.

There is a second, independent reason a computed ledger cannot work.
The meter counts what the *organization* consumed: another key, a colleague's
machine, a `curl` in someone's terminal.
Even a perfect record of JP's own usage would be missing an unbounded amount of
the total.

Both problems disappear if JP stops computing and starts reading.
Cerebras returns the answer on every successful response:

```
x-ratelimit-limit-tokens-minute: 500000
x-ratelimit-remaining-tokens-minute: 419616
```

## Design

### What the user sees

A session that would push the budget past its floor waits instead of sending,
using the same shape as conversation-lock contention ([`acquire_lock`]): silent
for a moment, then a timer line, then an interactive prompt if the wait runs
long.

```
⏳ Waiting for rate-limit headroom (cerebras, tokens/minute) — 12s
```

```
? Rate limit nearly exhausted for cerebras (tokens/minute: ~8,200 of 500,000).
> Continue waiting
  Send anyway
  Cancel
```

Ctrl-C during the wait jumps straight to the prompt rather than aborting the
turn, matching lock-wait behaviour.
Non-interactive runs wait up to the timeout and then send regardless, because
unlike a lock conflict a rate limit resolves itself: sending produces a 429 the
existing retry layer already handles, and failing the command outright would be
a regression on today's behaviour.

Configuration mirrors the existing pair, behaviour in its own section and
presentation under `style`:

```toml
[rate_limit]
enable = true          # set false to restore today's send-and-retry behaviour

[style.rate_limit_wait]
show = true
delay_secs = 1
interval_ms = 100
timeout_secs = 10      # then prompt
```

### The observation

```rust
/// What a provider reported about one of its rate-limit buckets, at one instant.
pub struct RateLimitSnapshot {
    pub unit: LimitUnit,        // Tokens | Requests
    pub window: Duration,       // 60s, 3600s, 86400s
    pub limit: u64,
    pub remaining: u64,
    pub observed_at: DateTime<Utc>,
}
```

Providers extract zero or more snapshots from a response head:

```rust
trait Provider {
    /// Rate-limit buckets this provider reported, if it reports any.
    fn rate_limit_snapshots(&self, _headers: &HeaderMap) -> Vec<RateLimitSnapshot> {
        vec![]
    }
}
```

The default returns nothing, so a provider that reports nothing simply gets no
throttling.
Cerebras yields six snapshots (tokens and requests, across minute, hour and
day).
OpenAI and Anthropic report comparable headers under different names and can be
added later without touching anything else.

This keeps providers and buckets orthogonal: a new provider is one adapter, a
new bucket kind touches no provider.

**Snapshots are taken from the response head, not at stream completion.**
Headers arrive before the first byte of body, so a stream that fails or is
interrupted still yields its observation — and those are exactly the
observations taken during the trouble that makes throttling worth having.

### The store

One file per credential under the user data directory
([`jp_workspace::user_data_dir`]), holding the most recent snapshot per `(unit,
window)`:

```
$JP_USER_DATA_DIR/rate-limits/cerebras-a3f19c2e.json
```

The suffix is a truncated SHA-256 of the API key.
It identifies the credential without storing anything reversible, and key
rotation starts a fresh budget for free.
The digest is never logged.

Writes go through the advisory locking in `jp_storage::lock` and land
atomically.
**Any failure to read, parse, or lock the store degrades to "no budget known",
which sends.** A throttle that can stall the CLI is worse than no throttle.

### The decision

Before a provider request, a session loads the snapshots, projects each forward
to now at `limit / window` per second (the token-bucket refill Cerebras
documents, capped at `limit`), and waits while any projected remaining sits
below a floor.

The floor cannot be a prediction of what this request will cost, for the reasons
in the Motivation.
It is instead **the largest single drop observed recently**: the difference
between consecutive snapshots, corrected for refill, is the measured drain of
real traffic including consumers JP cannot see.
The store keeps the last few drops per bucket and uses their maximum.

With no history the floor is zero and JP sends, so a first run is never worse
than today.

### Where the pieces live

`jp_llm` owns the snapshot type, the per-provider extraction, and the store.
`jp_cli` owns the wait and the prompt, because prompting is a terminal concern.
The collect-path callers (`collect_with_retry`, used by title generation,
summarization and inquiries) consume quota too and take the same wait, but
silently and bounded, since there is nobody watching to answer a prompt.

## Drawbacks

**A new blocking state in `jp query`.** Today a query either streams or fails.
Adding a wait before the request means a new state to interrupt, explain and
test, and it is the bulk of the work in this proposal.
The accounting is bookkeeping; this is the cost.

**Behaviour now depends on state other processes wrote.** A session's decision
is shaped by a file another `jp` may be writing concurrently.
The degradation rule above bounds the damage, but the coupling is real and did
not exist before.

**The floor is a heuristic and will be wrong sometimes.** Too low and JP still
hits 429s, which is today's behaviour.
Too high and JP throttles itself harder than the provider would, which is a new
failure mode and a worse one, because it is invisible: the user sees a wait with
no external cause.

**One more thing to keep current.** Every provider's header dialect is a
hand-maintained mapping against an API that moves, in the same way the model
tables are.

## Alternatives

**Predict the charge and keep a ledger.** The original plan, and the reason this
RFD exists in its current form.
Rejected on the evidence in the Motivation: the charging rule is not derivable,
and even a correct one would miss consumption by other clients on the same
account.

**Derive the budget from recorded usage.** A companion design records
per-request token usage on the conversation stream.
Summing it looks like it would answer the same question, but it measures what JP
spent rather than what the account has left, and the gap between them is
unbounded.
It would also require reading every conversation's stream to answer a question
scoped to sixty seconds.
The two designs read different parts of the response (body versus headers) at
different times, and share a theme rather than a seam.

**Serialize provider requests across sessions with a lock.** Correct and simple,
and it throws away the parallelism [RFD 020] exists to provide.

**Do nothing beyond the retry layer.** The 429 handling with jittered backoff is
already correct and already merged, and it is what this design falls back to.
The case against leaving it there is that reacting costs a full round trip and a
provider-dictated wait each time, and on a small plan the bucket is exhausted
for most of every minute.

## Non-Goals

**Accounting for consumers JP cannot see.** Another machine, another key, a
script.
The observed remaining already includes their effect, but JP cannot anticipate
them, so a burst from elsewhere between two observations will still produce a
429.

**Organizations sharing one key across machines.** The store is user-local by
design.
Two developers on one key each keep their own view and each believe they have
the full budget.
The failure mode is graceful: they learn from the 429, which is today's
behaviour.

**Multiple keys on one account.** Cerebras meters per organization, so two keys
share a meter while this design gives each its own budget.
The key is the best identity JP has, not the true one.

**Replacing the retry layer.** This is additive.
The 429 path stays exactly as it is and remains the backstop.

**Recording usage or cost.** A separate concern with a separate home.

## Risks and Open Questions

**Can we see the response headers at all?** `reqwest_eventsource::EventSource`
takes ownership of the request and its `Event::Open` carries no headers, so the
successful path may have no access to the very data this design needs — while
the error path already has it via `InvalidStatusCode`.
That would be backwards.
The likely route is dropping to `reqwest` plus `eventsource-stream` (which
`reqwest-eventsource` is itself built on, and which is already a dev-dependency
of `jp_llm`) in the providers that implement the trait.
**This needs a spike before the rest of the plan is worth scheduling**, because
it decides whether the change is confined to a new module or reaches into every
streaming provider.

**How should the floor be tuned?** The largest-recent-drop rule is a starting
point chosen because it needs no formula, not because it has been validated.
It cannot be validated without real multi-session traffic.
Worth shipping behind `rate_limit.enable` and revisiting with data.

**Which bucket dominates?** Cerebras reports six.
A day-window bucket sitting at zero should probably not trigger the same wait as
a minute-window one, since the wait would be hours.
The design likely needs to ignore buckets whose window exceeds some bound, and
that bound is unsettled.

**Does a 429 update the store?** Cerebras strips the `x-ratelimit-*` headers
from its 429 responses, so the most informative moment yields no observation.
The fallback is to record "remaining was zero at this instant" from the status
code alone, which is a different shape from a snapshot and may not be worth the
special case.

## Implementation Plan

**Phase 1 — Spike: response-head access.** Determine whether a streaming
provider can read response headers without abandoning `reqwest_eventsource`.
Output is a decision, not necessarily code.
Everything else depends on it.

**Phase 2 — Snapshot type and Cerebras extraction.** `RateLimitSnapshot`,
`LimitUnit`, the defaulted trait method, and the Cerebras adapter with unit
tests against captured header sets.
Mergeable alone; nothing consumes it yet.

**Phase 3 — The store.** Load, atomic write, advisory locking, credential
digest, and the degrade-to-send rule on every failure path.
Mergeable alone, still unconsumed.
Tests cover a corrupt file, a missing directory, and a lock held by another
process.

**Phase 4 — The decision, without waiting.** Project snapshots forward, compute
the floor, and log when a request *would* have waited.
Shipping this first gives real data on how often the floor triggers and how
wrong it is, before any user sees a pause.

**Phase 5 — The wait and the prompt.** The blocking state, the timer line, the
Ctrl-C path, the non-TTY behaviour, and the silent bounded variant for the
collect path.
Depends on 4, and is the phase with a user-visible behaviour change.

**Phase 6 — A second provider.** OpenAI or Anthropic, to confirm the snapshot
shape generalizes before it is treated as settled.

## References

- [Issue 1069] — the report this addresses
- [RFD 020] — parallel conversations, the source of the contention
- [RFD 045] — the interrupt handler stack the wait plugs into
- [`acquire_lock`] — the wait-then-prompt pattern this mirrors
- [Cerebras rate limits] — dual-bucket model and token-bucket replenishment

[Cerebras rate limits]: https://inference-docs.cerebras.ai/support/rate-limits
[Issue 1069]: https://github.com/dcdpr/jp/issues/1069
[RFD 020]: ../020-parallel-conversations.md
[RFD 045]: ../045-layered-interrupt-handler-stack.md
[`acquire_lock`]: https://github.com/dcdpr/jp/blob/main/crates/jp_cli/src/cmd/lock.rs
[`jp_workspace::user_data_dir`]: https://github.com/dcdpr/jp/blob/main/crates/jp_workspace/src/lib.rs
