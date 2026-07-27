# RFD D59: Non-Destructive Hide Overlay

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz
- **Date**: 2026-07-27

## Summary

Introduce `hide` as a second member of the non-destructive overlay event group
alongside `compaction`: an appended event that withholds specific conversation
events from the assistant while leaving them durable and visible to the user.

## Motivation

Some content must stop reaching the model without being deleted.

The concrete case is a safety refusal.
When a classifier declines mid-stream, the partial assistant output that already
streamed is incomplete, and replaying it on the next turn feeds the model a
truncated turn it never finished.
Anthropic's guidance is to discard it.

Deleting it is the wrong remedy.
The user already watched that text arrive in their terminal, so removing it
erases something they saw and may value.
Truncated-by-`max_tokens` tool calls have the same shape: the buffer is unusable
to the model but the user watched it stream.

Doing nothing means the incomplete content is replayed on every subsequent
request for the life of the conversation.

Compaction already solved the general problem — appended overlay events that
change what the assistant sees without touching the underlying stream — for the
size-reduction case.
Hide is the correctness-filter case, and it wants the same mechanism.

## Design

### What the user sees

Nothing, by default.
A hidden event still renders in `jp conversation print`, annotated to say it was
hidden and why:

```text
jp ▸ Starting with the failing tests for B1 and B2…
    ⚠ hidden: the model began this response, then declined it
```

A new `--projected` flag renders the stream as the assistant receives it, with
all overlays applied, so hidden events are absent and compaction summaries
replace their ranges.
That flag is the only place the show/hide decision is exposed: the live view
already showed the content as it streamed, and the request path always uses the
projection.

### The overlay group

`Compaction` and `Hide` are both overlay events: appended, never mutating, and
consumed by the projection layer rather than surviving into the projected
stream.

```rust
/// Withholds specific events from the assistant without removing them.
pub struct Hide {
    pub timestamp: DateTime<Utc>,

    /// Events to withhold, by stable identifier.
    pub targets: Vec<EventId>,

    /// Why these events were hidden.
    pub reason: HideReason,
}

pub enum HideReason {
    /// A safety classifier declined the request mid-stream, leaving the
    /// assistant's partial output incomplete.
    Refusal { category: Option<String> },
}
```

`targets` names events by stable id, which is the sub-turn precision this needs
and which compaction's `from_turn`/`to_turn` range model cannot express.

### Ordering: hide applies before compaction

Hide is the base layer.
Compaction sits on top of it, in both directions:

- **Projection**: `projection::apply` removes hidden events first, then applies
  compaction policies to what remains.
- **Summary generation**: a summary is generated from the *hidden* base, so
  hidden content never leaks into summary text, where it would become permanent.

A compaction summary is therefore cached derived state keyed on its input range.
A hide landing inside an existing summary's range invalidates that cache and the
summary must be regenerated.
This never occurs on the refusal path, where the hide lands on a fresh turn
before any compaction covers it, but the contract needs stating because
regeneration costs an LLM call.

### Visibility is not a per-reason property

A hide is **always** invisible to the assistant.
That is its entire purpose, and it holds everywhere the assistant reads history,
not per call site.
Any future conversation-access tool must read the projected view or a hide leaks
straight back in.

User visibility is a *view* concern, resolved by `--projected`, not a property
of the hide or its reason.

### Emitting one

The refusal path already has what it needs.
`FinishReason::Refused { category, explanation }` is produced by the Anthropic
provider today and drives the existing chrome notice.
The turn coordinator additionally appends a `Hide` targeting the refused cycle's
trailing `ChatResponse` events, with the same category as its reason.
The decline metadata does double duty and the refusal path does exactly one new
thing.

## Drawbacks

Conversations grow monotonically.
Hidden content is durable, so a conversation that accumulates refusals
accumulates bytes that no longer serve the model.
Since overlays already have this property, hide does not introduce it.

Two views of one conversation is a real cognitive cost.
A user reading the default view sees text the model does not have.
The annotation is what makes that legible rather than confusing, which is why
unmarked rendering is explicitly rejected below.

## Alternatives

**Destructive rollback.** Pop the refused cycle's events from the stream before
persisting.
Simpler, and no projection changes at all.
Rejected because it erases content the user already saw, and because it makes
the refusal path a special case rather than an instance of a general mechanism.

**A `hide` policy on `Compaction` instead of a sibling event.** Rejected on two
grounds.
Vocabulary: compaction is a deliberate size-reduction operation, so "compacting"
a conversation in response to a refusal is a category error.
Shape: compaction is turn-range-scoped while hide is event-id-targeted, and
forcing both into one struct gives a union where half the fields are meaningless
depending on the policy.

**Render hidden events as ordinary content.** Rejected: it recreates the
divergence the annotation exists to prevent, where a user references text and
the model denies having said it.

**Transactional turns.** Buffer streamed assistant content and commit only on
clean completion.
Rejected as disproportionate: it fights JP's incremental-commit model, where
live rendering and projection read the stream as the source of truth *during*
the turn.

## Non-Goals

- **Hiding from the user.** A future reason may want that (redacting a leaked
  secret, say), and the single `--projected` toggle cannot express
  "model-invisible always, user-invisible by default".
  Deferred consciously.
- **Refusal fallback.** Re-running a refused request on a different model is
  separate and larger.
- **A user-facing hide command.** This RFD covers the mechanism and its first
  automatic producer, not manual curation.

## Risks and Open Questions

- **Summary regeneration cost.** A hide inside an already-summarized range
  triggers an LLM call.
  Acceptable because it cannot happen on the refusal path, but a manual hide
  command would make it reachable.
- **Every assistant-facing read must honour the projection.** Today that is only
  the request builder.
  This becomes an invariant to hold, not a fact to verify once.
- **Mid-stream versus pre-output refusals.** A refusal arriving before any
  output has nothing to hide and must not emit an empty overlay.
- **Annotation rendering** in the default view is unspecified beyond the sketch
  above and should follow whatever RFD 048's channel model implies.

## Implementation Plan

**Phase 1 — Generalize the overlay group.** Rename what is now
compaction-specific projection into overlay-general terms and document
hide-before-compaction ordering.
Depends on \#544.
Independently mergeable, no behaviour change.

**Phase 2 — `Hide` event and projection.** Add the event, its `HideReason`, and
omission in `projection::apply` before compaction.
Depends on D24 for stable event identifiers.
Independently mergeable and inert until something emits one.

**Phase 3 — Summary cache invalidation.** Regenerate a compaction summary whose
input range gains a hide.
Depends on phase 2.

**Phase 4 — Emit on refusal.** The turn coordinator appends a `Hide` for the
refused cycle's trailing responses.
Depends on phase 2.
This is where behaviour changes.

**Phase 5 — `--projected`.** Add the flag to `jp conversation print`, plus the
default-view annotation.
Depends on phase 2, independent of phase 4.

## References

- RFD 064: Non-Destructive Conversation Compaction — the overlay pattern this
  extends
- RFD D24: stable event identifiers — required for `targets`
- RFD 040: Hidden Conversations and Tool Context — reconcile the word "hidden"
  against this usage
- RFD 048: Four-Channel Output Model — annotation rendering
- [Anthropic: refusals and fallback] — "treat any partial output as incomplete
  and discard it"

[Anthropic: refusals and fallback]: https://platform.claude.com/docs/en/build-with-claude/refusals-and-fallback
