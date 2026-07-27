<!--
  This template is a starting point, not a constraint. Delete sections that
  don't apply, add sections that do, or restructure entirely. The only
  requirement is the metadata header (Status, Authors, Date).

  Use HTML comments like this one for draft-time notes and review markers.
  They do not appear in the rendered output and can be removed when the RFD
  advances to Discussion status.
-->

# RFD D24: Capability-Derived Defaults for Unknown Anthropic Models

- **Status**: Draft
- **Category**: Design
- **Authors**: rgrant <rgrant@contract.design>
- **Date**: 2026-07-27

## Summary

`map_model` in the Anthropic provider resolves unrecognized model ids to
`ModelDetails::empty`, which leaves `reasoning: None` and silently disables
thinking for any model Anthropic ships before JP adds an exact-match arm.
This RFD replaces that single fallback with a tiered one: derive reasoning
support from the capability data the models API already reports, fill the
remaining gaps from the newest known model of the same family, and fall back
to the empty details only when both tiers produce nothing.
When configured reasoning settings cannot be honored, JP warns instead of
staying silent.

## Motivation

Thinking is opt-in on the Anthropic API.
`create_request` decides whether to emit a `thinking` field or
`output_config.effort` by matching on `model.reasoning`.
For an id without an exact-match arm, the catch-all in `map_model` builds
`ModelDetails::empty` with `reasoning: None`, so neither field is emitted:
the model runs without thinking, and a configured
`model.parameters.reasoning.effort` is discarded.
The only trace is a `debug!` line.

This failure recurs by construction.
Every new Anthropic model is unknown to JP on release day — precisely the day
users want to try it — and the result is degraded output with no error and no
visible signal.

The models API has since started reporting per-model capability data that
resolves most of the guesswork, which is what makes this design possible.

## Design

### User-visible behavior

- Selecting a new, unrecognized model from a known lineup (e.g.
  `claude-opus-5`) keeps thinking and configured effort working.
- When JP infers model details rather than knowing them, it logs a `warn!`
  naming the model and the source of the inference (capabilities or family).
- When a configured reasoning setting cannot be honored — the model's details
  report no thinking support at all — JP logs a `warn!` saying the setting is
  ignored, instead of dropping it silently.

### Design constraints

Four rules, in order of precedence:

1. **Known model: the arm answers alone.** An id with an exact-match arm
   resolves entirely from that arm.
   Capability data is never consulted for it, and this design adds no network
   calls anywhere — `map_model` stays a pure function over the response the
   provider already fetched.
2. **Unknown model, API answers: no guessing.** Every fact the response
   reports — thinking type, effort levels, token limits, structured-output
   support — is used as-is.
   Family heuristics never override a reported fact.
3. **Unknown model, API silent on a field: newest family sibling.** Each
   field the response leaves unanswered fills from the newest known
   exact-match arm of the same family.
4. **No family, no answer: raw defaults.** `ModelDetails::empty` plus
   whatever the response did report — the current behavior.

The three tiers below implement rules 2–4; rule 1 is the existing match
statement, untouched.

### Tier 1: capability-derived reasoning

The `/v1/models` response reports a `capabilities` object per model,
including `thinking.supported`, `thinking.types.{adaptive, enabled}`, and
per-level `effort.{low, medium, high, xhigh, max}` flags ([models API]).
The pinned `async-anthropic` fork parses these into `ModelCapabilities`,
`ThinkingCapability`, and `CapabilityGroup` ([async-anthropic]).

For any id reaching the catch-all, reasoning derives from capabilities:

| Reported                        | Derived `ReasoningDetails`             |
| ------------------------------- | -------------------------------------- |
| `thinking.types.adaptive`       | `adaptive(effort.xhigh, effort.max)`   |
| `thinking.types.enabled` only   | `budgetted(1024, None)`                |
| `thinking.supported == false`   | `unsupported()`                        |
| thinking not reported           | fall through to tier 2                 |

The `1024` minimum matches Anthropic's documented minimum thinking budget and
every existing budgetted arm.
An effort level absent from the response counts as unsupported — a configured
`max` or `xhigh` then degrades to `high`, which is conservative but safe.

This tier requires distinguishing "the API reported `false`" from "the API
reported nothing".
The pinned `async-anthropic` revision models capability flags as `bool` with
serde defaults, which collapses absent to `false`; the fork's `main` already
models them as `Option<bool>`, so the pin moves forward as part of this
change.

This tier is family-agnostic: a model from an entirely new lineup (e.g.
`claude-nova-1`) gets correct reasoning support as long as the API reports
its capabilities.

### Tier 2: family inheritance

Capabilities do not cover everything `ModelDetails` needs.
The `features` list (`interleaved-thinking`, `thinking-always-on`, `prefill`)
is not reported, and the API can report `0` for `max_input_tokens` /
`max_tokens`, meaning unspecified — the API reference's own example does so
for `claude-opus-4-6`.

For ids matching `claude-<family>-<version>[-<date>]` where `<family>` is one
of `opus`, `sonnet`, `haiku`, `fable`, the remaining gaps fill from the
**newest known exact-match arm of the same family**.
An unknown id is by definition newer than every id JP has an arm for, so the
newest known family member is the closest available model.
Inheriting from it, rather than from a hand-maintained defaults table, means
adding a new exact-match arm automatically updates the family's fallback.

