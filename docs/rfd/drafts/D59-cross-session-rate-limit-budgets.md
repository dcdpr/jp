# RFD D59: Cross-Session Rate-Limit Budgets

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-09-04
- **Summary**: Shares provider-reported rate-limit budgets across concurrent
  `jp` sessions in a credential-keyed local store so sessions wait before
  overdrawing.

## Summary

Providers meter usage per account, but each `jp` process retries in isolation,
so concurrent sessions sharing an API key overdraw the same bucket and then
retry into it together.
This RFD records the provider-reported remaining budget in a user-local store
keyed by credential, and makes sessions wait before sending when that budget is
nearly exhausted.
The budget is **observed, never predicted**: JP stores what the provider
reported, not an estimate of what a request will cost.

## Motivation

[Issue 1069] asks for per-minute throttling across all sessions against
Cerebras.
Cerebras enforces its limits per *organization*, and `jp` runs one process per
conversation, so parallel sessions ([RFD 020]) sharing an API key contend for a
bucket none of them can see.

The retry loop is purely reactive: a session sends the request, receives HTTP
429, and stalls for the `retry-after` delay returned by the provider.
With `max_retries` defaulting to 5 and Cerebras returning `retry-after: 60`, a
session can wait five minutes and still fail.

### Why a client-side ledger cannot work

An obvious design is a shared client-side ledger that estimates request cost,
decrements a local counter, and refills it at the limit rate.
Two independent problems rule this out.

#### Unobserved consumers share the account

Providers enforce rate limits against an account, not a single client process.
Other API keys on the same account, a colleague's machine, a `curl` command, or
a background script all draw from the same quota.
Even an exact record of JP's own usage misses an unbounded amount of external
traffic.
This alone rules out a local ledger, regardless of how well a provider documents
its token accounting.

#### Cerebras's charging formula is undocumented

Even for a single client, predicting request cost requires knowing how the
provider charges.
Cerebras reserves tokens at request admission, before generating the response.
Six test requests with a 68-token prompt reserved `min(max_completion_tokens,
16384)` tokens, with zero charge for the prompt itself:

| `max_completion_tokens` | remaining after | reserved |
| ----------------------- | --------------- | -------- |
| 1                       | 499,999         | 1        |
| 512                     | 499,488         | 512      |
| 4,096                   | 495,904         | 4,096    |
| 16,384                  | 483,616         | 16,384   |
| 40,960                  | 483,616         | 16,384   |
| unset                   | 483,616         | 16,384   |

The same request with a ~64,000-token prompt reserved 80,384 tokens (`64,000 +
16,384`), so prompt tokens count at 64,000 tokens but not at 68.
Sending the same large prompt twice charged *more* on the second request than on
the first, ruling out prompt caching.

Whether Cerebras uses coarse-grained input estimation or another internal
heuristic, the formula cannot be derived from its documentation or encoded
reliably.
Other providers document their token accounting clearly, but external account
traffic still rules out a local ledger for them.

#### Read the reported budget instead

Cerebras reports the account's remaining budget in the headers of every
successful response:

```
x-ratelimit-limit-tokens-minute: 500000
x-ratelimit-remaining-tokens-minute: 419616
```

Reading these headers reflects the true account state regardless of the
provider's charging formula or other clients on the account.

## Design

### User experience

When the remaining budget sits below the floor, a session waits before sending.
The wait follows the same progression as conversation-lock contention
([`acquire_lock`]): a brief silent delay, a live timer line, and an interactive
prompt if the timeout expires.

```
⏳ Waiting for rate-limit headroom (cerebras, tokens/minute) — 12s
```

```
? Rate limit nearly exhausted for cerebras (tokens/minute: ~8,200 of 500,000).
> Continue waiting
  Send anyway
  Cancel
```

Pressing Ctrl-C during the wait jumps straight to the prompt rather than
aborting the turn, matching lock-wait behavior.
Non-interactive runs wait up to `timeout_secs` and then send the request anyway.
Unlike a lock conflict, a rate limit clears on its own: if the request still
hits a 429, the existing retry layer handles it, whereas failing the command
outright would regress current behavior.

Configuration mirrors the lock-wait settings, splitting behavior under
`[rate_limit]` and display under `[style.rate_limit_wait]`:

```toml
[rate_limit]
enable = true          # set false to restore today's send-and-retry behaviour

[style.rate_limit_wait]
show = true
delay_secs = 1
interval_ms = 100
timeout_secs = 10      # then prompt
```

### Rate-limit snapshots

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

`Recovery` is an enum rather than a single refill rate because providers report
replenishment differently.
Anthropic returns the timestamp when a bucket resets to full, Cerebras documents
a continuous refill rate without returning a reset header, and fixed-window
providers report neither.
Applying the wrong replenishment model to a provider invents headroom that does
not exist.

Providers extract zero or more snapshots from a response head:

```rust
trait Provider {
    /// Rate-limit buckets this provider reported in a response head.
    fn snapshots_from_headers(&self, _headers: &HeaderMap) -> Vec<RateLimitSnapshot> {
        vec![]
    }
}
```

The default implementation returns an empty vector, disabling pre-request
throttling for providers that do not report rate-limit headers.

Providers supply different sets of headers:

- **Cerebras** reports six buckets (tokens and requests across minute, hour, and
  day windows) with `limit` and `remaining` headers but no reset timestamp.
  Its adapter computes `Recovery::Continuous` as `limit / window`.
- **Anthropic** returns
  `anthropic-ratelimit-{requests,tokens,input-tokens,output-tokens}-{limit,remaining,reset}`
  on every Messages API response.
  Because `reset` is an RFC 3339 timestamp, its adapter constructs
  `Recovery::FullAt` directly from the header.
- **OpenAI** returns `x-ratelimit-reset-requests` and `x-ratelimit-reset-tokens`
  (already parsed by `extract_retry_after`), though whether it also reports
  remaining quota has not been verified.

Providers and buckets remain orthogonal: supporting a new provider requires only
a header adapter, and adding a new bucket unit or window requires no changes to
existing providers.

**Snapshots are recorded from the response head, not at stream completion.**
Because HTTP headers arrive before the response body, failed or interrupted
streams still record a snapshot, preserving budget updates when traffic is
heaviest.

### Ceilings, readings, and other sources

A snapshot combines two values that age at different rates.
The **ceiling** (`limit`) changes only when an account's plan changes, whereas
the **reading** (`remaining`) goes stale within seconds.
Both arrive together in response headers, so `RateLimitSnapshot` carries both,
but `limit` is optional so providers that report only `remaining` can still
participate.

Anthropic's [Rate Limits API] exposes configured limits for an organization and
its workspaces, but it cannot drive throttling:

- It returns `{type, value}` pairs for the **configured limit** only, not the
  remaining quota.
  Anthropic documents it as a reference to compare against a separate usage API.
- It requires **Admin API credentials** (an admin key, an OAuth token with
  `org:admin`, or an unscoped account key).
  Standard workspace inference keys are rejected, and individual accounts cannot
  use the endpoint at all.

For Anthropic, the endpoint only duplicates the ceiling already present in
`anthropic-ratelimit-*-limit` while requiring a privileged credential most users
do not have.
**This design does not poll any rate-limit endpoint**, and `Provider` defines no
method for one.
`snapshots_from_headers` names its source explicitly so that if a future
provider reports remaining quota via an HTTP endpoint, `snapshots_from_query`
can be added alongside it without changing `RateLimitSnapshot`.

### Budget store

The store keeps one file per credential under the user data directory
([`jp_workspace::user_data_dir`]), holding the latest snapshot for each `(unit,
window)` bucket:

```
$JP_USER_DATA_DIR/rate-limits/cerebras-a3f19c2e.json
```

The store is keyed by an opaque **credential ID** supplied by the CLI shell.
When a provider authenticates with a single API key, the ID is a truncated
SHA-256 hash of the key, which distinguishes keys without storing the secret and
starts a fresh budget whenever a key rotates.
Under [RFD 090], which introduces named credential profiles (including OAuth
tokens without a static API key), the caller passes the provider and profile
name as the credential ID without changing the store format.

Keying by credential rather than by provider also supports [RFD 090]'s
credential chains: when a session falls back to a secondary credential mid-turn,
it automatically switches to that credential's rate-limit file.

The credential ID is never logged.
Writes use advisory locks from `jp_storage::lock` and atomic file replacement.
**Any failure to read, parse, or lock the store degrades to an unknown budget
and sends the request immediately**, ensuring store errors never block the CLI.

