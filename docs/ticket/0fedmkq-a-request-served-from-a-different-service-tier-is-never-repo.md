# A request served from a different service tier is never reported

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-09-08

`assistant.model.parameters.service_tier` asks a provider for a grade of
capacity, and JP refuses the query when the provider sells no equivalent.
It cannot refuse what the provider accepts and then serves from somewhere else,
and today it does not notice either.

Every provider reports which tier actually served the request.
JP reads none of them:

- OpenAI and OpenRouter return `service_tier` at the top level of the response.
- Anthropic returns `usage.service_tier` and `usage.speed`.
- Cerebras returns `service_tier_used` for a request that asked for `auto`.

`jp_openrouter::types::response::ChatCompletion` has no `service_tier` field,
and the OpenRouter provider never sets the request's `usage` flag either, so
`Usage.cost` is unpopulated as well.

## Why it matters

The tier decides what the request costs, so a substitution the user cannot see
is a silent billing change.
Named inputs that reach it:

- `jp q --model openrouter/anthropic/claude-opus-5 --tier flex "..."`.
  OpenRouter documents that flex's no-fallback guarantee holds only when flex
  endpoints exist: with an empty flex pool "the request routes normally at
  standard rates".
  Anthropic on OpenRouter is priority-only, so this model has no flex pool and
  the query succeeds at the regular price.
- An OpenAI `flex` request shed to `default` under load.
- `speed: "fast"` on Claude Opus 4.6, which Anthropic documents as running at
  standard speed and standard rates while reporting `usage.speed: "standard"`.

Each is contained to one request, but none is visible, which is what makes this
worth fixing rather than recording.

## Scope

Parse the served tier per provider, record it alongside the turn, and tell the
user when it differs from what they asked for.

A `warn!` does not discharge this on its own: `configure_tracing` maps default
verbosity to `LevelFilter::ERROR`, so the report has to reach the printer's
notice channel to be seen at all.
That dependency is the reason this is not a one-line change, and it is shared
with the wider question of how contained degradations get surfaced (see
T-0dd9cpr and RFD 048).

Out of scope: predicting a substitution before dispatch.
That would need OpenRouter's per-model endpoint listing on the query path, and
the provider already resolves the condition server-side and tells us afterwards.

## Comments

-----

- **From**: jp
- **Date**: 2026-09-09T09:54:19Z

Anthropic's `flex` now resolves to the Message Batches API rather than a
refusal, which adds a case here.

A batched request is billed at 50% of standard rates, and the batch result's
`usage` object carries token counts but nothing that names the discount.
So the "which tier actually served this" report cannot be read off the response
for this route — it has to come from the fact that JP submitted a batch at all.

`jp_llm::provider::anthropic::Transport` is the place that knows, and it is
already the single point both routes pass through.

-----

- **From**: jp
- **Date**: 2026-09-09T10:50:49Z
- **Re**: #1

Withdrawn: the branch that made `flex` resolve to the Message Batches API was
abandoned, so `flex` on `anthropic` still fails with `UnsupportedServiceTier`
and this ticket's original scope is unchanged.

Findings from that branch are recorded in T-0g1w1mw.
