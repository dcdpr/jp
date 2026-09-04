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

### Why a client-side ledger cannot work

The obvious design is a shared ledger: estimate what a request will cost,
decrement a counter, refill it at the limit rate.
Two things rule it out, and they have different reach.

#### Every provider: the meter counts consumers JP cannot see

A rate limit is enforced against an account, not against a process.
Another key on the same account, a colleague's machine, a `curl` in someone's
terminal, a script nobody remembers writing.
Even a perfect record of what JP itself spent would be missing an unbounded
amount of the total, so a ledger built from JP's own arithmetic is wrong by an
amount it cannot measure.

This holds however well a provider documents its charging, and it alone is
enough to reject the ledger.

#### Cerebras specifically: the charging rule is not derivable

The second argument is narrower, and is offered as corroboration rather than as
the basis for the design.
It is what the measurements below actually establish, and it establishes it for
one provider.

Cerebras takes its reservation at admission, before the response exists.
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
It is not derivable from Cerebras's documented behaviour, and it is not stable
enough to encode.
Whether any other provider is this opaque is unknown; several document their
accounting clearly, and for those the first argument is the only one that
applies.

#### What both point at

Stop computing, start reading.
A provider that meters an account has to tell its clients where they stand, and
Cerebras returns the answer on every successful response:

```
x-ratelimit-limit-tokens-minute: 500000
x-ratelimit-remaining-tokens-minute: 419616
```

Reading it is correct whether or not the charging rule is knowable, and correct
whether or not JP is the only client.
That is the property worth designing around.

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
    pub bucket: BucketId,          // provider-labelled: unit plus scope
    pub limit: Option<u64>,
    pub remaining: u64,
    pub recovery: Recovery,
    pub observed_at: DateTime<Utc>,
}

/// How a bucket returns to full, as the provider describes it.
pub enum Recovery {
    /// Fully replenished at this instant.
    FullAt(DateTime<Utc>),

    /// Refills continuously at this many units per second, capped at `limit`.
    Continuous { per_second: f64 },

