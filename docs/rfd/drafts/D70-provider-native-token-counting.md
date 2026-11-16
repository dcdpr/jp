# RFD D70: Provider-Native Token Counting

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-09-09
- **Summary**: Adds `Provider::count_input_tokens` backed by provider
  token-counting endpoints to compute fitting turn ranges when summarization
  overflows.

## Summary

This RFD adds `count_input_tokens` to the `Provider` trait, backed by each
provider's native token-counting endpoint where available.
When a summarization request exceeds the model's context window, JP uses token
counts to report a turn range that fits instead of leaving the user to
binary-search ranges by hand.

## Motivation

Compacting a long conversation with `--summary` requires picking a turn range
that fits the summarizer's context window.
Because JP does not report which range fits, the user has to guess.
In one session, finding a valid range took eight commands over seven minutes:

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

Each attempt uploaded a multi-megabyte inference request only to fail on the
context limit, turning range selection into a manual binary search with full
inference requests as probes.
The user already knows how to narrow the range; they only lack the token counts
needed to find the boundary in one step, which a provider token-counting
endpoint returns directly.

Without provider token counting, range selection stays manual and other
consumers must rely on character estimates.
Two other drafts, `115` (bounded tool output) and `D50` (automatic compaction),
defer accurate counting to avoid adding a tokenizer dependency and work around
its absence with safety margins.

## Design

### Error Reporting

When a turn range exceeds the context window, JP reports the provider's error
alongside a suggested range that fits:

```
Summarization failed

      model  anthropic/claude-opus-5
     reason  api error: invalid_request_error: prompt is too long: 1318026
             tokens > 1000000 maximum
 suggestion  Turns 2..25 fit, at 984210 tokens. Re-run with `--turn=2..25`, or
             summarize with a larger-window model.
```

The command prints the error and exits non-zero.
It does not automatically narrow the range and proceed: silently summarizing a
smaller turn span than requested is easy to miss, and reporting a fitting range
already eliminates the manual search.

When the provider does not support token counting, or when the count call fails,
JP reports the error without the `suggestion` line.
It does not bisect using the character heuristic, because presenting an
estimated range as exact would reintroduce the guesswork this RFD removes.

### Counting on the Failure Path

JP computes the suggestion only after the API rejects the summarization request,
never as a pre-flight check.

A count call uploads the same payload as the inference request.
Counting before every summarization would save nothing when the range overflows
(both paths upload the payload) and would add a full upload whenever the range
fits, which is the common case.
In benchmarks, counting a 4,220,150-byte payload took 0.63 seconds.
A 7-step bisection over a 150-turn range takes a few seconds on the failure
path, compared to minutes of manual retries.

### Provider Trait Method

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

The return type distinguishes three outcomes:

| Result        | Meaning                            | Caller                    |
| ------------- | ---------------------------------- | ------------------------- |
| `Ok(Some(n))` | counted with the model's tokenizer | use it                    |
| `Ok(None)`    | provider has no counting endpoint  | skip the suggestion       |
| `Err(_)`      | counting was attempted and failed  | warn, skip the suggestion |

The default implementation returns `Ok(None)` rather than an estimate, so
callers never mistake a heuristic for a tokenizer count.

The method takes `&ChatQuery` rather than `ChatQuery` so the caller retains the
query to send it.
Providers clone the query internally, and the network round trip dominates that
cost.

### Building the Count Request Body

Each provider derives its own count request body from `ChatQuery`.
For Anthropic, JP applies a **top-level allowlist** to the output of
`create_request` rather than stripping known-unsupported fields.
The `/v1/messages/count_tokens` endpoint accepts `messages`, `model`, `system`,
`thinking`, `tools`, `tool_choice`, and `mcp_servers`, and rejects any other
top-level field:

```
{"type":"error","error":{"type":"invalid_request_error",
 "message":"max_tokens: Extra inputs are not permitted"}}
```