### Pre-request decision

Before sending a provider request, a session loads the credential's snapshots,
projects each bucket's remaining quota to the current time, and waits while any
bucket sits below its floor.

Projection follows each snapshot's `Recovery` variant:

- `FullAt` interpolates toward the stated reset timestamp.
- `Continuous` adds `per_second * elapsed`, capped at `limit`.
- `Unknown` leaves `remaining` unchanged rather than inventing replenishment.

Because JP cannot predict what the next request will cost, the floor is **the
largest single drop observed recently**.
The difference between two consecutive snapshots, adjusted for replenishment
over the interval, measures the actual drain from both JP and any external
clients.
The store retains the last few drops per bucket and uses their maximum as the
floor.

When the store has no prior snapshots, the floor is zero and JP sends
immediately.

### Crate boundaries

`jp_llm` defines `RateLimitSnapshot`, `Recovery`, `BucketId`, and
`Provider::snapshots_from_headers`.
Provider adapters parse HTTP headers into snapshots without knowing which
credential authenticated the request.

The CLI (`jp_cli`) owns the credential ID, the store, the floor calculation, and
the wait prompt.
This boundary matches [RFD 090], which keeps credential identity out of
`jp_llm`, and keeps terminal prompting in the CLI layer.

Background requests sent through `collect_with_retry` (title generation,
summarization, and inquiries) consume the same quota and share the wait logic,
but wait silently up to the timeout without prompting.

## Drawbacks

**A new blocking state in `jp query`.** Waiting before a request introduces a
new interactive state to interrupt, explain, and test.
Most of the implementation complexity lies in this wait state rather than in the
budget accounting.

**Cross-process state coupling.** Whether a session waits depends on a store
file that other `jp` processes write concurrently.
Degrading to immediate send on store errors bounds the failure mode, but the
cross-process coupling is new.

**Heuristic floor estimation.** If the floor is too low, JP still hits HTTP 429
responses.
If the floor is too high, JP throttles requests that the provider would have
accepted, stalling the session without an external error.

**Per-provider header maintenance.** Each provider uses its own rate-limit
header names and semantics, requiring hand-maintained adapters that can drift as
provider APIs change.

## Alternatives