Inherited: `reasoning` (only if tier 1 produced nothing), `features`,
`context_window` and `max_output_tokens` (only where the API reported `0`),
`structured_output` (only where the API reported nothing).
Never inherited: `knowledge_cutoff` and `deprecated` stay `None` — no
invented dates — and `id` / `display_name` come from the API response.

This resolves the haiku question without a hand-picked guess: today an
unknown haiku inherits from `claude-haiku-4-5` (budgetted), and if a future
adaptive haiku gets an exact arm, the fallback follows.
In practice tier 1 is expected to answer the reasoning question for real
haiku releases before tier 2 is consulted.

Old-style ids of the form `claude-<version>-<family>` (e.g.
`claude-3-5-sonnet-20241022`) deliberately do not match the family grammar
and get no inheritance.
Those generations predate the current lineup's capabilities, so inheriting
from the newest arm (1M context, adaptive thinking) would be wrong in every
field; tier 1 and tier 3 cover them.

### Tier 3: empty details

Ids matching no known family, with no reported thinking capabilities, keep
the current behavior: `ModelDetails::empty` plus whatever concrete values the
API reported.

### Precedence

Concrete API-reported values always beat inherited ones — the catch-all
already prefers reported token limits and structured-output support, and that
behavior is kept.
Capability-derived reasoning beats family-inherited reasoning: capabilities
are the provider's own claim about the model, family inheritance is JP's
guess.

### Logging

The catch-all's `debug!` becomes a `warn!` whenever tier 1 or tier 2
supplied any value, stating which tier fired.
In `create_request`, when a reasoning config is present but `model.reasoning`
is `None` or `unsupported()`, a single `warn!` states that the configured
reasoning is ignored for this model.
This is a one-line addition at the existing fall-through; `create_request` is
not restructured.

## Drawbacks

- Inferred details can be wrong.
  A wrong adaptive-vs-budgetted guess produces a loud API error instead of
  today's silent degradation — an improvement, but still a failure the user
  sees.
- "Newest known arm" requires a deterministic version ordering parsed from
  ids, which adds a small parsing surface to maintain.
- Users deliberately running obscure or aliased ids will see recurring
  `warn!` lines they cannot fully silence without adding an exact arm.

## Alternatives

- **Family-name inference only** (the original idea).
  Rejected as the primary mechanism: the models API reports the decisive
  facts (adaptive vs. budgetted thinking, supported effort levels), and
  heuristics should not guess where facts are available.
- **Capabilities only, no family tier.**
  Simpler, but loses the `features` residue.
  `interleaved-thinking` affects thinking-budget computation and
  `thinking-always-on` drives the forced-tool retry strategy, so dropping
  them degrades unknown models in subtler ways than reasoning loss.
- **Replace the exact-match arms with fully capability-driven mapping.**
  Attractive long-term, but a much larger change touching known-model
  behavior; deferred (see Non-Goals).
- **Fail on unknown models.**
  Hostile to the primary use case — trying a new model on release day.

## Non-Goals

- Restructuring `create_request` or the retry strategies.
- Replacing the exact-match arms for known models; they remain authoritative.
- Applying the same scheme to other providers.
  The mechanism is Anthropic-specific because the capability data is.
- Caching or refreshing the models list; this RFD only changes how an
  already-fetched `types::Model` maps to `ModelDetails`.

## Risks and Open Questions

- `thinking.types` does not report whether thinking can be *disabled*, so the
  `thinking-always-on` distinction (Fable) still rests on family inheritance.
- Anthropic may report capabilities inconsistently across models (the token
  limits already show `0`-as-unspecified); each capability read must treat
  "absent" as "unknown", never as "unsupported", except for the explicit
  `supported: false` case.

## Implementation Plan

Single phase, tests first, confined to `map_model` plus helpers and the two
`warn!` sites.

1. Unit tests in `anthropic_tests.rs` (red on current code):
   - `claude-opus-5` with capabilities reporting adaptive + xhigh + max
     yields `adaptive(true, true)` (tier 1).
   - `claude-opus-5` with no reported capabilities yields the details of the
     newest known opus arm minus `knowledge_cutoff` / `deprecated` (tier 2).
   - `claude-sonnet-5-1-20260901` resolves to the sonnet family.
   - `claude-nova-1` with no reported capabilities keeps the empty fallback,
     `reasoning: None` (tier 3).
   - `claude-nova-1` with reported thinking capabilities gets derived
     reasoning (tier 1 is family-agnostic).
   - `claude-opus-4-5` still resolves via its exact arm to budgetted details.
   - API-reported token limits override family-inherited ones.
2. Bump the `async-anthropic` pin to a revision where capability flags are
   `Option<bool>` (the fork's `main` already is), so absent capabilities are
   distinguishable from reported-`false`.
   Adjust the one existing read (`structured_outputs.supported`) in the
   catch-all to the `Option` type.
3. Implement: an id parser (`family`, `version`), capability-to-reasoning
   derivation, newest-known-sibling lookup, and the tiered fill in the
   catch-all.
4. Add the two `warn!` lines (inference fired; configured reasoning ignored).
5. `cargo check` / `cargo test` for `jp_llm`.

## References

- [models API] — Anthropic `GET /v1/models` reference, including the
  `capabilities` object.
- [async-anthropic] — the pinned SDK fork whose `types::Model` parses the
  capability data.
- `crates/jp_llm/src/provider/anthropic.rs` — `map_model` and
  `create_request`.

[models API]: https://docs.claude.com/en/api/models-list
[async-anthropic]: https://github.com/JeanMertz/async-anthropic