`create_request` can emit eight fields outside that set (`stream`, `max_tokens`,
`cache_control`, `service_tier`, `output_config`, `temperature`, `top_p`, and
`top_k`), five of which depend on user configuration.
A denylist tested against a default request body would pass review and then fail
with HTTP 400 for the first user who configures `temperature`.
An allowlist fails safe: newly added request parameters are omitted
automatically.

The filter applies only to top-level keys.
Block-level `cache_control` entries nested inside `messages`, `system`, and
`tools` are accepted and ignored by the count endpoint, and must survive the
projection.

This projection has two consequences:

1. The counted body is not byte-identical to the inference body because sampling
   and output parameters are omitted, but those parameters do not affect input
   tokenization.
2. The returned count is itself an approximation: Anthropic documents that the
   actual request token count may differ slightly and includes tokens added for
   internal system optimizations.

### Unit Alignment

`jp_llm::window` currently measures sizes in characters via `estimate_chars`,
`budget_chars`, `estimate_overhead_chars`, and `truncate_to_fit` (which takes
`overhead_chars`).
No signature in the module accepts a token count.

This RFD converts `jp_llm::window` to tokens, making `CHARS_PER_TOKEN` a private
implementation detail of the estimator.
The arithmetic is unchanged apart from a single division, so existing behavior
is preserved, and the budgeting and selection functions can accept token counts
from either a provider endpoint or the character heuristic.

## Drawbacks

- **Extra round trips on the failure path.** A rejected summarization makes the
  initial failed request plus several token-count calls.
  Anthropic does not bill for token counting and rate-limits it separately, but
  the extra calls still add latency to a failed command.
- **Uneven provider support.** Anthropic, Google, and llama.cpp can count
  tokens, whereas OpenAI, Cerebras, OpenRouter, and Ollama cannot.
  Users on unsupported providers receive no suggestion and must still narrow
  ranges by hand.
- **Two token-sizing mechanisms.** `truncate_to_fit` continues to estimate with
  characters while the summarizer uses provider token counts, so the two paths
  can disagree on the size of the same input.
  This is acceptable because they make different decisions (`truncate_to_fit`
  picks a truncation boundary, whereas the summarizer reports a token count),
  but the inconsistency is real.
- **Per-provider projection logic.** Each counting implementation must project
  its request body to match the provider's count schema, adding maintenance that
  the character heuristic did not require.

## Alternatives

### Keep a pre-flight gate, but make it accurate

The character-based pre-flight gate was removed before this RFD was written, and
replacing it with a provider token-count gate was rejected.
Because a count call uploads the same payload as the inference request, a
pre-flight gate saves nothing on the failure path and costs an extra upload on
every successful request.
Relying on the provider's own rejection is more accurate than any local check.

### A `TokenCount { Native, Approximate }` enum

An earlier design tagged the count with its provenance so callers could decide
how much to trust it.
This was rejected because returning a heuristic estimate in the same type as a
tokenizer count invites callers to treat an approximation as a measurement.
Returning `Option<u32>` states directly when a provider cannot count, while
provenance is still recorded in `debug!` logs.

### A local tokenizer library

`tiktoken-rs` and `bpe-openai` are well-maintained, but they only bundle OpenAI
vocabularies.
Neither Anthropic nor Google publishes a tokenizer vocabulary, so `cl100k` is
only a rough proxy for Claude, and Claude 4.7+ counts roughly 30% higher under
its own tokenizer than earlier Claude models.
For local models, the GGUF file already contains the exact tokenizer and
llama.cpp serves it over `/tokenize`, which no bundled vocabulary can match.
If a provider without a counting endpoint needs local tokenization later, a
tokenizer library can be added behind `count_input_tokens` without changing the
trait.

### Calibrate the estimator from one count

JP could count a conversation stream once, divide by its character estimate, and
use that ratio to scale per-event character estimates, yielding per-event
granularity from a single API call.
This would benefit `truncate_to_fit`, which needs a truncation boundary rather
than a total.
This approach is deferred because `truncate_to_fit` does not fail hard, its 80%
target factor already absorbs estimation error, and no issues have been reported
with it.

### Search by sending the real request

Automating the user's manual bisection by sending full inference requests was
rejected.
It requires the same number of round trips as counting, but each probe runs a
full inference request instead of a free token-count call.