**Predict request cost with a local ledger.** Rejected for the reasons in
[Motivation](#motivation): provider charging formulas can be opaque, and a local
ledger cannot observe other clients sharing the account.

**Sum recorded token usage across conversations.** A companion design records
per-request token usage in conversation streams.
Summing those events measures what JP spent in the past, not what quota remains
on the account.
It also requires scanning every conversation stream to evaluate a 60-second
rate-limit window, and reads usage from the completed response body rather than
the initial response headers.

**Serialize provider requests across sessions with a lock.** Prevents concurrent
overdraw, but sacrifices the parallel session workflow introduced in [RFD 020].

**Rely solely on HTTP 429 retries.** The existing retry layer honors
`Retry-After` and jitters concurrent sessions ([PR 1080]) so they do not retry
in lockstep, and it remains the fallback when pre-request throttling misses.
However, reactive retries cost an extra round trip on every overdraw, and on
smaller tiers the per-minute bucket stays exhausted for most of each minute.

## Non-Goals

**Anticipating traffic from external clients.** Each response snapshot already
reflects prior usage by other machines, keys, or scripts on the same account,
but JP cannot predict external bursts between snapshots.
An external spike between two requests still triggers an HTTP 429.

**Sharing state across machines.** The budget store is user-local.
Two developers sharing an API key on different machines maintain separate stores
and fall back to HTTP 429 retries when their combined traffic exceeds the limit.

**Grouping distinct credentials that share an account.** Cerebras enforces
limits per organization, so two API keys in the same organization draw from one
bucket even though JP tracks them under separate credential IDs.
Because response headers do not expose an account or organization ID, JP keys
the store by credential.

**Replacing the HTTP 429 retry layer.** Pre-request throttling is additive; the
existing 429 retry loop remains as the fallback.

**Recording token usage or cost.** Historical usage tracking belongs in the
conversation event stream, not the rate-limit store.

## Risks and Open Questions

**Accessing response headers on streaming requests.**
`reqwest_eventsource::EventSource` takes ownership of the request and its
`Event::Open` variant does not expose HTTP headers, whereas error responses
expose them via `InvalidStatusCode`.
Streaming providers that implement `snapshots_from_headers` may need to replace
`reqwest-eventsource` with `reqwest` plus `eventsource-stream` (which
`reqwest-eventsource` wraps, and which is already a dev-dependency of `jp_llm`).
**Phase 1 spikes this before anything else**, because it determines whether the
change stays localized or touches every streaming provider.

**Tuning the floor heuristic.** Using the largest recent drop avoids
provider-specific formulas, but it has not been tested against real
multi-session traffic.
Shipping behind `rate_limit.enable` allows tuning the heuristic from observed
logs.

**Handling long-window buckets.** Cerebras reports minute, hour, and day
buckets.
Exhausting a day bucket cannot be resolved by a brief pre-request wait, so the
decision logic needs a maximum window threshold above which buckets are ignored
or reported differently.

**Updating the store on HTTP 429.** Cerebras omits `x-ratelimit-*` headers on
429 responses, so the response that confirms exhaustion carries no snapshot.
Recording a synthetic zero-remaining entry from the 429 status code is possible,
but requires knowing which bucket was exhausted.

**Disambiguating dynamic bucket scopes.** Anthropic's
`anthropic-ratelimit-tokens-*` headers report whichever limit is currently most
restrictive, switching between organization and workspace limits across
responses.
Keying the store by header name alone would compare snapshots from different
buckets.
The `anthropic-workspace-id` response header identifies the active workspace and
can disambiguate `BucketId` in the Anthropic adapter.

**Rounded `remaining` values.** Anthropic rounds `*-remaining` headers to the
nearest thousand.
Computing drops from rounded readings introduces quantization error that is
largest relative to the budget when `remaining` is near zero.
The floor calculation may need to account for the provider's rounding
granularity.

**Overlap with [RFD 090]'s `exhausted_until` store.** [RFD 090] persists an
`exhausted_until` timestamp per credential profile in the user data directory so
new invocations skip credentials that hit a billing quota error.
Both features persist per-credential availability state in the user data
directory, though they track different conditions (`InsufficientQuota` cooldowns
versus transient `RateLimit` headroom).
Whichever feature is implemented second should decide whether the two records
share a file.

## Implementation Plan

**Phase 1: Spike response-head access.** Determine whether streaming providers
can read HTTP response headers without replacing `reqwest_eventsource`.
The output is an architectural decision that gates the remaining phases.

**Phase 2: Snapshot types and Cerebras adapter.** Add `RateLimitSnapshot`,
`BucketId`, `Recovery`, `Provider::snapshots_from_headers`, and the Cerebras
header parser with unit tests against captured headers.
The Cerebras adapter computes `Recovery::Continuous` from `limit / window`.
This phase can merge independently before any caller consumes snapshots.

**Phase 3: Budget store.** Implement store loading, atomic writes, advisory
locking, credential ID derivation, and fallback to immediate send on any read,
parse, or lock error.
Include unit tests for corrupt files, missing directories, and contended locks.

**Phase 4: Shadow decision logging.** Project snapshots forward, compute the
recent-drop floor, and log when a request would have waited without blocking.
Shipping the calculation in shadow mode validates how often the floor triggers
before introducing user-visible waits.

**Phase 5: Pre-request wait and prompt.** Add the blocking wait state, live
timer line, Ctrl-C escalation to the prompt, non-interactive timeout fallback,
and silent bounded waits for `collect_with_retry` callers.

**Phase 6: Anthropic adapter.** Implement `snapshots_from_headers` for
Anthropic, exercising `Recovery::FullAt`, rounded `remaining` values, and
dynamic organization-versus-workspace bucket scopes.

## References

- [Issue 1069]: per-minute rate-limit throttling across sessions for Cerebras
- [RFD 020]: parallel conversations sharing provider credentials
- [RFD 045]: layered interrupt handler stack used by the wait prompt
- [RFD 090]: credential profiles and provider-boundary rules
- [`acquire_lock`]: conversation-lock wait and prompt progression
- [Cerebras rate limits]: dual-bucket model and continuous token replenishment
- [Anthropic rate limits]: per-response rate-limit headers and reset timestamps
- [Rate Limits API]: Anthropic's admin endpoint for configured organization
  limits

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
