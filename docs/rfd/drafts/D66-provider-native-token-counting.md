# RFD D66: Provider-Native Token Counting

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-09-09

## Summary

Add `count_input_tokens` to the `Provider` trait, backed by each provider's own
counting endpoint where one exists.
Use it to answer the question a user hits when summarizing a long range: not
*does this fit* (the API already answers that), but *what range would*.

## Motivation

Compacting a long conversation with `--summary` requires picking a turn range
that fits the summarizer's context window.
Nothing tells the user what that range is, so they guess.
A real session:

| Attempt | Range  | Outcome                           |
| ------- | ------ | --------------------------------- |
| 1       | 2..150 | too large                         |
| 2       | 2..100 | too large                         |
| 3       | 2..70  | too large                         |
| 4       | 2..50  | API: 1,318,026 tokens > 1,000,000 |
| 5       | 2..40  | API: 1,192,348 tokens > 1,000,000 |
| 6       | 2..35  | API: 1,146,033 tokens > 1,000,000 |
| 7       | 2..30  | API: 1,054,336 tokens > 1,000,000 |
| 8       | 2..25  | summarized                        |

Eight commands over seven minutes, each uploading a multi-megabyte request only
to be told the number was still too big.
That is a binary search, executed by hand, with the most expensive possible
probe at each step.

The user already knows *how* to narrow the range.
What they lack is the one number that would let them do it in a single step, and
that number is exactly what a provider counting endpoint returns.

Doing nothing leaves the search manual and leaves every future consumer
estimating.
Two other drafts, `D24` (bounded tool output) and `D50` (automatic compaction),
each defer accurate counting for the same stated reason — a tokenizer is a real
dependency — and each works around its absence with a safety margin.

## Design

### What the user sees

A range that does not fit is reported with the provider's numbers and a range
that does:

```
Summarization failed

      model  anthropic/claude-opus-5
     reason  api error: invalid_request_error: prompt is too long: 1318026
             tokens > 1000000 maximum
 suggestion  Turns 2..25 fit, at 984210 tokens. Re-run with `--turn=2..25`, or
             summarize with a larger-window model.
```

The command reports and exits non-zero.
It does not narrow the range and proceed: a summary silently standing in for a
different span than the one asked for is the kind of substitution that is
expensive to notice later, and the report already removes the seven minutes.

When the provider cannot count, or the count call fails, the failure is reported
without the suggestion line.
Bisecting on the character heuristic would present a guess as a fact, which is
the failure this RFD exists to remove.

### Counting is a failure-path cost

The suggestion is computed only after the API rejects the request, never before
it.

This follows from what a count call costs: it ships the same payload as the
request it would precede.
Counting before every summarize would save nothing when the range is too large
— the same bytes go up either way — and would add a full extra upload every
time the range fits, which is the common case.
Measured: 4,220,150 bytes counted in 0.63 seconds.
A ~7-step bisection over a 150-turn range is a few seconds, against a manual
search that took minutes.

### The trait method

```rust
/// Count the tokens `query` consumes as request input.
///
/// `None` means this provider has no way to count with the model's own
/// tokenizer. Callers needing a number regardless fall back to
/// `window::estimate_tokens`, which is a character heuristic.
async fn count_input_tokens(
    &self,
    model: &ModelDetails,
    query: &ChatQuery,
) -> Result<Option<u32>> {
    Ok(None)
}
```

Three outcomes, each with a distinct caller response:

| Result        | Meaning                            | Caller                    |
| ------------- | ---------------------------------- | ------------------------- |
| `Ok(Some(n))` | counted with the model's tokenizer | use it                    |
| `Ok(None)`    | provider has no counting endpoint  | skip the suggestion       |
| `Err(_)`      | counting was attempted and failed  | warn, skip the suggestion |

The default implementation returns `Ok(None)` rather than an estimate.
A provider that cannot count says so instead of handing back a weaker number
that reads identically to a real one.

`&ChatQuery` rather than `ChatQuery`: the caller keeps the query to send it.
Providers clone internally, which a network round trip dominates.