## Non-Goals

- **Capturing `usage` from responses.** Every provider reports actual input
  tokens in inference responses, which JP currently discards.
  Response usage is exact and free, but it is retrospective: it answers how full
  the context window is after a turn rather than whether a prospective request
  will fit.
  Recording response usage involves a different mechanism and different
  consumers, so it belongs in a separate RFD.
- **Counting on the main turn path.** Nothing on the query path reads a
  prospective token count today, so this RFD does not add pre-request counting
  there.
- **Caching counts.** Token counting is free, rate-limited independently, and
  called only once per failed summarization command.
- **Auto-narrowing the range.** JP reports a fitting range and exits rather than
  summarizing a narrower range than requested.
- **Token-accurate `truncate_to_fit`.** Title generation and inquiries keep the
  character estimate.

## Risks and Open Questions

- **The suggested range can still be rejected.** Provider token counts are
  themselves estimates, and the counted body omits fields present in the real
  request.
  A suggested range whose token count lands within a percent of the context
  limit may still fail when sent.
  Mitigation: suggest the largest range that fits with headroom, reusing the
  existing target factor, rather than the largest range under the raw limit.
- **Bisection assumes monotonicity.** A larger turn range costs more tokens than
  a smaller one with the same start turn.
  This holds when the start turn is fixed and the search varies only the end
  turn; it would not hold if the search moved both bounds.
- **A single turn may exceed the window.** If the first turn alone exceeds the
  context window, no range fits and the search has no suggestion to report.
  The bisection must terminate and omit the suggestion rather than returning an
  empty range.
- **Google's count body needs validation.** `countTokens` accepts either bare
  `contents` or a wrapped `generateContentRequest` when system instructions and
  tools must be counted.
  Which request shape JP needs is not yet verified.
- **Ollama has no endpoint yet.** An upstream PR adding `/api/tokenize` is open
  but not merged, so Ollama returns `Ok(None)` until that endpoint ships.

## Implementation Plan

### Phase 1: Unit alignment

Convert `jp_llm::window` from characters to tokens: `estimate_tokens`,
`estimate_overhead_tokens`, `budget_tokens`, and `truncate_to_fit` taking
`overhead_tokens`.
`CHARS_PER_TOKEN` becomes private to the estimator, and callers in `title.rs`
and `query/tool/inquiry.rs` follow the rename.
This phase preserves existing behavior and merges independently.

### Phase 2: Counting and its first consumer

Implement the trait method, the Anthropic endpoint, and summarizer bisection as
one vertical slice:

- Add `count_input_tokens` to `Provider`, defaulting to `Ok(None)`.
- Implement `count_input_tokens` for Anthropic using a top-level allowlist
  projection over `create_request`'s output, posted to
  `/v1/messages/count_tokens` via the existing `Client::post`.
- Bisect turn ranges in the summarizer's `ContextWindowExceeded` path and report
  the largest range that fits with headroom.
- Add VCR cassettes for the count endpoint, including the HTTP 400 response
  produced by an unprojected body so regressions fail in tests.

This phase depends on Phase 1.
A failed summarize over a 150-turn range makes roughly 7 count calls, starting
at ~0.6s for a 4 MB payload and shrinking as the search narrows.

### Phase 3: Remaining providers

Implement `count_input_tokens` for Google (`countTokens`) and llama.cpp
(`/apply-template` + `/tokenize`).
Neither requires changes to the summarizer consumer, and each provider can merge
independently.

## References

- [Anthropic token counting][anthropic-count]: endpoint shape, accepted fields,
  pricing and rate limits, and the note that the count is itself an estimate.
- [RFD 064]: non-destructive compaction, which defines the summary ranges this
  RFD helps a user choose.
- Commit `cf989181`: removes the character-based pre-flight gate and corrects
  `CHARS_PER_TOKEN` from 3 to 2, the baseline this RFD builds on.

[RFD 064]: ../064-non-destructive-conversation-compaction.md
[anthropic-count]: https://platform.claude.com/docs/en/build-with-claude/token-counting