    /// Not reported. The reading is only good for the instant it was taken.
    Unknown,
}
```

`Recovery` is an enum rather than a rate because providers describe
replenishment differently, and the difference matters: Anthropic states the
absolute instant a bucket is whole again, Cerebras documents a continuous refill
and states nothing per response, and a provider that resets on a fixed boundary
does neither.
Assuming any one of these on a provider that means another invents headroom that
does not exist.

Providers extract zero or more snapshots from a response head:

```rust
trait Provider {
    /// Rate-limit buckets this provider reported in a response head.
    fn snapshots_from_headers(&self, _headers: &HeaderMap) -> Vec<RateLimitSnapshot> {
        vec![]
    }
}
```

The default returns nothing, so a provider that reports nothing simply gets no
throttling.

The method is named for its source because a response head is not the only one a
provider could have.
See [Ceilings, readings and other sources](#ceilings-readings-and-other-sources)
below for what else exists and why none of it is built here.

Two providers are known to supply the raw material, and they supply different
amounts of it.
Cerebras yields six snapshots (tokens and requests, across minute, hour and day)
with a limit and a remaining each and no reset of any kind, so its recovery is
`Continuous` derived from `limit / window`.
Anthropic documents `anthropic-ratelimit-{requests,tokens,input-tokens,output-
tokens}-{limit,remaining,reset}` on every Messages API response, where `reset`
is an RFC 3339 instant, so its recovery is `FullAt` straight from the header and
needs no arithmetic at all.
OpenAI has in-tree evidence but only half of it: `extract_retry_after` already
parses `x-ratelimit-reset-requests` and `x-ratelimit-reset-tokens` for retry
timing, and whether it also reports remaining has not been checked.

This keeps providers and buckets orthogonal: a new provider is one adapter, a
new bucket kind touches no provider.

**Snapshots are taken from the response head, not at stream completion.**
Headers arrive before the first byte of body, so a stream that fails or is
interrupted still yields its observation — and those are exactly the
observations taken during the trouble that makes throttling worth having.

### Ceilings, readings and other sources

A snapshot bundles two facts with very different half-lives.
The **ceiling** (`limit`) changes when a plan changes, so a stale one is
harmless.
The **reading** (`remaining`) is stale within seconds.
They arrive together in a response head today, which is why one struct carries
both, but nothing in the design requires that they always do: `limit` is an
`Option` precisely so a provider that reports remaining without a ceiling can
still participate.

Some providers expose the ceiling separately.
Anthropic has a [Rate Limits API] that lists the configured limits for an
organization and its workspaces.
It is worth being precise about what that endpoint does and does not offer,
because it is easy to read as a way to ask how much room is left:

- It returns `{type, value}` pairs where `value` is the **configured limit**.
  There is no remaining.
  Anthropic's own documentation frames it as something to compare *against*
  usage data from a separate API, not as a source of usage.
- It requires **Admin API credentials**: an admin key, an OAuth token with
  `org:admin`, or an unscoped account key.
  The workspace-scoped key JP would use for inference does not work, and the
  endpoint is unavailable to individual accounts entirely.

So for Anthropic the endpoint would supply a number JP already gets for free
from `anthropic-ratelimit-*-limit`, at the cost of a second privileged
credential most users do not have.
**No active source is built here**, and the trait carries no method for one.

The axis is named rather than paved.
A provider could plausibly report remaining from a queried endpoint rather than
a response header, and if one does, adding `snapshots_from_query` beside
`snapshots_from_headers` costs nothing that the current shape has spent.
Building that plumbing now, for a case no provider presents, would be paying for
a second source before there is one.

### The store

One file per credential under the user data directory
([`jp_workspace::user_data_dir`]), holding the most recent snapshot per `(unit,
window)`:

```
$JP_USER_DATA_DIR/rate-limits/cerebras-a3f19c2e.json
```

The store is keyed by an opaque **credential id** that the shell supplies.
It never learns how that id was derived, and deliberately so.
Today a provider authenticates with a single API key, so the id is a truncated
SHA-256 of it: enough to tell two keys apart without storing anything
reversible, and key rotation starts a fresh budget for free.
[RFD 090] replaces that with a chain of named credential profiles, some of them
OAuth tokens with no API key to hash, at which point the id becomes the provider
and profile name that RFD already established as a credential's identity.
The store does not change when that happens; only what fills the id does.

A credential chain means a session can move between credentials mid-turn when
one is exhausted, and each has its own meter.
Switching credential switches bucket, which falls out of keying on the id rather
than on the provider.

The id is never logged.
Writes go through the advisory locking in `jp_storage::lock` and land
atomically.
**Any failure to read, parse, or lock the store degrades to "no budget known",
which sends.** A throttle that can stall the CLI is worse than no throttle.

### The decision

Before a provider request, a session loads the snapshots, projects each forward
to now, and waits while any projected remaining sits below a floor.

Projection follows the snapshot's `Recovery`, which the adapter filled in from
what the provider actually said.
`FullAt` interpolates towards the stated instant, `Continuous` adds `per_second`
capped at `limit`, and `Unknown` projects nothing: the reading stands as taken
and ages into uselessness rather than into invented headroom.

That the shared logic never assumes a recovery model is the point.
Anthropic hands over a reset instant and Cerebras hands over nothing, and a
single rate baked into the decision would be wrong for one of them.

The floor cannot be a prediction of what this request will cost, for the reasons
in the Motivation.
It is instead **the largest single drop observed recently**: the difference
between consecutive snapshots, corrected for refill, is the measured drain of
real traffic including consumers JP cannot see.
The store keeps the last few drops per bucket and uses their maximum.

With no history the floor is zero and JP sends, so a first run is never worse
than today.

### Where the pieces live

`jp_llm` owns the snapshot type and the per-provider extraction.
Neither needs to know whose credential produced the response: an adapter reads
headers and returns numbers.

The shell owns the credential id, the store, the decision and the wait.
That split is not a preference.
[RFD 090] rules that credential identity never crosses the provider boundary,
and a credential-keyed store inside `jp_llm` would break that rule the moment
that RFD lands.
Prompting is a terminal concern besides.

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

**Do nothing beyond the retry layer.** Reacting to the 429 is what this design
falls back to, and that fallback is sound on its own terms: the wait honours the
provider's `Retry-After`, and [PR 1080] spreads concurrent sessions so they do
not all resume on the same instant.
The case against leaving it there is that reacting costs a full round trip each
time, and on a small plan the bucket is exhausted for most of every minute.

## Non-Goals

**Anticipating consumers JP cannot see.** Another machine, another key, a
script.
Every reading already *includes* their effect, since the provider meters the
account rather than the process — that is the whole reason for reading rather
than computing.
What JP cannot do is see them coming, so a burst from elsewhere between two
observations will still produce a 429.

**Organizations sharing one key across machines.** The store is user-local by
design.
Two developers on one key each keep their own view and each believe they have
the full budget.
The failure mode is graceful: they learn from the 429, which is today's
behaviour.

**Multiple credentials on one account.** Cerebras meters per organization, so
two keys on one org share a meter while this design gives each its own budget.
The credential is the best identity JP has, not the true one, and nothing a
client can observe distinguishes the two cases.

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

**A bucket's identity can shift under its own name.** Anthropic's
`anthropic-ratelimit-tokens-*` headers report whichever limit is currently most
restrictive, so the same header names describe an organization limit on one
response and a workspace limit on the next.
A store keyed by header name would silently compare two different buckets and
conclude the budget had jumped.
The `anthropic-workspace-id` response header names the scope a request counted
against, which is probably the discriminator `BucketId` needs, and confirming
that is part of the Anthropic adapter rather than a blocker for the design.

**Readings can be deliberately imprecise.** Anthropic rounds every `*-remaining`
to the nearest thousand.
A floor computed from differences between rounded numbers inherits that error,
which matters most when the remaining is small, which is exactly when the
decision is being made.
The floor may need to carry the reported precision rather than assume exactness.

**Two records about the same credential.** [RFD 090] gives each credential
profile an `exhausted_until` timestamp, persisted in the user data directory, so
a fresh invocation skips a credential whose quota has not reset.
That is a coarse form of what this design stores: both say when a credential is
usable again, keyed the same way, in the same place.
They answer different questions — one is a hard cooldown after a billing quota
error, the other a soft headroom estimate from a refilling bucket — and
collapsing them would conflate the fatal case with the transient one, which
`InsufficientQuota` and `RateLimit` are deliberately kept apart elsewhere.
But two stores about one credential is a smell, and whoever builds the second of
the two should decide whether they share a file.
Neither exists yet, so this is not blocking.

## Implementation Plan

**Phase 1 — Spike: response-head access.** Determine whether a streaming
provider can read response headers without abandoning `reqwest_eventsource`.
Output is a decision, not necessarily code.
Everything else depends on it.

**Phase 2 — Snapshot type and Cerebras extraction.** `RateLimitSnapshot`,
`BucketId`, `Recovery`, the defaulted trait method, and the Cerebras adapter
with unit tests against captured header sets.
Cerebras fills `Recovery::Continuous` from `limit / window`, so the token-bucket
arithmetic lands in the adapter rather than in the shared decision.
Mergeable alone; nothing consumes it yet.

**Phase 3 — The store.** Load, atomic write, advisory locking, the credential
id, and the degrade-to-send rule on every failure path.
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

**Phase 6 — A second provider.** Anthropic, whose documented headers exercise
the parts of the shape Cerebras does not: an explicit reset instant rather than
a derived rate, a ceiling and a reading that need not travel together, rounded
remainings, and a bucket whose scope can change between responses.
If the shape survives that it is worth treating as settled.

## References

- [Issue 1069] — the report this addresses
- [RFD 020] — parallel conversations, the source of the contention
- [RFD 045] — the interrupt handler stack the wait plugs into
- [RFD 090] — credential profiles and the boundary this design keys against
- [`acquire_lock`] — the wait-then-prompt pattern this mirrors
- [Cerebras rate limits] — dual-bucket model and token-bucket replenishment
- [Anthropic rate limits] — per-response headers, including the reset instant
- [Rate Limits API] — Anthropic's admin endpoint for configured ceilings

[Anthropic rate limits]: https://platform.claude.com/docs/en/api/rate-limits
[Cerebras rate limits]: https://inference-docs.cerebras.ai/support/rate-limits
[Issue 1069]: https://github.com/dcdpr/jp/issues/1069
[PR 1080]: https://github.com/dcdpr/jp/pull/1080
[RFD 020]: ../020-parallel-conversations.md
[RFD 045]: ../045-layered-interrupt-handler-stack.md
[RFD 090]: ../090-anthropic-subscription-auth-with-credential-fallback.md
[Rate Limits API]: https://platform.claude.com/docs/en/manage-claude/rate-limits-api
[`acquire_lock`]: https://github.com/dcdpr/jp/blob/main/crates/jp_cli/src/cmd/lock.rs
[`jp_workspace::user_data_dir`]: https://github.com/dcdpr/jp/blob/main/crates/jp_workspace/src/lib.rs