### Building the count body

Each provider derives its own count body from the `ChatQuery`.
For Anthropic this is a **top-level allowlist** over `create_request`'s output,
not a strip of known-bad fields.
The endpoint accepts `messages`, `model`, `system`, `thinking`, `tools`,
`tool_choice`, and `mcp_servers`, and rejects anything else outright:

```
{"type":"error","error":{"type":"invalid_request_error",
 "message":"max_tokens: Extra inputs are not permitted"}}
```

`create_request` can emit eight fields outside that set — `stream`,
`max_tokens`, `cache_control`, `service_tier`, `output_config`, `temperature`,
`top_p`, `top_k` — and five are conditional on user config.
A denylist tested against one request body passes review and then 400s for the
first user with `temperature` set.
An allowlist fails safe: a newly added request parameter is simply not copied.

The filter applies to top-level keys only.
Block-level `cache_control` nested inside `messages`, `system`, and `tools` is
accepted and ignored at count time, and must survive the projection.

Two consequences worth stating plainly.
The counted body is not byte-identical to the sent body, so this design does not
claim to count exactly what goes on the wire; sampling parameters do not affect
input token count, so the number is still right.
And the count is itself an estimate — Anthropic documents that the real request
may differ by a small amount, and includes tokens added for its own system
optimizations.

### Unit alignment

`jp_llm::window` measures in characters: `estimate_chars`, `budget_chars`,
`estimate_overhead_chars`, and a `truncate_to_fit` taking `overhead_chars`.
No signature in the module can accept a token count.

Convert the module to tokens, with `CHARS_PER_TOKEN` becoming an implementation
detail of the estimator.
The arithmetic is unchanged apart from one division, so behavior is preserved.
Afterwards, where a number came from is a value on one axis and the budgeting
and selection logic does not care which source produced it.

## Drawbacks

- **A provider round trip on a failure path.** A rejected summarize now costs
  the rejected request plus a handful of count calls.
  Free at Anthropic (token counting is not billed and has its own rate limit),
  but it is still latency added to an already-failed command.
- **Uneven coverage.** Anthropic, Google, and llama.cpp can count.
  OpenAI, Cerebras, OpenRouter, and Ollama cannot, so their users keep guessing.
  The design makes the gap visible rather than papering over it, which is honest
  but not helpful to those users.
- **A second way to be wrong about size.** `truncate_to_fit` still estimates
  while the summarizer counts, so two paths answer "how big is this?"
  differently.
  Justified because they make different decisions — one picks a cut point, the
  other reports a number — but it is a real inconsistency.
- **Per-provider projection logic.** Each counting implementation carries a body
  projection that has to track its provider's accepted schema.
  That is maintenance the character heuristic did not need.

## Alternatives

### Keep a pre-flight gate, but make it accurate

Rejected, and the existing gate was removed before this RFD was written.
A count call ships the same payload as the request, so a gate saves nothing on
the failure path and costs an extra upload on every success.
The provider's own rejection is more precise than anything measured locally.

### A `TokenCount { Native, Approximate }` enum

An earlier shape tagged the count with its provenance so callers could decide
how much to trust it.
Rejected: carrying a second, weaker number alongside the real one invites
exactly the misuse this RFD removes — a heuristic presented in the same shape
as a measurement.
`Option<u32>` says "I cannot count" without offering a consolation prize.
Provenance still reaches the logs as a `debug!` field.

### A local tokenizer library

`tiktoken-rs` and `bpe-openai` are well-maintained but carry OpenAI vocabularies
only.
Anthropic and Google publish none, so `cl100k` against Claude is a stand-in, and
Claude 4.7+ counts roughly 30% higher than earlier models under its own
tokenizer.
Local models are the inverse: the GGUF holds the real tokenizer and llama.cpp
serves it over `/tokenize`, which no bundled vocabulary can match.
A local tokenizer can slot behind this trait method later if a provider without
an endpoint earns it.

### Calibrate the estimator from one count

Take one count of a stream, divide by its character estimate, and use the
resulting ratio to apportion per-event costs — one call, per-event granularity.
Attractive for `truncate_to_fit`, which needs a cut point rather than a total.
Deferred: `truncate_to_fit` makes no hard-failing decision, its 80% target
factor already absorbs the error, and nobody has reported a problem with it.

### Search by sending the real request

The manual bisection the user already performs, automated.
Rejected: the same number of round trips, each carrying a real inference request
instead of a free count.

## Non-Goals

- **Capturing `usage` from responses.** Every provider reports actual input
  tokens after a request, and JP discards all of it.
  That is the only exact count available and it is free, but it is
  retrospective, so it answers "how full is the window now?" rather than "would
  this fit?".
  Different mechanism, different consumers, separate RFD.
- **Counting on the main turn path.** Nothing on the query path reads a
  prospective count today.
  Adding one before a consumer exists is a speculative axis.
- **Caching counts.** Counting is free, rate-limited independently, and called
  once per failed command.
- **Auto-narrowing the range.** Report only; see above.
- **Token-accurate `truncate_to_fit`.** Title generation and inquiries keep the
  character estimate.

## Risks and Open Questions

- **The suggested range can still be rejected.** The count is an estimate on the
  provider's own terms, and the counted body omits fields the real request
  carries.
  A suggestion landing within a percent of the window may fail when sent.
  Mitigation: suggest the largest range that fits with headroom, reusing the
  existing target factor, rather than the largest that merely fits.
- **Bisection assumes monotonicity.** A larger range costs more tokens than a
  smaller one with the same start.
  This holds for ranges sharing a left bound, which is what the search varies.
  It would not hold for a search that moved both bounds.
- **A single turn may exceed the window.** Then no range fits and the search has
  no answer to report.
  It must terminate and say so rather than returning the empty range.
- **Google's count body needs validation.** `countTokens` accepts bare
  `contents`, and a wrapped `generateContentRequest` when system instructions
  and tools must count.
  Which shape JP needs is unverified.
- **Ollama has no endpoint yet.** An upstream PR adding `/api/tokenize` is open,
  not merged.
  Ollama returns `Ok(None)` until it lands.

## Implementation Plan

### Phase 1 — Unit alignment

Convert `jp_llm::window` from characters to tokens: `estimate_tokens`,
`estimate_overhead_tokens`, `budget_tokens`, and `truncate_to_fit` taking
`overhead_tokens`.
`CHARS_PER_TOKEN` becomes private to the estimator.
Behavior-preserving; callers in `title.rs` and `query/tool/inquiry.rs` follow
the rename.
Merges independently.

### Phase 2 — Counting and its first consumer

One vertical slice:

- `count_input_tokens` on `Provider`, defaulting to `Ok(None)`.
- The Anthropic implementation: top-level allowlist projection over
  `create_request`'s output, posted to `/v1/messages/count_tokens` via the
  existing `Client::post`.
- Bisection in the summarizer's `ContextWindowExceeded` path, reporting the
  largest range that fits with headroom.
- VCR cassettes for the count endpoint, including the 400 that an unprojected
  body produces — that response is the reason the projection exists and should
  fail loudly if the projection regresses.

Depends on phase 1.
Roughly 7 count calls per failed summarize on a 150-turn range, ~0.6s each at 4
MB, shrinking as the search narrows.

### Phase 3 — Remaining providers

Google (`countTokens`) and llama.cpp (`/apply-template` + `/tokenize`).
Each is a new value on the provider axis with no edits to the consumer.
Independently mergeable, one per provider.

## References

- [Anthropic token counting][anthropic-count] — endpoint shape, accepted
  fields, pricing and rate limits, and the note that the count is itself an
  estimate.
- [RFD 064] — non-destructive compaction, which defines the summary ranges this
  RFD helps a user choose.
- Commit `cf989181` — removes the character-based pre-flight gate and corrects
  `CHARS_PER_TOKEN` from 3 to 2, the baseline this RFD builds on.

[RFD 064]: ../064-non-destructive-conversation-compaction.md
[anthropic-count]: https://platform.claude.com/docs/en/build-with-claude/token-counting
